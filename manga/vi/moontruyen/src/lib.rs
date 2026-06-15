use aes::{Aes128, Aes192, Aes256};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: MoonTruyen = MoonTruyen;
const BASE_URL: &str = "https://moontruyen.com";
const DEFAULT_IMAGE_DECRYPT_KEY: &str = "%DBjZh[tcNdK4msQ";
const LOADING_IMAGE_PATH: &str = "/images/hinh-loading.png";

struct MoonTruyen;

impl MangaSource for MoonTruyen {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "danh-sach/yeu-thich"
        } else {
            "danh-sach/moi-cap-nhat"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/{path}/page-{page}/"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = if !query.is_empty() {
            format!(
                "{BASE_URL}/tim-kiem/page-{page}/?keyword={}",
                url::query_escape(query)
            )
        } else if let Some(genre) = filter(filters, "genre") {
            format!("{BASE_URL}/the-loai/{genre}/page-{page}/")
        } else {
            format!("{BASE_URL}/danh-sach/moi-cap-nhat/page-{page}/")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen/sample/chap-1".into());
        let chapter_url = absolute_url(&key);
        let body = fetch_document(&chapter_url, PAGES_FIXTURE);
        let pages = parse_pages(&body, &chapter_url);
        if pages.is_empty() {
            return Ok(vec![manga::text_page("Khong tim thay hinh anh")]);
        }
        Ok(pages)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(json!({"page": 1, "listingId": "latest"}))?,
            ),
        ])
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
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: key.contains("/truyen/").then(|| details_by_key(&key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("mcol_pos")
        .skip(1)
        .filter_map(|chunk| {
            let href = href_with(chunk, "/truyen/")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "ct_title", "</")
                .or_else(|| html::attr_after(chunk, "<a", "title"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("vi".into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("paging_prevnext next"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "de_title comictitle", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "comicthumb", "data-bg")
            .or_else(|| html::attr_after(body, "comicthumb", "src"))
            .map(|image| absolute_url(&image)),
        authors: info_value(body, "Tác giả")
            .map(|value| vec![value])
            .unwrap_or_default(),
        tags: link_texts_after(body, "lt_cate"),
        description: html::text_between(body, "lt_info99", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(&info_value(body, "Trạng thái").unwrap_or_default()),
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("table-row")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "table-data chapter", "href")
                .or_else(|| href_with(chunk, "/truyen/"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "table-data chapter", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            let raw_date = html::attr_after(chunk, "table-data", "title").or_else(|| {
                html::text_between(chunk, "table-data", "</").map(|v| html::strip_tags(&v))
            });
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: raw_date.as_deref().and_then(parse_dmy_short),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let key = decrypt_key(body);
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "data-encdes")
                .and_then(|value| decrypt_aes(&value, key.as_bytes(), key.as_bytes()))
                .or_else(|| image_attr(chunk))
        })
        .map(|image| normalize_image_url(&image))
        .filter(|image| looks_like_image(image))
        .fold(Vec::<String>::new(), |mut seen, image| {
            if !seen.contains(&image) {
                seen.push(image);
            }
            seen
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.clone(),
                context: Some(manga::image_headers(chapter_url)),
            },
            headers: manga::image_headers(chapter_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn decrypt_key(body: &str) -> String {
    let Some(base) = quoted_after(body, "var encdesbase") else {
        return DEFAULT_IMAGE_DECRYPT_KEY.into();
    };
    let suffix = quoted_after(body, "encdesbase +=").unwrap_or_default();
    format!("{base}{suffix}")
}

fn decrypt_aes(input: &str, key: &[u8], iv: &[u8]) -> Option<String> {
    let bytes = STANDARD.decode(input).ok()?;
    let out = match key.len() {
        16 => cbc::Decryptor::<Aes128>::new_from_slices(key, &iv[..16])
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(&bytes)
            .ok()?,
        24 => cbc::Decryptor::<Aes192>::new_from_slices(key, &iv[..16])
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(&bytes)
            .ok()?,
        32 => cbc::Decryptor::<Aes256>::new_from_slices(key, &iv[..16])
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(&bytes)
            .ok()?,
        _ => return None,
    };
    String::from_utf8(out)
        .ok()
        .filter(|value| !value.is_empty())
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(HomeSectionStyle::Cover),
        has_more: page.has_next_page,
        entries: page.entries,
        ..HomeSection::default()
    }
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("hoàn thành") {
        ItemStatus::Completed
    } else if lower.contains("đang tiến hành") {
        ItemStatus::Ongoing
    } else if lower.contains("tạm ngưng") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn parse_dmy_short(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('/');
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    let year = if year < 100 { 2000 + year } else { year };
    manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn info_value(body: &str, label: &str) -> Option<String> {
    let rest = &body[body.find(label)?..];
    html::text_between(rest, "rsub", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn link_texts_after(body: &str, marker: &str) -> Vec<String> {
    body.find(marker)
        .map(|index| {
            body[index..]
                .split("<a")
                .skip(1)
                .take(30)
                .map(html::strip_tags)
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn href_with(chunk: &str, needle: &str) -> Option<String> {
    chunk
        .split("<a")
        .skip(1)
        .filter_map(|part| html::attr(part, "href"))
        .find(|href| href.contains(needle))
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-bg")
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "data-original"))
        .or_else(|| html::attr(chunk, "src"))
}

fn normalize_image_url(input: &str) -> String {
    if input.starts_with("//") {
        format!("https:{input}")
    } else {
        absolute_url(input)
    }
}

fn looks_like_image(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    !lower.starts_with("data:")
        && !lower.contains(LOADING_IMAGE_PATH)
        && [".jpg", ".jpeg", ".png", ".webp", ".avif"]
            .iter()
            .any(|ext| lower.contains(ext))
}

fn quoted_after(body: &str, marker: &str) -> Option<String> {
    let rest = &body[body.find(marker)?..];
    let quote_index = rest.find(['"', '\''])?;
    let quote = rest.as_bytes()[quote_index] as char;
    let after = &rest[quote_index + 1..];
    let end = after.find(quote)?;
    Some(after[..end].to_string())
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http") {
        input
            .trim_start_matches(BASE_URL)
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .trim_end_matches('/')
            .to_string()
    } else {
        format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| key.contains("/truyen/"))
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters
        .get(id)
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .filter(|value| !value.is_empty())
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"<div class="m_l_col"><div class="mcol_ct"><div class="mcol_pos"><a href="/truyen/sample"><span class="ct_title">Sample</span></a><span class="img_link" data-bg="/cover.jpg"></span></div></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="de_title comictitle">Sample</h1><div class="comicthumb"><img src="/cover.jpg"></div><div class="lt_infocomic"><div class="cti_comic"><span class="lsub">Tác giả</span><span class="rsub">Author</span></div><div class="cti_comic"><span class="lsub">Trạng thái</span><span class="rsub">Đang tiến hành</span></div></div><div class="lt_cate"><a>Action</a></div><div class="lt_info99"><p>Summary</p></div><div class="list-chapter"><div class="table-content"><div class="table-row"><div class="table-data chapter"><a href="/truyen/sample/chap-1">Chapter 1</a></div><div class="table-data" title="01/01/24"></div></div></div></div>"#;
const PAGES_FIXTURE: &str = r#"<div class="chapter-content"><img src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
