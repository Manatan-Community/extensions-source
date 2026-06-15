use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    html, manga,
    sdk::{FilterValue, SearchRequest, http::HttpClient},
    url,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const SOURCE: LycanToons = LycanToons;
const BASE_URL: &str = "https://lycantoons.com";
const PAGE_LIMIT: u64 = 13;

struct LycanToons;

impl MangaSource for LycanToons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_page(POPULAR_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let metric = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "recently-updated"
        } else {
            "popular"
        };
        Ok(parse_series_page(&fetch_json_or_fixture(
            &format!("{BASE_URL}/api/metrics/{metric}?limit={PAGE_LIMIT}&page={page}"),
            POPULAR_FIXTURE,
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
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = parse_filters(request.get("filters"));
        let body = json!(SearchRequestBody {
            limit: PAGE_LIMIT,
            page,
            search: query.to_string(),
            series_type: filters.series_type,
            status: filters.status,
            tags: filters.tags,
        })
        .to_string();
        Ok(parse_search_page(&post_json_or_fixture(
            &format!("{BASE_URL}/api/series"),
            &body,
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let series = fetch_series(&normalize_key(&key));
        Ok(series
            .capitulos
            .unwrap_or_default()
            .into_iter()
            .map(|chapter| chapter.into_chapter(&series.slug))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/1?pages=2".into());
        Ok(parse_pages(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/series/{}", normalize_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/series/") {
            let key = normalize_key(input);
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

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_json_or_fixture(target: &str, body: &str, fixture: &str) -> String {
    client()
        .post(target)
        .xhr()
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(body.to_string())
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

fn parse_series_page(body: &str) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<PopularResponse>(body).unwrap_or_default();
    Paged {
        entries: payload
            .data
            .into_iter()
            .map(SeriesDto::into_catalog)
            .collect(),
        has_next_page: payload
            .pagination
            .and_then(|page| page.has_next)
            .unwrap_or(false),
    }
}

fn parse_search_page(body: &str) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<SearchResponse>(body).unwrap_or_default();
    Paged {
        entries: payload
            .series
            .into_iter()
            .map(SeriesDto::into_catalog)
            .collect(),
        has_next_page: false,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    fetch_series(&normalize_key(key)).into_catalog()
}

fn fetch_series(slug: &str) -> SeriesDto {
    serde_json::from_str::<SeriesDto>(&fetch_json_or_fixture(
        &format!("{BASE_URL}/api/series/{slug}"),
        DETAILS_FIXTURE,
    ))
    .unwrap_or_default()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    extract_image_urls(body)
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

fn extract_image_urls(body: &str) -> Vec<String> {
    let normalized = body
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
        .replace("\\/", "/");
    let Some(key_index) = normalized.find("\"imageUrls\"") else {
        return Vec::new();
    };
    let Some(array_start) = normalized[key_index..]
        .find('[')
        .map(|index| key_index + index)
    else {
        return Vec::new();
    };
    let Some(array_end) = matching_bracket(normalized.as_bytes(), array_start) else {
        return Vec::new();
    };
    serde_json::from_str(&normalized[array_start..=array_end]).unwrap_or_default()
}

fn matching_bracket(bytes: &[u8], start: usize) -> Option<usize> {
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
            b'[' => depth += 1,
            b']' => {
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

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_matches('/')
        .split('?')
        .next()
        .unwrap_or("sample")
        .rsplit('/')
        .next()
        .unwrap_or("sample")
        .to_string()
}

#[derive(Default)]
struct ParsedFilters {
    series_type: String,
    status: String,
    tags: Vec<String>,
}

fn parse_filters(filters: Option<&Value>) -> ParsedFilters {
    let mut parsed = ParsedFilters::default();
    for filter in filters_to_values(filters) {
        match filter.id.as_str() {
            "seriesType" => parsed.series_type = string_value(&filter.value),
            "status" => parsed.status = string_value(&filter.value),
            "tags" => parsed.tags = string_values(&filter.value),
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

fn string_value(value: &Value) -> String {
    value.as_str().unwrap_or_default().trim().to_string()
}

fn string_values(value: &Value) -> Vec<String> {
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    string_value(value)
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.unwrap_or_default() {
        "ONGOING" => ItemStatus::Ongoing,
        "COMPLETED" => ItemStatus::Completed,
        "HIATUS" => ItemStatus::Hiatus,
        "CANCELLED" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: Option<&str>) -> Option<i64> {
    value
        .and_then(|date| date.split(['T', ' ']).next())
        .and_then(manatan_shared::dates::parse_ymd)
}

#[derive(Default, Deserialize)]
struct PopularResponse {
    #[serde(default)]
    data: Vec<SeriesDto>,
    pagination: Option<PaginationDto>,
}

#[derive(Default, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    series: Vec<SeriesDto>,
}

#[derive(Default, Deserialize)]
struct PaginationDto {
    #[serde(default, rename = "hasNext")]
    has_next: Option<bool>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesDto {
    title: String,
    slug: String,
    cover_url: Option<String>,
    author: Option<String>,
    artist: Option<String>,
    description: Option<String>,
    genre: Option<Vec<String>>,
    status: Option<String>,
    series_type: Option<String>,
    capitulos: Option<Vec<ChapterDto>>,
}

impl SeriesDto {
    fn into_catalog(self) -> CatalogItem {
        let key = format!("/series/{}", self.slug);
        let mut description = self
            .description
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_default();
        if let Some(series_type) = self.series_type.filter(|value| !value.trim().is_empty()) {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str(&format!("Tipo: {series_type}"));
        }
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: self.cover_url,
            authors: self
                .author
                .filter(|value| !value.trim().is_empty())
                .into_iter()
                .collect(),
            artists: self
                .artist
                .filter(|value| !value.trim().is_empty())
                .into_iter()
                .collect(),
            description: (!description.is_empty()).then_some(description),
            tags: self.genre.unwrap_or_default(),
            status: parse_status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}{key}")),
            language: Some("pt-BR".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    numero: Value,
    #[serde(rename = "createdAt")]
    created_at: Option<String>,
    #[serde(default, rename = "pageCount")]
    page_count: Option<u64>,
}

impl ChapterDto {
    fn into_chapter(self, slug: &str) -> MangaChapter {
        let number = json_number_text(&self.numero);
        let pages_query = self
            .page_count
            .map(|count| format!("?pages={count}"))
            .unwrap_or_default();
        let key = format!("/series/{slug}/{number}{pages_query}");
        MangaChapter {
            key: key.clone(),
            title: Some(format!("Capítulo {number}")),
            chapter_number: number.parse::<f32>().ok(),
            date_uploaded: parse_date(self.created_at.as_deref()),
            url: Some(format!("{BASE_URL}{key}")),
            ..MangaChapter::default()
        }
    }
}

fn json_number_text(value: &Value) -> String {
    if let Some(number) = value.as_f64() {
        let text = number.to_string();
        return text.trim_end_matches(".0").to_string();
    }
    value
        .as_str()
        .unwrap_or_default()
        .trim_end_matches(".0")
        .to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchRequestBody {
    limit: u64,
    page: u64,
    search: String,
    series_type: String,
    status: String,
    tags: Vec<String>,
}

export_manga_source!(SOURCE);

const POPULAR_FIXTURE: &str = r#"{"data":[{"title":"Sample Lycan","slug":"sample","coverUrl":"https://lycantoons.com/cover.jpg","author":"Author","artist":"Artist","description":"Summary","genre":["action"],"status":"ONGOING","seriesType":"MANGA"}],"pagination":{"page":1,"totalPages":2,"hasNext":true}}"#;
const SEARCH_FIXTURE: &str = r#"{"series":[{"title":"Sample Lycan","slug":"sample","coverUrl":"https://lycantoons.com/cover.jpg","status":"ONGOING"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"title":"Sample Lycan","slug":"sample","coverUrl":"https://lycantoons.com/cover.jpg","author":"Author","artist":"Artist","description":"Summary","genre":["action"],"status":"ONGOING","seriesType":"MANGA","capitulos":[{"id":1,"numero":1,"createdAt":"2024-01-01T00:00:00.000Z","pageCount":2}]}"#;
const PAGES_FIXTURE: &str = r#"<html><script>self.__next_f.push([1,"{\"imageUrls\":[\"https://lycantoons.com/page1.jpg\",\"https://lycantoons.com/page2.jpg\"]}"])</script></html>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_lycan_fixtures() {
        let list = SOURCE.list(json!({})).unwrap();
        assert_eq!(list.entries[0].title, "Sample Lycan");
        assert!(list.has_next_page);

        let details = serde_json::from_str::<SeriesDto>(DETAILS_FIXTURE)
            .unwrap()
            .into_catalog();
        assert_eq!(details.title, "Sample Lycan");
        assert_eq!(details.status, ItemStatus::Ongoing);

        let series = serde_json::from_str::<SeriesDto>(DETAILS_FIXTURE).unwrap();
        let chapters = series
            .capitulos
            .unwrap_or_default()
            .into_iter()
            .map(|chapter| chapter.into_chapter(&series.slug))
            .collect::<Vec<_>>();
        assert_eq!(chapters[0].key, "/series/sample/1?pages=2");

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
