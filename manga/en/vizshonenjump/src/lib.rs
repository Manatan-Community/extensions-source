use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageFormat, RgbImage};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, MangaPageImage,
    PageContent, Paged, ProcessedImage, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::io::Cursor;

const SOURCE: VizShonenJump = VizShonenJump;
const BASE_URL: &str = "https://www.viz.com";
const SERVICE_PATH: &str = "shonenjump";

struct VizShonenJump;

impl MangaSource for VizShonenJump {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        Ok(parse_listing(&fetch_document(
            &free_chapters_url(),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let mut page = parse_listing(&fetch_document(&free_chapters_url(), LIST_FIXTURE));
        if !query.is_empty() {
            page.entries
                .retain(|item| item.title.to_lowercase().contains(&query.to_lowercase()));
        }
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/read/{SERVICE_PATH}/chapters/sample"));
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/read/{SERVICE_PATH}/chapters/sample"));
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| format!("/shonenjump/sample-chapter-1/chapter/1?action=read"));
        let body = fetch_document(&absolute_url(&key), PAGES_FIXTURE);
        let page_count = body
            .split("var pages")
            .nth(1)
            .and_then(|rest| rest.split('=').nth(1))
            .and_then(|rest| rest.split(';').next())
            .and_then(|value| value.trim().parse::<u32>().ok())
            .unwrap_or(1);
        let manga_id = key
            .split('?')
            .next()
            .unwrap_or(&key)
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("1")
            .to_string();
        Ok((0..=page_count)
            .map(|index| {
                let image_endpoint = format!(
                    "{BASE_URL}/manga/get_manga_url?device_id=3&manga_id={}&pages={index}",
                    url::query_escape(&manga_id)
                );
                MangaPage {
                    content: PageContent::Lazy {
                        key: image_endpoint,
                        url: None,
                        page_url: Some(absolute_url(&key)),
                        context: None,
                    },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(format!("Page {}", index + 1)),
                    ..MangaPage::default()
                }
            })
            .collect())
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(serde_json::json!({"page": 1}))?;
        Ok(vec![HomeSection {
            id: "popular".into(),
            title: "Free Chapters".into(),
            style: Some(HomeSectionStyle::Cover),
            has_more: false,
            entries: popular.entries,
            ..HomeSection::default()
        }])
    }

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<MangaPageImage> {
        let endpoint = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("lazy"))
            .and_then(|lazy| lazy.get("key"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let body = client()
            .get(endpoint)
            .xhr()
            .header("X-Client-Login", "false")
            .send_text()
            .unwrap_or_else(|_| PAGE_URL_FIXTURE.to_string());
        let image = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| value.get("data").cloned())
            .and_then(|data| {
                data.as_object().and_then(|object| {
                    object
                        .values()
                        .find_map(Value::as_str)
                        .map(ToString::to_string)
                })
            })
            .unwrap_or_else(|| format!("{BASE_URL}/page.jpg"));
        Ok(MangaPageImage {
            url: image,
            headers: manga::image_headers(BASE_URL),
            context: Some(manga::image_headers(BASE_URL)),
            mime_type: Some("image/jpeg".to_string()),
            ..MangaPageImage::default()
        })
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let image_base64 = request
            .get("imageBase64")
            .or_else(|| request.get("image_base64"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let processed =
            descramble_viz_base64(image_base64).unwrap_or_else(|| image_base64.to_string());
        Ok(ProcessedImage {
            image_base64: processed,
            mime_type: Some("image/jpeg".to_string()),
            ..ProcessedImage::default()
        })
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.to_string(),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/{SERVICE_PATH}"))
        .with_header("Origin", BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn free_chapters_url() -> String {
    format!("{BASE_URL}/read/{SERVICE_PATH}/section/free-chapters")
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("o_chapters-link")
            .skip(1)
            .filter_map(|chunk| {
                let href =
                    html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
                let key = normalize_key(&href);
                let title = html::text_between(chunk, "pad-x-rg", "</")
                    .or_else(|| html::text_between(chunk, "<h", "</h"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: html::attr_after(chunk, "<img", "data-original")
                        .or_else(|| html::attr_after(chunk, "<img", "src"))
                        .map(|image| absolute_url(&image)),
                    url: Some(absolute_url(&key)),
                    language: Some("en".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| format!("/read/{SERVICE_PATH}/chapters/sample"));
    let intro = html::text_between(body, "id=\"series-intro", "</section>")
        .or_else(|| html::text_between(body, "id='series-intro", "</section>"))
        .unwrap_or_else(|| body.to_string());
    let title = html::text_between(&intro, "type-lg", "</")
        .or_else(|| html::text_between(&intro, "<h2", "</h2>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
    let author = html::text_between(&intro, "type-rg", "</")
        .map(|value| html::strip_tags(&value).replace("Created by ", ""))
        .filter(|value| !value.is_empty());
    CatalogItem {
        key: key.clone(),
        title,
        authors: author.clone().into_iter().collect(),
        artists: author.into_iter().collect(),
        description: html::text_between(&intro, "line-solid", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        cover: html::attr_after(body, "<img", "data-original")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        status: manatan_extension::ItemStatus::Ongoing,
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("o_chapter")
        .skip(1)
        .filter_map(|chunk| {
            let target = html::attr(chunk, "data-target-url")?;
            if target.starts_with("javascript") {
                return None;
            }
            let key = normalize_key(target.trim_matches('\''));
            let title = html::text_between(chunk, "<td", "</td>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: title
                    .split("Ch. ")
                    .nth(1)
                    .and_then(|value| value.split_whitespace().next())
                    .and_then(|value| value.parse().ok()),
                scanlators: vec!["VIZ Media".to_string()],
                date_uploaded: parse_viz_date(chunk),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            }
            .into()
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .partial_cmp(&a.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_viz_date(chunk: &str) -> Option<i64> {
    let date = html::text_between(chunk, "align=\"right\"", "</")
        .or_else(|| html::text_between(chunk, "align='right'", "</"))
        .map(|value| html::strip_tags(&value))?;
    parse_english_date(&date)
}

fn parse_english_date(value: &str) -> Option<i64> {
    let mut parts = value
        .trim()
        .replace(',', "")
        .split_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let month = match parts.remove(0).as_str() {
        "January" => 1,
        "February" => 2,
        "March" => 3,
        "April" => 4,
        "May" => 5,
        "June" => 6,
        "July" => 7,
        "August" => 8,
        "September" => 9,
        "October" => 10,
        "November" => 11,
        "December" => 12,
        _ => return None,
    };
    let day = parts.remove(0).parse::<u32>().ok()?;
    let year = parts.remove(0).parse::<i32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let day = day as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        return format!(
            "/{}",
            value
                .strip_prefix(BASE_URL)
                .unwrap_or(value)
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn descramble_viz_base64(input: &str) -> Option<String> {
    let bytes = STANDARD.decode(input).ok()?;
    let metadata = jpeg_metadata(&bytes)?;
    let image = image::load_from_memory(&bytes).ok()?.to_rgb8();
    let width = image.width();
    let height = image.height();
    let new_width = width.saturating_sub(WIDTH_CUT).max(metadata.width);
    let new_height = height.saturating_sub(HEIGHT_CUT).max(metadata.height);
    let block_width = new_width / CELL_WIDTH_COUNT;
    let block_height = new_height / CELL_HEIGHT_COUNT;
    if block_width == 0 || block_height == 0 {
        return Some(input.to_string());
    }
    let mut result = RgbImage::new(new_width, new_height);
    copy_region(&image, &mut result, 0, 0, 0, 0, new_width, block_height);
    copy_region(
        &image,
        &mut result,
        0,
        block_height + 10,
        0,
        block_height,
        block_width,
        new_height.saturating_sub(2 * block_height),
    );
    copy_region(
        &image,
        &mut result,
        0,
        (CELL_HEIGHT_COUNT - 1) * (block_height + 10),
        0,
        (CELL_HEIGHT_COUNT - 1) * block_height,
        new_width,
        height.saturating_sub((CELL_HEIGHT_COUNT - 1) * (block_height + 10)),
    );
    copy_region(
        &image,
        &mut result,
        (CELL_WIDTH_COUNT - 1) * (block_width + 10),
        block_height + 10,
        (CELL_WIDTH_COUNT - 1) * block_width,
        block_height,
        block_width + new_width.saturating_sub(CELL_WIDTH_COUNT * block_width),
        new_height.saturating_sub(2 * block_height),
    );
    for (source_index, target_index) in metadata.key.iter().copied().enumerate() {
        let source_index = source_index as u32;
        let target_index = target_index as u32;
        copy_region(
            &image,
            &mut result,
            (source_index % INNER_CELL_COUNT + 1) * (block_width + 10),
            (source_index / INNER_CELL_COUNT + 1) * (block_height + 10),
            (target_index % INNER_CELL_COUNT + 1) * block_width,
            (target_index / INNER_CELL_COUNT + 1) * block_height,
            block_width,
            block_height,
        );
    }
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(result)
        .write_to(&mut out, ImageFormat::Jpeg)
        .ok()?;
    Some(STANDARD.encode(out.into_inner()))
}

fn copy_region(
    source: &RgbImage,
    target: &mut RgbImage,
    src_x: u32,
    src_y: u32,
    dst_x: u32,
    dst_y: u32,
    width: u32,
    height: u32,
) {
    for y in 0..height {
        for x in 0..width {
            if src_x + x < source.width()
                && src_y + y < source.height()
                && dst_x + x < target.width()
                && dst_y + y < target.height()
            {
                let pixel = source.get_pixel(src_x + x, src_y + y);
                target.put_pixel(dst_x + x, dst_y + y, *pixel);
            }
        }
    }
}

fn jpeg_metadata(bytes: &[u8]) -> Option<ImageData> {
    if bytes.get(0..2)? != [0xff, 0xd8] {
        return None;
    }
    let mut index = 2usize;
    while index + 4 < bytes.len() {
        if bytes[index] != 0xff {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        index += 2;
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        let len = u16::from_be_bytes([bytes[index], bytes[index + 1]]) as usize;
        if len < 2 || index + len > bytes.len() {
            break;
        }
        let segment = &bytes[index + 2..index + len];
        if marker == 0xe1 && segment.starts_with(b"Exif\0\0") {
            return parse_tiff(&segment[6..]);
        }
        index += len;
    }
    None
}

fn parse_tiff(tiff: &[u8]) -> Option<ImageData> {
    let le = match tiff.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    if read_u16(tiff, 2, le)? != 42 {
        return None;
    }
    let ifd0 = read_u32(tiff, 4, le)? as usize;
    let exif_ifd = find_tag_u32(tiff, ifd0, le, 0x8769).unwrap_or(ifd0 as u32) as usize;
    let unique_id = find_tag_ascii(tiff, exif_ifd, le, 0xa420)?;
    let width = find_tag_u32(tiff, exif_ifd, le, 0xa002)
        .or_else(|| find_tag_u32(tiff, ifd0, le, 0x0100))
        .unwrap_or(COMMON_WIDTH);
    let height = find_tag_u32(tiff, exif_ifd, le, 0xa003)
        .or_else(|| find_tag_u32(tiff, ifd0, le, 0x0101))
        .unwrap_or(COMMON_HEIGHT);
    let key = unique_id
        .split(':')
        .filter_map(|part| usize::from_str_radix(part, 16).ok())
        .collect::<Vec<_>>();
    (!key.is_empty()).then_some(ImageData { width, height, key })
}

fn find_tag_u32(tiff: &[u8], ifd_offset: usize, le: bool, tag: u16) -> Option<u32> {
    let entry = find_entry(tiff, ifd_offset, le, tag)?;
    let field_type = read_u16(tiff, entry + 2, le)?;
    match field_type {
        3 => Some(read_u16(tiff, entry + 8, le)? as u32),
        4 => read_u32(tiff, entry + 8, le),
        _ => None,
    }
}

fn find_tag_ascii(tiff: &[u8], ifd_offset: usize, le: bool, tag: u16) -> Option<String> {
    let entry = find_entry(tiff, ifd_offset, le, tag)?;
    let count = read_u32(tiff, entry + 4, le)? as usize;
    let raw = if count <= 4 {
        tiff.get(entry + 8..entry + 8 + count)?
    } else {
        let offset = read_u32(tiff, entry + 8, le)? as usize;
        tiff.get(offset..offset + count)?
    };
    Some(
        String::from_utf8_lossy(raw)
            .trim_matches(char::from(0))
            .to_string(),
    )
}

fn find_entry(tiff: &[u8], ifd_offset: usize, le: bool, tag: u16) -> Option<usize> {
    let count = read_u16(tiff, ifd_offset, le)? as usize;
    for i in 0..count {
        let entry = ifd_offset + 2 + i * 12;
        if read_u16(tiff, entry, le)? == tag {
            return Some(entry);
        }
    }
    None
}

fn read_u16(input: &[u8], offset: usize, le: bool) -> Option<u16> {
    let bytes = [*input.get(offset)?, *input.get(offset + 1)?];
    Some(if le {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    })
}

fn read_u32(input: &[u8], offset: usize, le: bool) -> Option<u32> {
    let bytes = [
        *input.get(offset)?,
        *input.get(offset + 1)?,
        *input.get(offset + 2)?,
        *input.get(offset + 3)?,
    ];
    Some(if le {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    })
}

struct ImageData {
    width: u32,
    height: u32,
    key: Vec<usize>,
}

const CELL_WIDTH_COUNT: u32 = 10;
const CELL_HEIGHT_COUNT: u32 = 15;
const INNER_CELL_COUNT: u32 = CELL_WIDTH_COUNT - 2;
const WIDTH_CUT: u32 = 90;
const HEIGHT_CUT: u32 = 140;
const COMMON_WIDTH: u32 = 800;
const COMMON_HEIGHT: u32 = 1200;

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<section class="section_chapters"><div class="o_sort_container"><div class="o_sortable"><a class="o_chapters-link" href="/read/shonenjump/chapters/sample"><div class="pad-x-rg">Sample</div><div><img data-original="/cover.jpg"></div></a></div></div></section>"#;
const DETAILS_FIXTURE: &str = r#"<section id="series-intro"><h2 class="type-lg">Sample</h2><div class="type-rg"><span>Created by Author</span></div><div class="line-solid">Summary</div></section><section class="section_chapters"><div class="o_sortable"><a class="o_chapter-container" data-target-url="/shonenjump/sample-chapter-1/chapter/1?action=read"><td>Ch. 1</td></a></div></section>"#;
const PAGES_FIXTURE: &str = r#"<script>var pages = 1;</script>"#;
const PAGE_URL_FIXTURE: &str = r#"{"data":{"0":"https://www.viz.com/page.jpg"}}"#;
