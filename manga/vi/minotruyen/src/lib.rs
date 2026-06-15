use aes::Aes256;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use image::{DynamicImage, GenericImage, GenericImageView, ImageFormat, RgbaImage};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, manga_image, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Cursor};

type Aes256CbcDec = cbc::Decryptor<Aes256>;

const SOURCE: MinoTruyen = MinoTruyen;
const DEFAULT_BASE_URL: &str = "https://minotruyenv5.xyz";
const API_URL: &str = "https://api.cloudkk-v1.xyz/api";
const AES_KEY: &str = "GCERKSmf28E6nWwrnR8Lz4f7TacKpzMy7aK0rxSB";
const DRM_XOR_KEY: &[u8] = b"3141592653589793";
const DRM_MAP_PREFIX: &str = "#mino-v1|";

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    category: &'static str,
    rating: &'static str,
}

const SOURCES: [SourceConfig; 3] = [
    SourceConfig {
        id: "minotruyen-manga",
        category: "manga",
        rating: "suggestive",
    },
    SourceConfig {
        id: "minotruyen-comics",
        category: "comics",
        rating: "suggestive",
    },
    SourceConfig {
        id: "minotruyen-hentai",
        category: "hentai",
        rating: "adult",
    },
];

struct MinoTruyen;

impl MangaSource for MinoTruyen {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_side_home(SIDE_HOME_FIXTURE, source));
        }
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
            let target = format!(
                "{API_URL}/books?take=24&page={page}&category={}",
                source.category
            );
            return Ok(parse_books(
                &fetch_json(&target, BOOKS_FIXTURE),
                source,
                page,
                24,
            ));
        }
        let target = format!("{API_URL}/books/side-home?category={}", source.category);
        Ok(parse_side_home(
            &fetch_json(&target, SIDE_HOME_FIXTURE),
            source,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(&base_url(&request), query).or_else(|| {
            query
                .strip_prefix("id:")
                .map(|id| format!("/books/{}", id.trim()))
        }) {
            return Ok(Paged {
                entries: vec![details_by_key(source, &request, &key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut target = format!(
            "{API_URL}/books?take=24&page={page}&category={}",
            source.category
        );
        if !query.is_empty() {
            target.push_str("&q=");
            target.push_str(&url::query_escape(query));
        }
        Ok(parse_books(
            &fetch_json(&target, BOOKS_FIXTURE),
            source,
            page,
            24,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/books/1".into());
        Ok(details_by_key(source, &request, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/books/1".into());
        let book_id = key.rsplit('/').next().unwrap_or("1");
        let target = format!("{API_URL}/chapters/{book_id}?order=desc&take=5000");
        Ok(parse_chapters(
            &fetch_json(&target, CHAPTERS_FIXTURE),
            source,
            &base,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let source = source_for(&request);
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/books/1/1".into());
        let chapter_url = format!("{base}/{}{}", source.category, normalize_key(&key));
        let body = client(&base)
            .get(&chapter_url)
            .browser_document()
            .send_text()
            .unwrap_or_else(|_| PAGES_FIXTURE.to_string());
        let pages = parse_pages(&body, &base, &chapter_url);
        if pages.is_empty() {
            return Ok(vec![manga::text_page("Khong giai ma duoc du lieu chuong")]);
        }
        Ok(pages)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let source = source_for(&request);
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(with_listing(&request, source, "popular"))?,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(with_listing(&request, source, "latest"))?,
            ),
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let source = source_for(&request);
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{base}/{}{}", source.category, normalize_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let source = source_for(&request);
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{base}/{}{}", source.category, normalize_key(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let source = source_for(&request);
        let base = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(&base, input) {
            let is_manga = key.split('/').filter(|part| !part.is_empty()).count() == 2;
            return Ok(Some(UrlResolveResult {
                item: is_manga.then(|| details_by_key(source, &request, &key)),
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

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(map) =
            manga_image::page_extra_str(&request, "minoStrips").filter(|value| !value.is_empty())
        else {
            return Ok(manga_image::passthrough_processed_image(&request));
        };
        let Some(input) = manga_image::image_base64(&request) else {
            return Ok(manga_image::passthrough_processed_image(&request));
        };
        let Some(output) = descramble_base64(input, map) else {
            return Ok(manga_image::passthrough_processed_image(&request));
        };
        Ok(ProcessedImage {
            image_base64: output,
            mime_type: Some("image/jpeg".into()),
            ..ProcessedImage::default()
        })
    }
}

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_header("Accept", "application/json, text/plain, */*")
        .with_header("Origin", base)
        .with_referer(format!("{base}/"))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client(DEFAULT_BASE_URL)
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_side_home(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let response: SideHomeResponse = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(SIDE_HOME_FIXTURE).unwrap());
    Paged {
        entries: response
            .top_books_view
            .into_iter()
            .map(|book| book.into_item(source))
            .collect(),
        has_next_page: false,
    }
}

fn parse_books(body: &str, source: SourceConfig, page: u64, take: u64) -> Paged<CatalogItem> {
    let response: BooksResponse =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(BOOKS_FIXTURE).unwrap());
    let count = response.count_book.unwrap_or(0) as u64;
    let entries = response
        .books
        .into_iter()
        .map(|book| book.into_item(source))
        .collect::<Vec<_>>();
    Paged {
        has_next_page: if count > 0 {
            page * take < count
        } else {
            !entries.is_empty()
        },
        entries,
    }
}

fn details_by_key(source: SourceConfig, request: &Value, key: &str) -> CatalogItem {
    let base = base_url(request);
    let book_id = key.rsplit('/').next().unwrap_or("1");
    let body = fetch_json(&format!("{API_URL}/books/{book_id}"), DETAILS_FIXTURE);
    parse_details(&body, source, &base)
}

fn parse_details(body: &str, source: SourceConfig, base: &str) -> CatalogItem {
    let response: BookDetailResponse = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let book = response.book;
    CatalogItem {
        key: format!("/books/{}", book.book_id),
        title: book.title.trim().to_string(),
        cover: resolve_thumbnail(book.covers.first().map(|cover| cover.url.as_str()), base),
        authors: book.author.into_iter().collect(),
        tags: book.tags.into_iter().map(|tag| tag.tag.name).collect(),
        description: book.description,
        status: parse_status(book.status),
        url: Some(format!(
            "{base}/{}{}",
            source.category,
            format!("/books/{}", book.book_id)
        )),
        language: Some("vi".into()),
        content_rating: Some(source.rating.into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, source: SourceConfig, base: &str) -> Vec<MangaChapter> {
    let response: ChaptersResponse = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).unwrap());
    response
        .chapters
        .into_iter()
        .map(|chapter| {
            let chapter_num = format_number(chapter.chapter_number);
            let key = format!("/books/{}/{}", chapter.book_id, chapter_num);
            MangaChapter {
                key: key.clone(),
                title: Some(chapter.num),
                date_uploaded: chapter.created_at.as_deref().and_then(parse_iso_day),
                url: Some(format!("{base}/{}{}", source.category, key)),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str, base: &str, referer: &str) -> Vec<MangaPage> {
    let Some(encrypted) = encrypted_payload(body) else {
        return Vec::new();
    };
    let Some(decrypted) = decrypt_openssl_payload(&encrypted, AES_KEY) else {
        return Vec::new();
    };
    let servers: Vec<ChapterServer> = serde_json::from_str(&decrypted).unwrap_or_default();
    let Some(server) = select_server(&servers, base) else {
        return Vec::new();
    };
    server
        .content
        .iter()
        .enumerate()
        .map(|(index, page_data)| {
            let image = normalize_image(&page_data.image_url, base);
            let strips = page_data.drm_data.as_deref().and_then(decode_drm_map);
            let mut page = page(index, &image, referer);
            if let Some(strips) = strips.filter(|value| !value.is_empty()) {
                page.extra = BTreeMap::from([("minoStrips".into(), Value::String(strips))]);
            }
            page
        })
        .collect()
}

fn select_server<'a>(servers: &'a [ChapterServer], base: &str) -> Option<&'a ChapterServer> {
    let candidates = servers
        .iter()
        .filter(|server| !server.content.is_empty())
        .collect::<Vec<_>>();
    candidates
        .iter()
        .copied()
        .find(|server| {
            server
                .content
                .iter()
                .any(|page| !normalize_image(&page.image_url, base).contains("ibyteimg.com"))
        })
        .or_else(|| candidates.first().copied())
}

fn page(index: usize, image: &str, referer: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image.to_string(),
            context: Some(manga::image_headers(referer)),
        },
        headers: manga::image_headers(referer),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn encrypted_payload(body: &str) -> Option<String> {
    let marker = ":U2FsdGVk";
    let marker_index = body.find(marker)?;
    let start = body[..marker_index]
        .rfind(|ch: char| !ch.is_ascii_hexdigit())
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let tail = &body[start..];
    let token = tail
        .split(|ch: char| {
            !(ch.is_ascii_alphanumeric() || ch == ':' || ch == '+' || ch == '/' || ch == '=')
        })
        .next()?;
    Some(token.split_once(':')?.1.to_string())
}

fn decrypt_openssl_payload(cipher_base64: &str, password: &str) -> Option<String> {
    let data = STANDARD.decode(cipher_base64).ok()?;
    if data.len() < 16 || &data[..8] != b"Salted__" {
        return None;
    }
    let (key, iv) = evp_bytes_to_key(password.as_bytes(), &data[8..16]);
    let mut ciphertext = data[16..].to_vec();
    let decryptor = Aes256CbcDec::new_from_slices(&key, &iv).ok()?;
    let plaintext = decryptor
        .decrypt_padded_mut::<Pkcs7>(&mut ciphertext)
        .ok()?;
    String::from_utf8(plaintext.to_vec()).ok()
}

fn evp_bytes_to_key(password: &[u8], salt: &[u8]) -> ([u8; 32], [u8; 16]) {
    let mut generated = Vec::with_capacity(48);
    let mut previous = Vec::new();
    while generated.len() < 48 {
        let mut input = Vec::with_capacity(previous.len() + password.len() + salt.len());
        input.extend_from_slice(&previous);
        input.extend_from_slice(password);
        input.extend_from_slice(salt);
        previous = md5::compute(input).0.to_vec();
        generated.extend_from_slice(&previous);
    }
    let mut key = [0u8; 32];
    let mut iv = [0u8; 16];
    key.copy_from_slice(&generated[..32]);
    iv.copy_from_slice(&generated[32..48]);
    (key, iv)
}

fn decode_drm_map(drm_data: &str) -> Option<String> {
    let encrypted = STANDARD.decode(drm_data).ok()?;
    let plain = encrypted
        .iter()
        .enumerate()
        .map(|(idx, byte)| byte ^ DRM_XOR_KEY[idx % DRM_XOR_KEY.len()])
        .collect::<Vec<_>>();
    let text = String::from_utf8(plain).ok()?;
    let raw = text.strip_prefix(DRM_MAP_PREFIX)?;
    let entries = raw
        .split('|')
        .filter_map(|token| {
            let (dest, height) = token.split_once('-')?;
            let dest = dest.parse::<u32>().ok()?;
            let height = height.parse::<u32>().ok()?;
            (height > 0).then_some(format!("{dest}-{height}"))
        })
        .collect::<Vec<_>>();
    (!entries.is_empty()).then(|| entries.join(","))
}

fn descramble_base64(input: &str, map: &str) -> Option<String> {
    let bytes = STANDARD.decode(input).ok()?;
    let source = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (width, height) = source.dimensions();
    let mut target = RgbaImage::new(width, height);
    let mut src_y = 0u32;
    for (dest_y, strip_height) in parse_strip_map(map) {
        if src_y >= height || dest_y >= height {
            break;
        }
        let draw_height = strip_height.min(height - src_y).min(height - dest_y);
        if draw_height > 0 {
            let sub = source.view(0, src_y, width, draw_height).to_image();
            target.copy_from(&sub, 0, dest_y).ok()?;
        }
        src_y = src_y.saturating_add(strip_height);
    }
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(target)
        .write_to(&mut out, ImageFormat::Jpeg)
        .ok()?;
    Some(STANDARD.encode(out.into_inner()))
}

fn parse_strip_map(map: &str) -> Vec<(u32, u32)> {
    map.split(',')
        .filter_map(|token| {
            let (dest, height) = token.split_once('-')?;
            let dest = dest.parse().ok()?;
            let height = height.parse().ok()?;
            (height > 0).then_some((dest, height))
        })
        .collect()
}

fn normalize_image(value: &str, base: &str) -> String {
    if value.starts_with("//") {
        format!("https:{value}")
    } else if value.starts_with('/') {
        url::join_url(base, value)
    } else {
        value.to_string()
    }
}

fn resolve_thumbnail(value: Option<&str>, base: &str) -> Option<String> {
    let normalized = normalize_image(value?.trim(), base);
    if !normalized.contains("ibyteimg.com") || normalized.contains("~tplv-") {
        return Some(normalized);
    }
    let rewritten = normalized.replace("-ad-", "-lp-").replace("/obj/", "/");
    Some(format!("{rewritten}~tplv-375lmtcpo0-resize:200:200.webp"))
}

fn parse_status(status: Option<i32>) -> ItemStatus {
    match status {
        Some(1) => ItemStatus::Ongoing,
        Some(2) => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value
            .to_string()
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

fn parse_iso_day(value: &str) -> Option<i64> {
    value.get(..10).and_then(dates::parse_ymd)
}

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("minotruyen-manga");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("overrideBaseUrl"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn normalize_key(value: &str) -> String {
    let raw = format!(
        "/{}",
        value.trim_start_matches(DEFAULT_BASE_URL).trim_matches('/')
    );
    for category in ["manga", "comics", "hentai"] {
        let prefix = format!("/{category}/books/");
        if raw.starts_with(&prefix) {
            return raw.replacen(&format!("/{category}"), "", 1);
        }
    }
    raw
}

fn key_from_url(base: &str, input: &str) -> Option<String> {
    if !input.starts_with(base) {
        return None;
    }
    let key = normalize_key(input.trim_start_matches(base));
    key.contains("/books/").then_some(key)
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.to_string(),
        title: title.to_string(),
        style: Some(HomeSectionStyle::Cover),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

fn with_listing(request: &Value, source: SourceConfig, listing: &str) -> Value {
    json!({
        "sourceId": source.id,
        "page": 1,
        "listingId": listing,
        "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BooksResponse {
    books: Vec<Book>,
    #[serde(default)]
    count_book: Option<i32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SideHomeResponse {
    #[serde(default)]
    top_books_view: Vec<Book>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookDetailResponse {
    book: BookDetail,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChaptersResponse {
    chapters: Vec<Chapter>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Book {
    book_id: i32,
    title: String,
    #[serde(default)]
    status: Option<i32>,
    #[serde(default)]
    covers: Vec<Cover>,
}

impl Book {
    fn into_item(self, source: SourceConfig) -> CatalogItem {
        CatalogItem {
            key: format!("/books/{}", self.book_id),
            title: self.title.trim().to_string(),
            cover: resolve_thumbnail(
                self.covers.first().map(|cover| cover.url.as_str()),
                DEFAULT_BASE_URL,
            ),
            status: parse_status(self.status),
            language: Some("vi".into()),
            content_rating: Some(source.rating.into()),
            url: Some(format!(
                "{DEFAULT_BASE_URL}/{}{}",
                source.category,
                format!("/books/{}", self.book_id)
            )),
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookDetail {
    book_id: i32,
    title: String,
    #[serde(default)]
    status: Option<i32>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    covers: Vec<Cover>,
    #[serde(default)]
    tags: Vec<TagWrapper>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Chapter {
    book_id: i32,
    num: String,
    chapter_number: f64,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Deserialize)]
struct Cover {
    url: String,
}

#[derive(Deserialize)]
struct TagWrapper {
    tag: Tag,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
}

#[derive(Deserialize, Default)]
struct ChapterServer {
    #[serde(default)]
    content: Vec<ChapterPage>,
}

#[derive(Deserialize)]
struct ChapterPage {
    #[serde(rename = "imageUrl")]
    image_url: String,
    #[serde(default, rename = "drm_data")]
    drm_data: Option<String>,
}

const SIDE_HOME_FIXTURE: &str = r#"{"topBooksView":[{"bookId":1,"title":"Sample","status":1,"covers":[{"url":"/cover.jpg"}]}]}"#;
const BOOKS_FIXTURE: &str = r#"{"books":[{"bookId":1,"title":"Sample","status":1,"covers":[{"url":"/cover.jpg"}]}],"countBook":1}"#;
const DETAILS_FIXTURE: &str = r#"{"book":{"bookId":1,"title":"Sample","status":1,"description":"Summary","author":"Author","covers":[{"url":"/cover.jpg"}],"tags":[{"tag":{"tagId":"1","name":"Action"}}]}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"chapters":[{"bookId":1,"num":"Chapter 1","chapterNumber":1.0,"createdAt":"2024-01-01T00:00:00.000Z"}]}"#;
const PAGES_FIXTURE: &str = r#"0123456789abcdef:U2FsdGVkX19UBVOmquQI07hkAKnY2bh0k5+wqd+Md6stNDwhNOdjA8AATXVLvV5CLnJoIFJcaSSpI1WH5doYPDa/ni26BlNRnn/A1BzLLug="#;

export_manga_source!(SOURCE);
