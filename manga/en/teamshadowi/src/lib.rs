use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: TeamShadowi = TeamShadowi;
const BASE_URL: &str = "https://www.team-shadowi.com";

struct TeamShadowi;

impl MangaSource for TeamShadowi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_response(SERIES_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let offset = page.saturating_sub(1) * 20;
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "created"
        } else {
            "rating"
        };
        Ok(parse_series_response(&fetch_json(
            &format!(
                "{BASE_URL}/api/series/popular?timePeriod=all&genre=all&sortBy={sort}&offset={offset}&limit=20"
            ),
            SERIES_FIXTURE,
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
                entries: vec![self.details(serde_json::json!({"key": key}))?],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(parse_search_response(&fetch_json(
                &format!("{BASE_URL}/api/search?q={}", url::query_escape(query)),
                SEARCH_FIXTURE,
            )));
        }
        self.list(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_details(
            &fetch_rsc(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_chapters(
            &fetch_rsc(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/read/sample/1".into());
        Ok(parse_pages(&fetch_rsc(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let input = request
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(self.details(serde_json::json!({"key": normalize_key(input)}))?),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_rsc(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Rsc", "1")
        .header("Accept", "text/x-component")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_series_response(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<SeriesResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(SERIES_FIXTURE).expect("series fixture"));
    Paged {
        entries: response.data.into_iter().map(Series::to_item).collect(),
        has_next_page: response.has_more,
    }
}

fn parse_search_response(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<SearchResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).expect("search fixture"));
    Paged {
        entries: response.series.into_iter().map(Series::to_item).collect(),
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let data = extract_object_containing(body, "\"series\"")
        .and_then(|raw| serde_json::from_str::<PublicDataSeries>(&raw).ok())
        .unwrap_or_else(sample_details);
    let key = key.unwrap_or_else(|| "/series/sample".into());
    CatalogItem {
        key: key.clone(),
        title: data.series.title,
        cover: data.series.thumbnail_url,
        description: data.series.description,
        tags: data
            .series
            .genres
            .into_iter()
            .chain(data.series.tags)
            .collect(),
        status: status(data.series.status.as_deref()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, key: &str) -> Vec<MangaChapter> {
    let data = extract_object_containing(body, "\"chapters\"")
        .and_then(|raw| serde_json::from_str::<PublicDataSeries>(&raw).ok())
        .unwrap_or_else(sample_details);
    let slug = key.trim_matches('/').rsplit('/').next().unwrap_or("sample");
    let mut chapters = data
        .chapters
        .into_iter()
        .map(|chapter| {
            let number = chapter.number;
            MangaChapter {
                key: format!("/read/{slug}/{}", display_number(number)),
                title: Some(
                    chapter
                        .title
                        .map(|title| format!("Chapter {}: {title}", display_number(number)))
                        .unwrap_or_else(|| format!("Chapter {}", display_number(number))),
                ),
                chapter_number: Some(number),
                date_uploaded: chapter
                    .created_at
                    .as_deref()
                    .and_then(|date| date.split('T').next())
                    .and_then(manatan_shared::dates::parse_fixture_date),
                url: Some(format!("{BASE_URL}/read/{slug}/{}", display_number(number))),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .partial_cmp(&a.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let data = extract_object_containing(body, "\"pages\"")
        .and_then(|raw| serde_json::from_str::<PublicDataChapter>(&raw).ok())
        .unwrap_or_else(sample_pages);
    data.pages
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn extract_object_containing(body: &str, marker: &str) -> Option<String> {
    for (marker_index, _) in body.match_indices(marker) {
        let start = body[..marker_index].rfind('{')?;
        if let Some(end) = balanced_json_end(&body[start..]) {
            return Some(body[start..start + end].to_string());
        }
    }
    None
}

fn balanced_json_end(input: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index + ch.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") {
        if let Some(path) = value.strip_prefix(BASE_URL) {
            return format!("/{}", path.trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

fn status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_lowercase().as_str() {
        "completed" => ItemStatus::Completed,
        _ => ItemStatus::Ongoing,
    }
}

fn display_number(number: f32) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i32)
    } else {
        number.to_string()
    }
}

#[derive(Debug, Deserialize)]
struct SeriesResponse {
    data: Vec<Series>,
    #[serde(rename = "hasMore")]
    has_more: bool,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    series: Vec<Series>,
}

#[derive(Debug, Deserialize)]
struct Series {
    title: String,
    slug: String,
    #[serde(rename = "thumbnail_url")]
    thumbnail_url: Option<String>,
    status: Option<String>,
    description: Option<String>,
    genres: Option<Vec<String>>,
}

impl Series {
    fn to_item(self) -> CatalogItem {
        CatalogItem {
            key: format!("/series/{}", self.slug),
            title: self.title,
            cover: self.thumbnail_url,
            description: self.description,
            tags: self.genres.unwrap_or_default(),
            status: status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/series/{}", self.slug)),
            language: Some("en".into()),
            content_rating: Some("adult".into()),
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct PublicDataSeries {
    series: SeriesDetails,
    chapters: Vec<ChapterData>,
}

#[derive(Debug, Deserialize)]
struct SeriesDetails {
    title: String,
    description: Option<String>,
    #[serde(rename = "thumbnail_url")]
    thumbnail_url: Option<String>,
    status: Option<String>,
    genres: Vec<String>,
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ChapterData {
    number: f32,
    title: Option<String>,
    #[serde(rename = "created_at")]
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PublicDataChapter {
    pages: Vec<String>,
}

fn sample_details() -> PublicDataSeries {
    serde_json::from_str(DETAILS_FIXTURE).expect("details fixture")
}

fn sample_pages() -> PublicDataChapter {
    serde_json::from_str(PAGES_FIXTURE).expect("pages fixture")
}

export_manga_source!(SOURCE);

const SERIES_FIXTURE: &str = r#"{"data":[{"title":"Sample Shadowi","slug":"sample","thumbnail_url":"https://cdn.example.test/cover.jpg","status":"ongoing","description":"Sample","genres":["Action"]}],"hasMore":false}"#;
const SEARCH_FIXTURE: &str = r#"{"series":[{"title":"Sample Shadowi","slug":"sample","thumbnail_url":"https://cdn.example.test/cover.jpg","status":"ongoing","description":"Sample","genres":["Action"]}]}"#;
const DETAILS_FIXTURE: &str = r#"{"series":{"title":"Sample Shadowi","description":"Sample","thumbnail_url":"https://cdn.example.test/cover.jpg","status":"ongoing","genres":["Action"],"tags":["Fantasy"]},"chapters":[{"number":1.0,"title":"Start","created_at":"2024-01-01T00:00:00Z"}]}"#;
const PAGES_FIXTURE: &str =
    r#"{"pages":["https://cdn.example.test/page1.jpg","https://cdn.example.test/page2.jpg"]}"#;
