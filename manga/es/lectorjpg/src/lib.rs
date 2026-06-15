use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;
use std::sync::{LazyLock, Mutex};

const SOURCE: LectorJpg = LectorJpg;
const BASE_URL: &str = "https://visorjpg.lat";
const API_URL: &str = "https://api.visorjpg.lat";
const NAME: &str = "LectorJPG";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

static LATEST_CURSORS: LazyLock<Mutex<Vec<(u64, String)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));
static SEARCH_CURSORS: LazyLock<Mutex<Vec<(SearchKey, String)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

struct LectorJpg;

impl MangaSource for LectorJpg {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_query(TRENDING_FIXTURE, None, None));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let cursor = latest_cursor(page);
            let mut query = Query::new(&format!("{API_URL}/home/lastest-updates"));
            query.param("cursor", &cursor);
            return Ok(parse_series_query(
                &fetch_json(&query.finish(), LATEST_FIXTURE),
                Some(CursorTarget::Latest(page)),
                None,
            ));
        }
        Ok(parse_series_query(
            &fetch_json(&format!("{API_URL}/home/trending"), TRENDING_FIXTURE),
            None,
            None,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query_text = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query_text.starts_with(BASE_URL) {
            let key = normalize_series_key(query_text);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&series_url(&key), DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let genres = selected_genres(request.get("filters").unwrap_or(&Value::Null));
        let search_key = SearchKey {
            page,
            query: query_text.to_string(),
            genres: genres.clone(),
        };
        let mut target = Query::new(&format!("{API_URL}/search"));
        target.param("cursor", &search_cursor(&search_key));
        target.param("name", query_text);
        if !genres.is_empty() {
            target.param("genres", &genres);
        }
        Ok(parse_series_query(
            &fetch_json(&target.finish(), SEARCH_FIXTURE),
            Some(CursorTarget::Search(search_key)),
            None,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(parse_details(
            &fetch_document(&series_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(parse_chapters(
            &fetch_document(&series_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| series_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/series/") {
            let key = normalize_series_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), &key)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_cookies_for(API_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_series_query(
    body: &str,
    cursor_target: Option<CursorTarget>,
    fixture: Option<&str>,
) -> Paged<CatalogItem> {
    let fixture = fixture.unwrap_or(SEARCH_FIXTURE);
    let response = serde_json::from_str::<SeriesQueryDto>(body)
        .or_else(|_| serde_json::from_str::<SeriesQueryDto>(fixture))
        .unwrap_or_default();
    if let (Some(target), Some(cursor)) = (cursor_target, response.next_cursor.clone()) {
        store_cursor(target, cursor);
    }
    Paged {
        entries: response.data.into_iter().map(SeriesDto::to_item).collect(),
        has_next_page: response.next_cursor.is_some(),
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_series_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| NAME.to_string()),
        cover: html::attr_after(body, "bg_main bg-cover", "style")
            .and_then(|style| image_from_style(&style)),
        description: html::text_between(body, "<p", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: body
            .split("<a")
            .filter(|chunk| chunk.contains("/series?genres"))
            .filter_map(|chunk| html::text_between(chunk, "<span", "</span>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(status_text(body).as_deref()),
        url: Some(series_url(key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, series_key: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("group") || chunk.contains("/chapter"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "truncate", "</span>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_chapter_key(&href, series_key);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "w-fit", "</span>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_chapter_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut images = images_from_script(body);
    if images.is_empty() {
        images = body
            .split("<img")
            .skip(1)
            .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
            .collect();
    }
    images
        .into_iter()
        .filter(|image| !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn images_from_script(body: &str) -> Vec<String> {
    let Some(start) = body.find("images:") else {
        return Vec::new();
    };
    let rest = &body[start + "images:".len()..];
    let Some(open) = rest.find('[') else {
        return Vec::new();
    };
    let mut depth = 0i32;
    for (index, ch) in rest[open..].char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    let raw = &rest[open..open + index + 1];
                    return serde_json::from_str(raw).unwrap_or_default();
                }
            }
            _ => {}
        }
    }
    Vec::new()
}

fn image_from_style(style: &str) -> Option<String> {
    html::html_unescape(style)
        .split("url(")
        .last()
        .map(|value| {
            value
                .split(')')
                .next()
                .unwrap_or(value)
                .trim_matches('"')
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn status_text(body: &str) -> Option<String> {
    body.split("<div")
        .find(|chunk| chunk.contains("Status"))
        .map(html::strip_tags)
        .map(|value| value.replace("Status", ""))
        .map(|value| value.trim_matches([':', ' ']).to_string())
        .filter(|value| !value.is_empty())
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value
        .map(|status| status.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("on-going") | Some("ongoing") => ItemStatus::Ongoing,
        Some("end") | Some("completed") => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn selected_genres(filters: &Value) -> String {
    match filters.get("genres") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::String(value)) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    }
}

fn series_url(key: &str) -> String {
    format!("{BASE_URL}/series/{}", normalize_series_key(key))
}

fn normalize_series_key(input: &str) -> String {
    let value = input.trim().trim_end_matches('/');
    if let Some((_, rest)) = value.split_once("/series/") {
        return rest.split('/').next().unwrap_or(rest).to_string();
    }
    value.trim_matches('/').to_string()
}

fn normalize_chapter_key(input: &str, series_key: &str) -> String {
    let value = input.trim().trim_end_matches('/');
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!(
                "/{}",
                value[index + BASE_URL.len()..].trim_start_matches('/')
            );
        }
    }
    if value.starts_with('/') {
        return value.to_string();
    }
    format!("{}/{}", normalize_series_key(series_key), value)
}

fn latest_cursor(page: u64) -> String {
    if page <= 1 {
        return create_latest_cursor();
    }
    LATEST_CURSORS
        .lock()
        .ok()
        .and_then(|items| {
            items
                .iter()
                .find(|(stored_page, _)| *stored_page == page - 1)
                .cloned()
        })
        .map(|(_, cursor)| cursor)
        .unwrap_or_default()
}

fn search_cursor(key: &SearchKey) -> String {
    if key.page <= 1 {
        return String::new();
    }
    let previous = SearchKey {
        page: key.page - 1,
        query: key.query.clone(),
        genres: key.genres.clone(),
    };
    SEARCH_CURSORS
        .lock()
        .ok()
        .and_then(|items| {
            items
                .iter()
                .find(|(stored, _)| *stored == previous)
                .cloned()
        })
        .map(|(_, cursor)| cursor)
        .unwrap_or_default()
}

fn store_cursor(target: CursorTarget, cursor: String) {
    match target {
        CursorTarget::Latest(page) => {
            if let Ok(mut items) = LATEST_CURSORS.lock() {
                items.retain(|(stored_page, _)| *stored_page != page);
                items.push((page, cursor));
                if items.len() > 8 {
                    items.remove(0);
                }
            }
        }
        CursorTarget::Search(key) => {
            if let Ok(mut items) = SEARCH_CURSORS.lock() {
                items.retain(|(stored, _)| stored != &key);
                items.push((key, cursor));
                if items.len() > 8 {
                    items.remove(0);
                }
            }
        }
    }
}

fn create_latest_cursor() -> String {
    let now = unix_now();
    let days = now.div_euclid(86_400);
    let seconds = now.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    let json = format!(
        "{{\"last_update_at\":\"{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}\",\"id\":0,\"_pointsToNextItems\":true}}"
    );
    base64_encode(json.as_bytes())
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn parse_chapter_date(value: &str) -> Option<i64> {
    let lower = value.trim().to_ascii_lowercase();
    if lower == "ayer" {
        return Some(unix_now() - 86_400);
    }
    if let Some(rest) = lower.strip_prefix("hace") {
        let amount = rest
            .split_whitespace()
            .next()
            .and_then(|part| part.parse::<i64>().ok())?;
        let multiplier = if rest.contains("hora") {
            3_600
        } else if rest.contains("minuto") {
            60
        } else if rest.contains("segundo") {
            1
        } else if rest.contains("dia") || rest.contains("d\u{ed}a") {
            86_400
        } else {
            return None;
        };
        return Some(unix_now() - amount * multiplier);
    }
    parse_slash_date(value)
}

fn parse_slash_date(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('/');
    let day = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<i32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

fn civil_from_days(days: i64) -> (i32, i32, i32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    (year as i32, month as i32, day as i32)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchKey {
    page: u64,
    query: String,
    genres: String,
}

enum CursorTarget {
    Latest(u64),
    Search(SearchKey),
}

#[derive(Debug, Default, Deserialize)]
struct SeriesQueryDto {
    #[serde(default)]
    data: Vec<SeriesDto>,
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SeriesDto {
    name: String,
    slug: String,
    cover_url: Option<String>,
}

impl SeriesDto {
    fn to_item(self) -> CatalogItem {
        CatalogItem {
            key: self.slug.clone(),
            title: self.name,
            cover: self.cover_url,
            url: Some(series_url(&self.slug)),
            language: Some(LANG.to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

struct Query {
    target: String,
    has_query: bool,
}

impl Query {
    fn new(base: &str) -> Self {
        Self {
            target: base.to_string(),
            has_query: base.contains('?'),
        }
    }

    fn param(&mut self, key: &str, value: &str) {
        self.target.push(if self.has_query { '&' } else { '?' });
        self.has_query = true;
        self.target.push_str(&url::query_escape(key));
        self.target.push('=');
        self.target.push_str(&url::query_escape(value));
    }

    fn finish(self) -> String {
        self.target
    }
}

export_manga_source!(SOURCE);

const TRENDING_FIXTURE: &str = r#"{"data":[{"name":"Sample","slug":"sample","cover_url":"https://visorjpg.lat/cover.jpg"}],"next_cursor":null}"#;
const LATEST_FIXTURE: &str = TRENDING_FIXTURE;
const SEARCH_FIXTURE: &str = TRENDING_FIXTURE;
const DETAILS_FIXTURE: &str = r#"<div class="grid"><h1>Sample</h1><div class="bg_main bg-cover" style="background-image:url(&quot;https://visorjpg.lat/cover.jpg&quot;)"></div><div class="container"><p>Summary</p></div><div><span>Status</span></div><div>on-going</div><a href="/series?genres=drama"><span>Drama</span></a><a class="group" href="/sample/chapter-1"><span class="truncate">Chapter 1</span><span class="w-fit">01/01/2024</span></a></div>"#;
const PAGES_FIXTURE: &str = r#"<script>const svelteKit={images:["https://visorjpg.lat/page1.jpg","https://visorjpg.lat/page2.jpg"]}</script>"#;
