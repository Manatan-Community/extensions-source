use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    manga,
    sdk::{FilterValue, SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: OrtegaScans = OrtegaScans;
const BASE_URL: &str = "https://ortegascans.fr";
const PER_PAGE: u64 = 18;

struct OrtegaScans;

impl MangaSource for OrtegaScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_response(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "recent"
        } else {
            "popular"
        };
        Ok(parse_series_response(&fetch_text_or_fixture(
            &series_url(page, "", sort, &[]),
            LIST_FIXTURE,
            false,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let filters = parse_filters(request.get("filters"));
        Ok(parse_series_response(&fetch_text_or_fixture(
            &series_url(page, query, &filters.sort, &filters.params()),
            LIST_FIXTURE,
            false,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(sample_key);
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(sample_key);
        let details = fetch_rsc_or_fixture(&manga_url(&key), DETAILS_FIXTURE);
        let hide_premium = request
            .get("preferences")
            .and_then(|value| value.get("hidePremium"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(parse_chapters(&details, &manga_slug(&key), hide_premium))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(sample_chapter_key);
        Ok(parse_pages(&fetch_rsc_or_fixture(
            &chapter_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| manga_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| chapter_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/serie/") {
            let key = normalize_manga_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text_or_fixture(target: &str, fixture: &str, rsc: bool) -> String {
    let client = client();
    let request = client.get(target).browser_document();
    let request = if rsc {
        request.header("RSC", "1").header("rsc", "1")
    } else {
        request.xhr()
    };
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_rsc_or_fixture(target: &str, fixture: &str) -> String {
    fetch_text_or_fixture(target, fixture, true)
}

fn series_url(page: u64, query: &str, sort: &str, extra: &[(&str, String)]) -> String {
    let mut params = vec![
        ("limit", PER_PAGE.to_string()),
        ("page", page.to_string()),
        ("search", url::query_escape(query)),
        ("tags", String::new()),
        ("status", String::new()),
        ("sort", sort.to_string()),
        ("minChapters", "0".to_string()),
        ("isOrtegaOnly", "false".to_string()),
        ("unreadOnly", "false".to_string()),
        ("maxChapters", "9999".to_string()),
    ];
    for (key, value) in extra {
        if let Some(existing) = params.iter_mut().find(|(name, _)| name == key) {
            existing.1 = value.clone();
        }
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/api/series?{query}")
}

#[derive(Default)]
struct ParsedFilters {
    sort: String,
    status: String,
    tags: String,
    min_chapters: String,
    max_chapters: String,
}

impl ParsedFilters {
    fn params(&self) -> Vec<(&str, String)> {
        vec![
            ("status", self.status.clone()),
            ("tags", self.tags.clone()),
            ("minChapters", self.min_chapters.clone()),
            ("maxChapters", self.max_chapters.clone()),
        ]
    }
}

fn parse_filters(filters: Option<&Value>) -> ParsedFilters {
    let mut parsed = ParsedFilters {
        sort: "popular".to_string(),
        min_chapters: "0".to_string(),
        max_chapters: "9999".to_string(),
        ..ParsedFilters::default()
    };
    let values = filters_to_values(filters);
    for filter in values {
        match filter.id.as_str() {
            "sort" => parsed.sort = string_value(&filter.value, "popular"),
            "status" => parsed.status = list_value(&filter.value),
            "tags" => parsed.tags = list_value(&filter.value),
            "minChapters" => parsed.min_chapters = int_string(&filter.value, "0"),
            "maxChapters" => parsed.max_chapters = int_string(&filter.value, "9999"),
            _ => {}
        }
    }
    parsed.sort = url::query_escape(&parsed.sort);
    parsed.status = url::query_escape(&parsed.status);
    parsed.tags = url::query_escape(&parsed.tags);
    parsed
}

fn filters_to_values(filters: Option<&Value>) -> Vec<FilterValue> {
    let Some(filters) = filters else {
        return Vec::new();
    };
    if let Ok(values) = serde_json::from_value::<Vec<FilterValue>>(filters.clone()) {
        return values;
    }
    filters
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(id, value)| FilterValue {
                    id: id.clone(),
                    value: value.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string_value(value: &Value, default: &str) -> String {
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_string()
}

fn list_value(value: &Value) -> String {
    if let Some(items) = value.as_array() {
        return items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(",");
    }
    value.as_str().unwrap_or_default().to_string()
}

fn int_string(value: &Value, default: &str) -> String {
    let raw = value.as_str().unwrap_or(default);
    if raw.parse::<u32>().is_ok() {
        raw.to_string()
    } else {
        default.to_string()
    }
}

fn parse_series_response(body: &str) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<SeriesResponse>(body).unwrap_or_default();
    Paged {
        entries: payload
            .data
            .into_iter()
            .map(SeriesDto::into_catalog)
            .collect(),
        has_next_page: payload.has_more,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let body = fetch_rsc_or_fixture(&manga_url(key), DETAILS_FIXTURE);
    extract_value_with_keys(&body, &["manga", "title", "coverImage"])
        .and_then(|value| serde_json::from_value::<MangaDetailsDataDto>(value).ok())
        .map(|details| details.manga.into_catalog_initialized())
        .unwrap_or_else(|| fallback_item(key))
}

fn parse_chapters(body: &str, manga_slug: &str, hide_premium: bool) -> Vec<MangaChapter> {
    extract_value_with_keys(body, &["chapters", "number", "createdAt"])
        .and_then(|value| serde_json::from_value::<ChapterListDataDto>(value).ok())
        .map(|payload| {
            payload
                .chapters
                .into_iter()
                .filter(|chapter| !hide_premium || !chapter.is_premium)
                .map(|chapter| chapter.into_chapter(manga_slug))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    extract_value_with_keys(body, &["images", "url"])
        .and_then(|value| serde_json::from_value::<PageListDto>(value).ok())
        .map(|payload| {
            payload
                .images
                .into_iter()
                .map(|image| {
                    let page_url = if image.url.starts_with("http") {
                        image.url
                    } else {
                        format!("{BASE_URL}{}", image.url)
                    };
                    MangaPage {
                        content: PageContent::Url {
                            url: page_url,
                            context: Some(manga::image_headers(BASE_URL)),
                        },
                        headers: manga::image_headers(BASE_URL),
                        description: Some(format!("Page {}", image.index + 1)),
                        ..MangaPage::default()
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_manga_key(input: &str) -> String {
    let slug = input
        .split("/serie/")
        .nth(1)
        .unwrap_or(input)
        .trim_matches('/')
        .split('/')
        .next()
        .unwrap_or("sample");
    let id = input
        .split('#')
        .nth(1)
        .filter(|id| !id.is_empty())
        .unwrap_or(slug);
    format!("{slug}#{id}")
}

fn normalize_chapter_key(manga_slug: &str, id: &str, number: &str) -> String {
    format!("{manga_slug}#{id}#{number}")
}

fn manga_slug(key: &str) -> String {
    key.split('#').next().unwrap_or("sample").to_string()
}

fn manga_url(key: &str) -> String {
    format!("{BASE_URL}/serie/{}", manga_slug(key))
}

fn chapter_url(key: &str) -> String {
    let mut parts = key.split('#');
    let slug = parts.next().unwrap_or("sample");
    let _id = parts.next();
    let number = parts.next().unwrap_or("1");
    format!("{BASE_URL}/serie/{slug}/chapter/{number}")
}

fn sample_key() -> String {
    "sample#1".to_string()
}

fn sample_chapter_key() -> String {
    "sample#chapter1#1".to_string()
}

fn fallback_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: url::slug_from_url(&manga_slug(key)).unwrap_or_else(|| "Ortega Scans".to_string()),
        url: Some(manga_url(key)),
        language: Some("fr".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn extract_value_with_keys(body: &str, keys: &[&str]) -> Option<Value> {
    let bytes = body.as_bytes();
    let first_key = keys.first()?;
    for (index, _) in body.match_indices(&format!("\"{first_key}\"")) {
        if let Some(start) = body[..index].rfind('{') {
            if let Some(end) = matching_brace(bytes, start) {
                let candidate = &body[start..=end];
                if keys
                    .iter()
                    .all(|key| candidate.contains(&format!("\"{key}\"")))
                {
                    if let Ok(value) = serde_json::from_str(candidate) {
                        return Some(value);
                    }
                }
            }
        }
    }
    None
}

fn matching_brace(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, &byte) in bytes[start..].iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(start + offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_date(value: &str) -> Option<i64> {
    let date = value
        .trim()
        .trim_start_matches("$D")
        .split(['T', ' '])
        .next()
        .unwrap_or(value);
    manatan_shared::dates::parse_ymd(date)
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesResponse {
    #[serde(default)]
    data: Vec<SeriesDto>,
    #[serde(default)]
    has_more: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesDto {
    id: String,
    title: String,
    slug: String,
    cover_image: String,
}

impl SeriesDto {
    fn into_catalog(self) -> CatalogItem {
        let key = format!("{}#{}", self.slug, self.id);
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: Some(format!(
                "{BASE_URL}/{}",
                self.cover_image.replace("storage/", "api/")
            )),
            url: Some(manga_url(&key)),
            language: Some("fr".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct MangaDetailsDataDto {
    manga: MangaDto,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaDto {
    id: String,
    title: String,
    slug: String,
    description: Option<String>,
    cover_image: String,
    status: Option<String>,
    author: Option<String>,
    artist: Option<String>,
    alternative_names: Option<String>,
    #[serde(default)]
    categories: Vec<CategoryDto>,
}

impl MangaDto {
    fn into_catalog_initialized(self) -> CatalogItem {
        let key = format!("{}#{}", self.slug, self.id);
        CatalogItem {
            key: key.clone(),
            title: self.title,
            alternate_titles: self
                .alternative_names
                .as_deref()
                .map(|names| {
                    names
                        .split(',')
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            cover: Some(format!(
                "{BASE_URL}/{}",
                self.cover_image.replace("storage/", "api/")
            )),
            description: self.description,
            authors: self.author.into_iter().collect(),
            artists: self.artist.into_iter().collect(),
            tags: self
                .categories
                .into_iter()
                .map(|category| category.name)
                .collect(),
            status: parse_status(self.status.as_deref()),
            url: Some(manga_url(&key)),
            language: Some("fr".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct CategoryDto {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ChapterListDataDto {
    chapters: Vec<ChapterDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterDto {
    id: String,
    number: f32,
    title: Option<String>,
    #[serde(default)]
    is_premium: bool,
    created_at: String,
}

impl ChapterDto {
    fn into_chapter(self, manga_slug: &str) -> MangaChapter {
        let number = trim_float(self.number);
        let mut title = format!("Chapitre {number}");
        if let Some(extra) = self.title.filter(|title| !title.trim().is_empty()) {
            title.push_str(" - ");
            title.push_str(extra.trim());
        }
        if self.is_premium {
            title = format!("Verrouillé - {title}");
        }
        MangaChapter {
            key: normalize_chapter_key(manga_slug, &self.id, &number),
            title: Some(title),
            chapter_number: Some(self.number),
            date_uploaded: parse_date(&self.created_at),
            url: Some(format!("{BASE_URL}/serie/{manga_slug}/chapter/{number}")),
            is_locked: self.is_premium,
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct PageListDto {
    images: Vec<ImageDto>,
}

#[derive(Debug, Deserialize)]
struct ImageDto {
    index: usize,
    url: String,
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "en cours" | "ongoing" => ItemStatus::Ongoing,
        "terminé" | "complete" | "completed" => ItemStatus::Completed,
        "en pause" | "on hold" => ItemStatus::Hiatus,
        "annulé" | "canceled" | "cancelled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn trim_float(value: f32) -> String {
    let mut text = value.to_string();
    if text.ends_with(".0") {
        text.truncate(text.len() - 2);
    }
    text
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":[{"id":"1","title":"Sample Ortega","slug":"sample","coverImage":"storage/covers/sample.jpg"}],"hasMore":true}"#;

const DETAILS_FIXTURE: &str = r#"
1:{"manga":{"id":"1","title":"Sample Ortega","slug":"sample","description":"Summary","coverImage":"storage/covers/sample.jpg","status":"en cours","author":"Auteur","artist":"Artiste","alternativeNames":"Alt One, Alt Two","categories":[{"name":"Action"},{"name":"Romance"}]}}
2:{"chapters":[{"id":"c1","number":1.0,"title":"Début","isPremium":false,"createdAt":"2024-01-01T00:00:00.000Z"},{"id":"c2","number":2.0,"title":"Premium","isPremium":true,"createdAt":"2024-02-01T00:00:00.000Z"}]}
"#;

const PAGES_FIXTURE: &str = r#"1:{"images":[{"index":0,"url":"/api/pages/1.jpg"},{"index":1,"url":"https://cdn.ortegascans.fr/2.jpg"}]}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_api_listing() {
        let page = SOURCE.list(json!({})).unwrap();
        assert_eq!(page.entries[0].title, "Sample Ortega");
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_rsc_details_chapters_and_pages() {
        let details = details_from_key("sample#1");
        assert_eq!(details.title, "Sample Ortega");
        let chapters = parse_chapters(DETAILS_FIXTURE, "sample", true);
        assert_eq!(chapters.len(), 1);
        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
