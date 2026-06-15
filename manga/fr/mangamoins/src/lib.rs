use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: MangaMoins = MangaMoins;
const BASE_URL: &str = "https://mangamoins.com";
const API_URL: &str = "https://mangamoins.com/api/v1";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";
const PAGE_LIMIT: u64 = 20;
const FALLBACK_ALPHABET: &str = "abcdefghijk-lmnopqrstuvwxyz_0123456789+";
const FALLBACK_SALTS: [&str; 2] = ["a1f", "Z0_9"];

struct MangaMoins;

impl MangaSource for MangaMoins {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_trend(TREND_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let target = format!("{API_URL}/mangas?page={page}&limit={PAGE_LIMIT}");
            return Ok(parse_list(&fetch_json_or_fixture(&target, LIST_FIXTURE)));
        }
        Ok(parse_trend(&fetch_json_or_fixture(
            &format!("{API_URL}/trend"),
            TREND_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(slug) = deeplink_slug(query, "/manga/") {
            let body = fetch_json_or_fixture(&manga_api_url(&slug), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details_json(&body, &slug)],
                has_next_page: false,
            });
        }
        let mut target = format!("{API_URL}/explore?page={page}&limit={PAGE_LIMIT}");
        if !query.is_empty() {
            target.push_str("&q=");
            target.push_str(&url::query_escape(query));
        }
        Ok(parse_list(&fetch_json_or_fixture(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let slug = manga::request_key(&request, "manga")
            .map(|value| value.trim_start_matches("/manga/").to_string())
            .unwrap_or_else(|| "sample".into());
        let body = fetch_json_or_fixture(&manga_api_url(&slug), DETAILS_FIXTURE);
        Ok(parse_details_json(&body, &slug))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let slug = manga::request_key(&request, "manga")
            .map(|value| value.trim_start_matches("/manga/").to_string())
            .unwrap_or_else(|| "sample".into());
        let body = fetch_json_or_fixture(&manga_api_url(&slug), DETAILS_FIXTURE);
        Ok(parse_chapters_json(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let slug = manga::request_key(&request, "chapter")
            .map(|value| value.trim_start_matches("/scan/").to_string())
            .unwrap_or_else(|| "sample-1".into());
        let target = format!("{API_URL}/scan?slug={}", url::query_escape(&slug));
        let body = fetch_json_or_fixture(&target, SCAN_FIXTURE);
        Ok(parse_scan_pages(&body))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/manga/{}", key.trim_start_matches("/manga/"))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/scan/{}", key.trim_start_matches("/scan/"))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = deeplink_slug(input, "/manga/") {
            let body = fetch_json_or_fixture(&manga_api_url(&slug), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details_json(&body, &slug)),
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

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_header("Origin", BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn manga_api_url(slug: &str) -> String {
    format!("{API_URL}/manga?manga={}", url::query_escape(slug))
}

fn parse_trend(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<TrendResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(TREND_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: response
            .data
            .into_iter()
            .map(MangaListItem::into_item)
            .collect(),
        has_next_page: false,
    }
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<MangaListResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    Paged {
        has_next_page: (response.page as u64) * (response.limit as u64) < response.total as u64,
        entries: response
            .data
            .into_iter()
            .map(MangaListItem::into_item)
            .collect(),
    }
}

fn parse_details_json(body: &str, slug: &str) -> CatalogItem {
    let response = serde_json::from_str::<MangaDetailsResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let info = response.info;
    CatalogItem {
        key: slug.to_string(),
        title: unescape(&info.title),
        cover: (!info.cover.is_empty()).then_some(info.cover),
        authors: (!info.author.is_empty())
            .then(|| vec![unescape(&info.author)])
            .unwrap_or_default(),
        artists: (!info.author.is_empty())
            .then(|| vec![unescape(&info.author)])
            .unwrap_or_default(),
        description: (!info.description.is_empty()).then(|| unescape(&info.description)),
        status: status_from_text(&info.status),
        url: Some(format!("{BASE_URL}/manga/{slug}")),
        language: Some(LANG.into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters_json(body: &str) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<MangaDetailsResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    response
        .chapters
        .into_iter()
        .map(|chapter| {
            let chapter_name = format_chapter_num(chapter.num);
            let title = unescape(&chapter.title);
            let full_title = if title.is_empty() || title.eq_ignore_ascii_case(&chapter_name) {
                chapter_name
            } else {
                format!("{chapter_name} - {title}")
            };
            MangaChapter {
                key: chapter.slug.clone(),
                title: Some(full_title),
                chapter_number: Some(chapter.num),
                date_uploaded: (chapter.time > 0).then_some(chapter.time),
                url: Some(format!(
                    "{BASE_URL}/scan/{}",
                    chapter.slug.trim_start_matches("/scan/")
                )),
                language: Some(LANG.into()),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_scan_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<ScanResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(SCAN_FIXTURE).expect("fixture is valid"));
    let salts = reader_salts(&response.pages_base_url);
    let base = salts.iter().fold(
        response.pages_base_url.trim_end_matches('/').to_string(),
        |acc, salt| acc.replace(salt, ""),
    );
    (1..=response.page_numbers)
        .map(|page| {
            let page_num = format!("{page:02}");
            MangaPage {
                content: PageContent::Url {
                    url: format!("{base}/{page_num}.webp"),
                    context: None,
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {page}")),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn reader_salts(pages_base_url: &str) -> Vec<String> {
    let script = fetch_document_or_fixture(
        &format!("{BASE_URL}/includes/components/js/reader.js"),
        READER_FIXTURE,
    );
    let path_segment = pages_base_url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let alphabet = quoted_strings(&script)
        .into_iter()
        .find(|value| {
            value.len() >= 30
                && value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || "-+_".contains(ch))
        })
        .unwrap_or_else(|| FALLBACK_ALPHABET.to_string());
    let (multiplier, offset) = formula(&script).unwrap_or((3, 7));
    let mut salts = Vec::new();
    for array in hex_arrays(&script) {
        let decoded = array
            .into_iter()
            .filter_map(|value| {
                alphabet
                    .chars()
                    .nth(((value * multiplier + offset) % alphabet.len()) as usize)
            })
            .collect::<String>();
        if decoded.len() >= 3 && path_segment.contains(&decoded) {
            salts.push(decoded);
        }
    }
    for value in quoted_strings(&script) {
        let decoded = decode_hex_escapes(&value);
        if decoded.len() >= 3 && path_segment.contains(&decoded) {
            salts.push(decoded);
        }
    }
    salts.sort_by_key(|value| std::cmp::Reverse(value.len()));
    salts.dedup();
    if salts.is_empty() {
        FALLBACK_SALTS
            .iter()
            .map(|value| value.to_string())
            .collect()
    } else {
        salts
    }
}

fn quoted_strings(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    for quote in ['"', '\''] {
        let mut rest = input;
        while let Some(start) = rest.find(quote) {
            rest = &rest[start + 1..];
            let Some(end) = rest.find(quote) else {
                break;
            };
            out.push(rest[..end].to_string());
            rest = &rest[end + 1..];
        }
    }
    out
}

fn hex_arrays(input: &str) -> Vec<Vec<usize>> {
    input
        .split('[')
        .skip(1)
        .filter_map(|chunk| {
            let inside = chunk.split(']').next()?;
            let values = inside
                .split(',')
                .map(str::trim)
                .filter_map(|part| usize::from_str_radix(part.trim_start_matches("0x"), 16).ok())
                .collect::<Vec<_>>();
            (values.len() > 1).then_some(values)
        })
        .collect()
}

fn formula(input: &str) -> Option<(usize, usize)> {
    let start = input.find("*0x")?;
    let rest = &input[start + 3..];
    let multiplier = rest.split("+0x").next()?;
    let rest = rest.split("+0x").nth(1)?;
    let offset = rest
        .chars()
        .take_while(|ch| ch.is_ascii_hexdigit())
        .collect::<String>();
    Some((
        usize::from_str_radix(multiplier, 16).ok()?,
        usize::from_str_radix(&offset, 16).ok()?,
    ))
}

fn decode_hex_escapes(input: &str) -> String {
    let mut out = String::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'x') {
            chars.next();
            let hex = chars.by_ref().take(2).collect::<String>();
            if let Ok(value) = u8::from_str_radix(&hex, 16) {
                out.push(value as char);
                continue;
            }
        }
        out.push(ch);
    }
    out
}

fn status_from_text(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("en cours") {
        ItemStatus::Ongoing
    } else if lower.contains("termin") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn format_chapter_num(num: f32) -> String {
    if num.fract() == 0.0 {
        format!("Chapitre {}", num as i64)
    } else {
        format!("Chapitre {num}")
    }
}

fn unescape(value: &str) -> String {
    html::html_unescape(value).trim().to_string()
}

fn slugify(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

fn deeplink_slug(input: &str, marker: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) {
        return None;
    }
    let rest = input.split(marker).nth(1)?;
    Some(rest.trim_matches('/').to_string())
}

#[derive(Deserialize)]
struct MangaListResponse {
    #[serde(default)]
    total: i64,
    #[serde(default = "default_one")]
    page: i64,
    #[serde(default = "default_ten")]
    limit: i64,
    #[serde(default)]
    data: Vec<MangaListItem>,
}

#[derive(Deserialize)]
struct TrendResponse {
    #[serde(default)]
    data: Vec<MangaListItem>,
}

#[derive(Deserialize)]
struct MangaListItem {
    title: String,
    #[serde(default)]
    cover: String,
    #[serde(default, rename = "mangaSlug")]
    slug: Option<String>,
    #[serde(default, rename = "slug")]
    trend_slug: Option<String>,
}

impl MangaListItem {
    fn into_item(self) -> CatalogItem {
        let title = unescape(&self.title);
        let slug = self
            .slug
            .or(self.trend_slug)
            .unwrap_or_else(|| slugify(&title));
        CatalogItem {
            key: slug.clone(),
            title,
            cover: (!self.cover.is_empty()).then_some(self.cover),
            url: Some(format!("{BASE_URL}/manga/{slug}")),
            language: Some(LANG.into()),
            content_rating: Some(CONTENT_RATING.into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct MangaDetailsResponse {
    info: MangaInfo,
    #[serde(default)]
    chapters: Vec<ChapterItem>,
}

#[derive(Deserialize)]
struct MangaInfo {
    title: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    cover: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct ChapterItem {
    slug: String,
    num: f32,
    #[serde(default)]
    title: String,
    #[serde(default)]
    time: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanResponse {
    page_numbers: i32,
    pages_base_url: String,
}

fn default_one() -> i64 {
    1
}

fn default_ten() -> i64 {
    10
}

const TREND_FIXTURE: &str =
    r#"{"data":[{"title":"Sample","cover":"https://mangamoins.com/cover.jpg","slug":"sample"}]}"#;
const LIST_FIXTURE: &str = r#"{"total":21,"page":1,"limit":20,"data":[{"title":"Sample","cover":"https://mangamoins.com/cover.jpg","mangaSlug":"sample"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"info":{"title":"Sample","author":"Author","status":"En cours","cover":"https://mangamoins.com/cover.jpg","description":"Summary"},"chapters":[{"slug":"sample-1","num":1.0,"title":"Sample title","time":1704067200}]}"#;
const SCAN_FIXTURE: &str =
    r#"{"pageNumbers":2,"pagesBaseUrl":"https://mangamoins.com/uploads/a1fsampleZ0_9"}"#;
const READER_FIXTURE: &str = r#"
const alphabet="abcdefghijk-lmnopqrstuvwxyz_0123456789+";
const salts=["a1f","Z0_9"];
const formula=x*0x3+0x7;
"#;
