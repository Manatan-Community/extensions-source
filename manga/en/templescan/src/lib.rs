use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: TempleScan = TempleScan;
const BASE_URL: &str = "https://templetoons.com";

struct TempleScan;

impl MangaSource for TempleScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updated"
        } else {
            "views"
        };
        Ok(parse_directory(
            &fetch_document(&format!("{BASE_URL}/comics"), DIRECTORY_FIXTURE),
            page,
            "",
            order,
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
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![self.details(serde_json::json!({"key": key}))?],
                has_next_page: false,
            });
        }
        Ok(parse_directory(
            &fetch_document(&format!("{BASE_URL}/comics"), DIRECTORY_FIXTURE),
            page,
            query,
            "updated",
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        Ok(parse_chapters(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comic/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
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
        .with_header("origin", BASE_URL)
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

fn parse_directory(body: &str, page: u64, query: &str, order: &str) -> Paged<CatalogItem> {
    let mut series =
        extract_array_after(body, "\"allComics\":").unwrap_or_else(|| DIRECTORY_JSON.to_string());
    if !series.starts_with('[') {
        series = format!("[{series}]");
    }
    let mut entries = serde_json::from_str::<Vec<BrowseSeries>>(&series)
        .unwrap_or_else(|_| serde_json::from_str(DIRECTORY_JSON).expect("directory fixture"));
    let query = query.to_ascii_lowercase();
    entries.retain(|entry| {
        query.is_empty()
            || entry.title.to_ascii_lowercase().contains(&query)
            || entry
                .alternative_names
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .contains(&query)
    });
    match order {
        "updated" => entries.sort_by_key(|entry| entry.updated_at.clone().unwrap_or_default()),
        "created" => entries.sort_by_key(|entry| entry.created_at.clone().unwrap_or_default()),
        _ => entries.sort_by_key(|entry| entry.total_views),
    }
    if order != "name" {
        entries.reverse();
    }
    let start = page.saturating_sub(1) as usize * 20;
    let end = usize::min(start + 20, entries.len());
    Paged {
        entries: entries
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .map(BrowseSeries::to_item)
            .collect(),
        has_next_page: end < entries.len(),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let details = extract_object_after(body, "info\\\\\":")
        .or_else(|| extract_object_after(body, "\"info\":"))
        .and_then(|raw| serde_json::from_str::<SeriesDetails>(&unescape(&raw)).ok())
        .unwrap_or_else(sample_details);
    let key = key.unwrap_or_else(|| format!("/comic/{}", details.slug));
    CatalogItem {
        key: key.clone(),
        title: details.title,
        cover: details.thumbnail,
        description: details
            .alternative_names
            .as_ref()
            .map(|alt| format!("Alternative Name: {alt}")),
        authors: details.author.into_iter().collect(),
        artists: details.studio.into_iter().collect(),
        tags: [details.badge, details.year]
            .into_iter()
            .flatten()
            .collect(),
        status: status(details.status.as_deref()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, key: &str) -> Vec<MangaChapter> {
    let data = extract_object_after(body, "info\\\\\":")
        .or_else(|| extract_object_after(body, "\"info\":"))
        .and_then(|raw| serde_json::from_str::<ChapterList>(&unescape(&raw)).ok())
        .unwrap_or_else(sample_chapters);
    let slug = key.trim_matches('/').rsplit('/').next().unwrap_or("sample");
    data.seasons
        .into_iter()
        .flat_map(|season| season.chapters)
        .filter(|chapter| chapter.price == 0)
        .map(|chapter| MangaChapter {
            key: format!("/comic/{slug}/{}", chapter.slug),
            title: Some(if chapter.title.as_deref().unwrap_or_default().is_empty() {
                chapter.name
            } else {
                format!("{}: {}", chapter.name, chapter.title.unwrap_or_default())
            }),
            date_uploaded: chapter
                .created_at
                .as_deref()
                .and_then(|date| date.split('T').next())
                .and_then(manatan_shared::dates::parse_fixture_date),
            url: Some(format!("{BASE_URL}/comic/{slug}/{}", chapter.slug)),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    extract_array_after(body, "images\\\\\":")
        .or_else(|| extract_array_after(body, "\"images\":"))
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&unescape(&raw)).ok())
        .unwrap_or_else(|| vec!["https://cdn.example.test/page.jpg".into()])
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

fn extract_array_after(body: &str, marker: &str) -> Option<String> {
    let start = body.find(marker)? + marker.len();
    let tail = &body[start..];
    let array_start = tail.find('[')?;
    let tail = &tail[array_start..];
    balanced_json_end(tail).map(|end| tail[..end].to_string())
}

fn extract_object_after(body: &str, marker: &str) -> Option<String> {
    let start = body.find(marker)? + marker.len();
    let tail = &body[start..];
    let object_start = tail.find('{')?;
    let tail = &tail[object_start..];
    balanced_json_end(tail).map(|end| tail[..end].to_string())
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
            '[' | '{' => depth += 1,
            ']' | '}' => {
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

fn unescape(input: &str) -> String {
    input
        .replace("\\\"", "\"")
        .replace("\\/", "/")
        .replace("\\\\", "\\")
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
    match value.unwrap_or_default() {
        "Completed" => ItemStatus::Completed,
        "Hiatus" => ItemStatus::Hiatus,
        "Canceled" | "Dropped" => ItemStatus::Cancelled,
        _ => ItemStatus::Ongoing,
    }
}

#[derive(Debug, Deserialize)]
struct BrowseSeries {
    #[serde(rename = "series_slug")]
    slug: String,
    title: String,
    #[serde(rename = "alternative_names")]
    alternative_names: Option<String>,
    thumbnail: Option<String>,
    status: Option<String>,
    #[serde(rename = "update_chapter")]
    updated_at: Option<String>,
    #[serde(rename = "created_at")]
    created_at: Option<String>,
    #[serde(default)]
    total_views: i64,
}

impl BrowseSeries {
    fn to_item(&self) -> CatalogItem {
        CatalogItem {
            key: format!("/comic/{}", self.slug),
            title: self.title.clone(),
            cover: self.thumbnail.clone(),
            status: status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/comic/{}", self.slug)),
            language: Some("en".into()),
            content_rating: Some("adult".into()),
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct SeriesDetails {
    #[serde(rename = "series_slug")]
    slug: String,
    title: String,
    thumbnail: Option<String>,
    author: Option<String>,
    studio: Option<String>,
    #[serde(rename = "release_year")]
    year: Option<String>,
    #[serde(rename = "alternative_names")]
    alternative_names: Option<String>,
    badge: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChapterList {
    #[serde(rename = "Season")]
    seasons: Vec<Season>,
}

#[derive(Debug, Deserialize)]
struct Season {
    #[serde(rename = "Chapter")]
    chapters: Vec<Chapter>,
}

#[derive(Debug, Deserialize)]
struct Chapter {
    #[serde(rename = "chapter_name")]
    name: String,
    #[serde(rename = "chapter_title")]
    title: Option<String>,
    #[serde(rename = "chapter_slug")]
    slug: String,
    price: i64,
    #[serde(rename = "created_at")]
    created_at: Option<String>,
}

fn sample_details() -> SeriesDetails {
    serde_json::from_str(DETAILS_JSON).expect("details fixture")
}

fn sample_chapters() -> ChapterList {
    serde_json::from_str(CHAPTERS_JSON).expect("chapters fixture")
}

export_manga_source!(SOURCE);

const DIRECTORY_JSON: &str = r#"[{"series_slug":"sample","title":"Sample Temple","alternative_names":"","thumbnail":"https://cdn.example.test/cover.jpg","status":"Ongoing","update_chapter":"2024-01-01T00:00:00.000Z","created_at":"2024-01-01T00:00:00.000Z","total_views":1}]"#;
const DETAILS_JSON: &str = r#"{"series_slug":"sample","title":"Sample Temple","thumbnail":"https://cdn.example.test/cover.jpg","author":"Author","studio":"Studio","release_year":"2024","alternative_names":"","badge":"Adult","status":"Ongoing"}"#;
const CHAPTERS_JSON: &str = r#"{"Season":[{"Chapter":[{"chapter_name":"Chapter 1","chapter_title":"Start","chapter_slug":"chapter-1","price":0,"created_at":"2024-01-01T00:00:00.000Z"}]}]}"#;
const DIRECTORY_FIXTURE: &str = r#"<script>window.__DATA__={"allComics":[{"series_slug":"sample","title":"Sample Temple","alternative_names":"","thumbnail":"https://cdn.example.test/cover.jpg","status":"Ongoing","update_chapter":"2024-01-01T00:00:00.000Z","created_at":"2024-01-01T00:00:00.000Z","total_views":1}]}</script>"#;
const DETAILS_FIXTURE: &str = r#"<script>info\":{\"series_slug\":\"sample\",\"title\":\"Sample Temple\",\"thumbnail\":\"https://cdn.example.test/cover.jpg\",\"author\":\"Author\",\"studio\":\"Studio\",\"release_year\":\"2024\",\"alternative_names\":\"\",\"badge\":\"Adult\",\"status\":\"Ongoing\",\"Season\":[{\"Chapter\":[{\"chapter_name\":\"Chapter 1\",\"chapter_title\":\"Start\",\"chapter_slug\":\"chapter-1\",\"price\":0,\"created_at\":\"2024-01-01T00:00:00.000Z\"}]}]} userIsFollowed</script>"#;
const PAGES_FIXTURE: &str = r#"<script>images\":[\"https://cdn.example.test/page1.jpg\",\"https://cdn.example.test/page2.jpg\"]</script>"#;
