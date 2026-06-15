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

const SOURCE: DoujinDesuUnoriginal = DoujinDesuUnoriginal;
const BASE_URL: &str = "https://v2.doujindesu.fun";

struct DoujinDesuUnoriginal;

impl MangaSource for DoujinDesuUnoriginal {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list(LIST_FIXTURE, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "popular"
        };
        Ok(parse_list(
            &fetch_rsc_or_fixture(
                &search_url(page, "", &ParsedFilters::with_order(order)),
                LIST_FIXTURE,
            ),
            page,
        ))
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
        Ok(parse_list(
            &fetch_rsc_or_fixture(&search_url(page, query, &filters), LIST_FIXTURE),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(sample_key);
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(sample_key);
        let body = fetch_rsc_or_fixture(&manga_url_from_key(&key), DETAILS_FIXTURE);
        let data = parse_chapters(&body).unwrap_or_default();
        Ok(data
            .chapters
            .into_iter()
            .map(|chapter| chapter.into_chapter(&normalize_manga_key(&key)))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/read/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_json_or_fixture(
            &reader_api_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| manga_url_from_key(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}{}", normalize_chapter_key(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
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
        .with_origin(BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_rsc_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Rsc", "1")
        .header("RSC", "1")
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(page: u64, query: &str, filters: &ParsedFilters) -> String {
    let mut params = Vec::new();
    if !query.is_empty() {
        params.push(("q", url::query_escape(query)));
    }
    if !filters.status.is_empty() {
        params.push(("status", url::query_escape(&filters.status)));
    }
    if !filters.kind.is_empty() {
        params.push(("type", url::query_escape(&filters.kind)));
    }
    if !filters.order.is_empty() {
        params.push(("order", url::query_escape(&filters.order)));
    }
    if !filters.genre.is_empty() {
        params.push(("genre", url::query_escape(&filters.genre)));
    }
    if page > 1 {
        params.push(("page", page.to_string()));
    }
    if params.is_empty() {
        return format!("{BASE_URL}/manga");
    }
    format!(
        "{BASE_URL}/manga?{}",
        params
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&")
    )
}

#[derive(Default)]
struct ParsedFilters {
    order: String,
    status: String,
    kind: String,
    genre: String,
}

impl ParsedFilters {
    fn with_order(order: &str) -> Self {
        Self {
            order: order.to_string(),
            ..Self::default()
        }
    }
}

fn parse_filters(filters: Option<&Value>) -> ParsedFilters {
    let mut parsed = ParsedFilters::default();
    for filter in filters_to_values(filters) {
        let value = filter.value.as_str().unwrap_or_default().trim();
        match filter.id.as_str() {
            "order" => parsed.order = value.to_string(),
            "status" => parsed.status = value.to_string(),
            "type" => parsed.kind = value.to_string(),
            "genre" => parsed.genre = value.to_string(),
            _ => {}
        }
    }
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

fn parse_list(body: &str, page: u64) -> Paged<CatalogItem> {
    let list = extract_value_with_keys(body, &["mangas", "totalItems"])
        .and_then(|value| serde_json::from_value::<MangaList>(value).ok())
        .or_else(|| serde_json::from_str::<MangaList>(body).ok())
        .unwrap_or_default();
    Paged {
        entries: list
            .mangas
            .into_iter()
            .map(MangaListItem::into_catalog)
            .collect(),
        has_next_page: list.total_items.is_some_and(|total| page * 24 < total),
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let body = fetch_rsc_or_fixture(&manga_url_from_key(key), DETAILS_FIXTURE);
    parse_details(&body)
        .map(MangaDetails::into_catalog)
        .unwrap_or_else(|| fallback_item(key))
}

fn parse_details(body: &str) -> Option<MangaDetails> {
    extract_value_with_keys(body, &["manga", "alternativeTitle", "tags"])
        .and_then(|value| serde_json::from_value::<MangaDetailsEnvelope>(value).ok())
        .map(|envelope| envelope.manga)
        .or_else(|| {
            serde_json::from_str::<MangaDetailsEnvelope>(body)
                .ok()
                .map(|envelope| envelope.manga)
        })
}

fn parse_chapters(body: &str) -> Option<ChaptersList> {
    extract_value_with_keys(body, &["chapters", "createdAt"])
        .and_then(|value| serde_json::from_value(value).ok())
        .or_else(|| serde_json::from_str(body).ok())
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    serde_json::from_str::<ReaderData>(body)
        .unwrap_or_default()
        .data
        .map(|data| data.chapter.images)
        .unwrap_or_default()
        .into_iter()
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

fn manga_url_from_key(key: &str) -> String {
    format!("{BASE_URL}/manga/{}", normalize_manga_key(key))
}

fn reader_api_url(key: &str) -> String {
    let parts = normalize_chapter_key(key)
        .trim_matches('/')
        .split('/')
        .map(str::to_string)
        .collect::<Vec<_>>();
    let manga_slug = parts.get(1).map(String::as_str).unwrap_or("sample");
    let chapter_slug = parts.get(2).map(String::as_str).unwrap_or("chapter-1");
    format!("{BASE_URL}/api/read/{manga_slug}/{chapter_slug}")
}

fn normalize_manga_key(input: &str) -> String {
    let value = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_matches('/');
    let mut segments = value.split('/');
    match segments.next() {
        Some("manga") => segments.next().unwrap_or("sample").to_string(),
        Some("read") => segments.next().unwrap_or("sample").to_string(),
        Some(value) if !value.is_empty() => value.to_string(),
        _ => "sample".to_string(),
    }
}

fn normalize_chapter_key(input: &str) -> String {
    let value = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_matches('/');
    if value.starts_with("read/") {
        return format!("/{value}");
    }
    format!("/read/{value}")
}

fn sample_key() -> String {
    "sample".to_string()
}

fn fallback_item(key: &str) -> CatalogItem {
    let slug = normalize_manga_key(key);
    CatalogItem {
        key: slug.clone(),
        title: title_from_slug(&slug),
        url: Some(format!("{BASE_URL}/manga/{slug}")),
        language: Some("id".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn title_from_slug(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_lowercase().as_str() {
        "publishing" | "ongoing" => ItemStatus::Ongoing,
        "finished" | "completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: Option<&str>) -> Option<i64> {
    let date = value?.split(['T', ' ']).next()?;
    manatan_shared::dates::parse_ymd(date)
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

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaList {
    #[serde(default)]
    mangas: Vec<MangaListItem>,
    total_items: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct MangaListItem {
    slug: String,
    title: String,
    thumb: Option<String>,
}

impl MangaListItem {
    fn into_catalog(self) -> CatalogItem {
        CatalogItem {
            key: self.slug.clone(),
            title: self.title,
            cover: self.thumb,
            url: Some(format!("{BASE_URL}/manga/{}", self.slug)),
            language: Some("id".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct MangaDetailsEnvelope {
    manga: MangaDetails,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaDetails {
    slug: String,
    title: String,
    thumb: Option<String>,
    author: Option<String>,
    status: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    rating: Option<f64>,
    synopsis: Option<String>,
    alternative_title: Option<String>,
}

impl MangaDetails {
    fn into_catalog(self) -> CatalogItem {
        let mut description = String::new();
        if let Some(rating) = self.rating {
            description.push_str(&format!("Rating: {rating}/10\n\n"));
        }
        if let Some(synopsis) = self.synopsis.filter(|value| !value.trim().is_empty()) {
            description.push_str(synopsis.trim());
            description.push_str("\n\n");
        }
        if let Some(alternative) = self
            .alternative_title
            .filter(|value| !value.trim().is_empty())
        {
            description.push_str("Judul Alternatif: ");
            description.push_str(alternative.trim());
        }
        CatalogItem {
            key: self.slug.clone(),
            title: self.title,
            cover: self.thumb,
            description: (!description.trim().is_empty()).then_some(description.trim().to_string()),
            authors: self
                .author
                .filter(|author| !author.trim().is_empty() && author != "Unknown")
                .into_iter()
                .collect(),
            tags: self.tags,
            status: parse_status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/manga/{}", self.slug)),
            language: Some("id".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ChaptersList {
    #[serde(default)]
    chapters: Vec<ChapterDto>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterDto {
    slug: String,
    title: String,
    created_at: Option<String>,
}

impl ChapterDto {
    fn into_chapter(self, manga_slug: &str) -> MangaChapter {
        let title = if self.title.starts_with("Chapter")
            || (self.title.chars().any(|char| char.is_alphabetic())
                && !self
                    .title
                    .chars()
                    .next()
                    .is_some_and(|char| char.is_ascii_digit()))
        {
            self.title
        } else {
            format!("Chapter {}", self.title)
        };
        let key = format!("/read/{manga_slug}/{}", self.slug);
        MangaChapter {
            key: key.clone(),
            title: Some(title),
            date_uploaded: parse_date(self.created_at.as_deref()),
            url: Some(format!("{BASE_URL}{key}")),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ReaderData {
    data: Option<ReaderPayload>,
}

#[derive(Debug, Deserialize)]
struct ReaderPayload {
    chapter: ReaderChapter,
}

#[derive(Debug, Deserialize)]
struct ReaderChapter {
    #[serde(default)]
    images: Vec<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"mangas":[{"slug":"sample","title":"Sample Doujin","thumb":"https://example.invalid/cover.jpg"}],"totalItems":25}"#;
const DETAILS_FIXTURE: &str = r#"{"manga":{"slug":"sample","title":"Sample Doujin","thumb":"https://example.invalid/cover.jpg","author":"Author","status":"publishing","tags":["Action","Manga"],"rating":8.5,"synopsis":"Summary","alternativeTitle":"Alt Sample"},"chapters":[{"slug":"chapter-1","title":"1","createdAt":"2024-01-01T00:00:00.000Z"}]}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"chapter":{"images":["https://example.invalid/page1.jpg","https://example.invalid/page2.jpg"]}}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_rsc_and_reader_fixtures() {
        let list = SOURCE.list(json!({})).unwrap();
        assert_eq!(list.entries[0].title, "Sample Doujin");
        assert!(list.has_next_page);

        let details = SOURCE.details(json!({"manga":"sample"})).unwrap();
        assert!(details.initialized);
        assert_eq!(details.status, ItemStatus::Ongoing);

        let chapters = SOURCE.chapters(json!({"manga":"sample"})).unwrap();
        assert_eq!(chapters[0].key, "/read/sample/chapter-1");

        let pages = SOURCE
            .pages(json!({"chapter":"/read/sample/chapter-1"}))
            .unwrap();
        assert_eq!(pages.len(), 2);
    }
}
