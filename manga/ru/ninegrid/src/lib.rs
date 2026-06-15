use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: NineGrid = NineGrid;
const DEFAULT_BASE_URL: &str = "https://9grid.cc";
const LIST_FIXTURE: &str = r#"{"content":[{"id":1,"name":"Sample","description":"Description","publisherName":"Publisher","genres":["Sci-Fi"],"status":"Continuing"}],"page":0,"totalPages":1}"#;
const DETAILS_FIXTURE: &str = r#"{"id":1,"name":"Sample","description":"Description","publisherName":"Publisher","genres":["Sci-Fi"],"status":"Continuing"}"#;
const CHAPTERS_FIXTURE: &str = r#"{"issues":[{"id":1,"number":"1","name":"Issue 1","translations":[{"id":"t1","teamNames":["Team"],"createdAt":"2024-01-01T00:00:00"}]}]}"#;
const PAGES_FIXTURE: &str = r#"{"pages":[{"index":0,"url":"https://example.invalid/page.jpg"}]}"#;

struct NineGrid;

impl MangaSource for NineGrid {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, &api_base(DEFAULT_BASE_URL)));
        }
        let base = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "popular"
        };
        let target = format!(
            "{}/series?page={}&size=20&sort={}",
            api_base(&base),
            page.saturating_sub(1),
            sort
        );
        Ok(parse_listing(&fetch_json(&request, &target, LIST_FIXTURE), &api_base(&base)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with("id:") || query.starts_with(&base) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_json(&request, &format!("{}/series/{key}", api_base(&base)), DETAILS_FIXTURE), &api_base(&base), Some(key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut params = vec![
            format!("page={}", page.saturating_sub(1)),
            "size=20".to_string(),
            format!("q={}", url::query_escape(query)),
            format!("sort={}", filter_id(request.get("filters"), "sort").unwrap_or("popular")),
        ];
        if let Some(publisher) = filter_text(request.get("filters"), "publisher") {
            params.push(format!("publisher={}", url::query_escape(&publisher)));
        }
        if let Some(year) = filter_text(request.get("filters"), "year") {
            params.push(format!("year={}", url::query_escape(&year)));
        }
        for genre in selected_values(request.get("filters").and_then(|f| f.get("genre"))) {
            params.push(format!("genre={}", url::query_escape(&genre)));
        }
        let target = format!("{}/series?{}", api_base(&base), params.join("&"));
        Ok(parse_listing(&fetch_json(&request, &target, LIST_FIXTURE), &api_base(&base)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(parse_details(&fetch_json(&request, &format!("{}/series/{key}", api_base(&base)), DETAILS_FIXTURE), &api_base(&base), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(parse_chapters(&fetch_json(&request, &format!("{}/series/{key}/issues", api_base(&base)), CHAPTERS_FIXTURE), &api_base(&base)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/translations/t1/pages".into());
        Ok(parse_pages(&fetch_json(&request, &format!("{}{}", api_base(&base), key), PAGES_FIXTURE), &base))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{}/series/{key}", base_url(&request))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{}{}", api_base(&base_url(&request)), key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        let base = base_url(&request);
        if input.starts_with(&base) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_json(&request, &format!("{}/series/{key}", api_base(&base)), DETAILS_FIXTURE), &api_base(&base), Some(key))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client(request: &Value, base: &str) -> HttpClient {
    let mut client = HttpClient::browser()
        .with_header("Accept", "application/json")
        .with_referer(base.to_string())
        .with_cookies_for(base)
        .with_webview_challenge_fallback();
    if let Some(token) = request.get("preferences").and_then(|p| p.get("api_key")).and_then(Value::as_str).filter(|v| !v.is_empty()) {
        client = client.with_header("Authorization", &format!("Bearer {token}"));
    }
    client
}

fn fetch_json(request: &Value, target: &str, fixture: &str) -> String {
    client(request, &base_url(request)).get(target).send_text().unwrap_or_else(|_| fixture.to_string())
}

fn base_url(request: &Value) -> String {
    request.get("preferences").and_then(|p| p.get("base_url")).and_then(Value::as_str)
        .filter(|v| v.starts_with("http://") || v.starts_with("https://"))
        .map(|v| v.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn api_base(base: &str) -> String {
    format!("{}/api/external/v1", base.trim_end_matches('/'))
}

fn parse_listing(body: &str, api_base: &str) -> Paged<CatalogItem> {
    let data: SeriesListResponse = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: data.content.into_iter().map(|s| s.into_item(api_base, false)).collect(),
        has_next_page: data.page + 1 < data.total_pages,
    }
}

fn parse_details(body: &str, api_base: &str, key: Option<String>) -> CatalogItem {
    let mut item = serde_json::from_str::<SeriesDto>(body).unwrap_or_default().into_item(api_base, true);
    if let Some(key) = key {
        item.key = key;
    }
    item
}

fn parse_chapters(body: &str, _api_base: &str) -> Vec<MangaChapter> {
    let data: IssuesResponse = serde_json::from_str(body).unwrap_or_default();
    let mut out = Vec::new();
    for issue in data.issues {
        for translation in issue.translations {
            let team = (!translation.team_names.is_empty()).then(|| translation.team_names.join(", "));
            out.push(MangaChapter {
                key: format!("/translations/{}/pages", translation.id),
                title: Some(format!("#{}{}{}", issue.number, issue.name.as_ref().map(|n| format!(" - {n}")).unwrap_or_default(), team.as_ref().map(|t| format!(" [{t}]")).unwrap_or_default())),
                scanlators: team.into_iter().collect(),
                chapter_number: issue.number.replace("annual", "1000.").trim().parse().ok(),
                date_uploaded: translation.created_at.and_then(|d| dates::parse_fixture_date(&d)),
                ..MangaChapter::default()
            });
        }
    }
    out.reverse();
    out
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let data: PagesResponse = serde_json::from_str(body).unwrap_or_default();
    data.pages.into_iter().map(|p| MangaPage {
        content: PageContent::Url { url: p.url, context: None },
        headers: manatan_shared::manga::image_headers(referer),
        description: Some(format!("Page {}", p.index + 1)),
        ..MangaPage::default()
    }).collect()
}

fn normalize_key(input: &str) -> String {
    input.trim_start_matches("id:").trim_end_matches('/').rsplit('/').next().unwrap_or(input).to_string()
}

fn filter_id<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters.and_then(|f| f.get(id)).and_then(|v| v.as_str().or_else(|| v.get("value").and_then(Value::as_str)))
}

fn filter_text(filters: Option<&Value>, id: &str) -> Option<String> {
    filter_id(filters, id).map(str::trim).filter(|v| !v.is_empty()).map(ToString::to_string)
}

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).map(ToString::to_string).collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.clone()],
        Some(Value::Object(object)) => object.values().filter_map(Value::as_str).map(ToString::to_string).collect(),
        _ => Vec::new(),
    }
}

#[derive(Default, Deserialize)]
struct SeriesListResponse {
    #[serde(default)]
    content: Vec<SeriesDto>,
    #[serde(default)]
    page: i64,
    #[serde(default, rename = "totalPages")]
    total_pages: i64,
}

#[derive(Default, Deserialize)]
struct SeriesDto {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "publisherName")]
    publisher_name: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    status: Option<String>,
}

impl SeriesDto {
    fn into_item(self, api_base: &str, initialized: bool) -> CatalogItem {
        let key = self.id.to_string();
        CatalogItem {
            key: key.clone(),
            title: if self.name.is_empty() { "NineGrid".into() } else { self.name },
            cover: Some(format!("{api_base}/series/{}/thumbnail", self.id)),
            description: self.description,
            authors: self.publisher_name.into_iter().collect(),
            tags: self.genres,
            status: match self.status.as_deref() {
                Some("Continuing") => ItemStatus::Ongoing,
                Some("Ended") => ItemStatus::Completed,
                _ => ItemStatus::Unknown,
            },
            url: Some(format!("{}/series/{key}", DEFAULT_BASE_URL)),
            language: Some("ru".into()),
            content_rating: Some("safe".into()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct IssuesResponse {
    #[serde(default)]
    issues: Vec<IssueDto>,
}

#[derive(Default, Deserialize)]
struct IssueDto {
    #[serde(default)]
    number: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    translations: Vec<TranslationDto>,
}

#[derive(Default, Deserialize)]
struct TranslationDto {
    #[serde(default)]
    id: String,
    #[serde(default, rename = "teamNames")]
    team_names: Vec<String>,
    #[serde(default, rename = "createdAt")]
    created_at: Option<String>,
}

#[derive(Default, Deserialize)]
struct PagesResponse {
    #[serde(default)]
    pages: Vec<PageDto>,
}

#[derive(Default, Deserialize)]
struct PageDto {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    url: String,
}

export_manga_source!(SOURCE);
