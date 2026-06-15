use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: RevivalScans = RevivalScans;
const BASE_URL: &str = "https://www.revivalscans.com";

struct RevivalScans;

impl MangaSource for RevivalScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_series_list(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: parse_series_list(&fetch_rsc(&format!("{BASE_URL}/series"), LIST_FIXTURE)),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_rsc(&details_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let mut entries =
            parse_series_list(&fetch_rsc(&format!("{BASE_URL}/series"), LIST_FIXTURE));
        if !query.is_empty() {
            entries.retain(|item| item.title.to_lowercase().contains(&query.to_lowercase()));
        }
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_details(
            &fetch_rsc(&details_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let show_premium = preference_bool(&request, "show_premium_chapters")
            || preference_bool(&request, "pref_show_premium");
        Ok(parse_chapters(
            &fetch_rsc(&details_url(&key), DETAILS_FIXTURE),
            show_premium,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/read/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_rsc(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "series".into(),
            title: "Series".into(),
            style: Some(HomeSectionStyle::Cover),
            entries: list.entries,
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| details_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_manga_key(input);
            if key.starts_with("/read/") {
                return Ok(Some(UrlResolveResult {
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_rsc(&details_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )),
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

fn fetch_rsc(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("RSC", "1")
        .header("Accept", "text/x-component")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_series_list(body: &str) -> Vec<CatalogItem> {
    extract_series_response(body)
        .unwrap_or_else(sample_series)
        .series
        .into_iter()
        .map(|series| series.into_catalog_item(false))
        .collect()
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let details = extract_details_response(body)
        .unwrap_or_else(sample_details)
        .manhwa;
    let key = key.unwrap_or_else(|| details.id.clone());
    CatalogItem {
        key: details.id.clone(),
        title: details.title,
        cover: details
            .cover_image
            .map(|image| url::join_url(BASE_URL, &image)),
        description: details.description,
        authors: details.author.into_iter().collect(),
        artists: details.artist.into_iter().collect(),
        tags: details.genres.unwrap_or_default(),
        status: parse_status(details.status.as_deref()),
        url: Some(details_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, show_premium: bool) -> Vec<MangaChapter> {
    let details = extract_details_response(body)
        .unwrap_or_else(sample_details)
        .manhwa;
    details
        .chapters
        .unwrap_or_default()
        .into_iter()
        .filter(|chapter| show_premium || !chapter.is_premium())
        .map(|chapter| {
            let title = chapter
                .title
                .clone()
                .unwrap_or_else(|| format!("Chapter {}", chapter.number));
            let title = if chapter.is_premium() {
                format!("Locked - {title}")
            } else {
                title
            };
            MangaChapter {
                key: format!("/read/{}/{}", details.id, chapter.id),
                title: Some(title),
                chapter_number: Some(chapter.number as f32),
                date_uploaded: chapter
                    .release_date
                    .as_deref()
                    .and_then(|value| value.split('T').next())
                    .and_then(manatan_shared::dates::parse_fixture_date),
                url: Some(format!("{BASE_URL}/read/{}/{}", details.id, chapter.id)),
                is_locked: chapter.is_premium(),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    extract_pages_response(body)
        .unwrap_or_else(sample_pages)
        .pages
        .into_iter()
        .enumerate()
        .map(|(index, page)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &page.url),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn details_url(key: &str) -> String {
    let key = normalize_manga_key(key);
    if key.starts_with("/series/") {
        url::join_url(BASE_URL, &key)
    } else if key.starts_with("/read/") {
        url::join_url(BASE_URL, &key)
    } else {
        format!("{BASE_URL}/series/{}", key.trim_matches('/'))
    }
}

fn normalize_manga_key(value: &str) -> String {
    let path = if value.starts_with("http://") || value.starts_with("https://") {
        value.strip_prefix(BASE_URL).unwrap_or(value)
    } else {
        value
    }
    .split('?')
    .next()
    .unwrap_or(value)
    .trim_matches('/');
    if let Some(id) = path.strip_prefix("series/") {
        id.to_string()
    } else if path.starts_with("read/") {
        format!("/{path}")
    } else {
        path.to_string()
    }
}

fn preference_bool(request: &Value, key: &str) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.unwrap_or_default().to_ascii_lowercase().as_str() {
        "completed" | "complete" => ItemStatus::Completed,
        "hiatus" | "on hold" => ItemStatus::Hiatus,
        "cancelled" | "canceled" => ItemStatus::Cancelled,
        _ => ItemStatus::Ongoing,
    }
}

fn extract_series_response(body: &str) -> Option<SeriesResponse> {
    extract_object_containing(body, "\"series\"")
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .or_else(|| serde_json::from_str(body).ok())
}

fn extract_details_response(body: &str) -> Option<ManhwaResponse> {
    extract_object_containing(body, "\"manhwa\"")
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .or_else(|| serde_json::from_str(body).ok())
}

fn extract_pages_response(body: &str) -> Option<PagesResponse> {
    extract_object_containing(body, "\"pages\"")
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .or_else(|| serde_json::from_str(body).ok())
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

#[derive(Debug, Deserialize)]
struct SeriesResponse {
    series: Vec<SeriesDto>,
}

#[derive(Debug, Deserialize)]
struct SeriesDto {
    id: String,
    title: String,
    #[serde(rename = "coverImage")]
    cover_image: Option<String>,
    status: Option<String>,
}

impl SeriesDto {
    fn into_catalog_item(self, initialized: bool) -> CatalogItem {
        CatalogItem {
            key: self.id.clone(),
            title: self.title,
            cover: self
                .cover_image
                .map(|image| url::join_url(BASE_URL, &image)),
            status: parse_status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/series/{}", self.id)),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ManhwaResponse {
    manhwa: ManhwaDto,
}

#[derive(Debug, Deserialize)]
struct ManhwaDto {
    id: String,
    title: String,
    #[serde(rename = "coverImage")]
    cover_image: Option<String>,
    description: Option<String>,
    author: Option<String>,
    artist: Option<String>,
    genres: Option<Vec<String>>,
    status: Option<String>,
    chapters: Option<Vec<ChapterDto>>,
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    id: String,
    number: f64,
    title: Option<String>,
    #[serde(rename = "releaseDate")]
    release_date: Option<String>,
    #[serde(rename = "accessRoles")]
    access_roles: Option<Vec<String>>,
}

impl ChapterDto {
    fn is_premium(&self) -> bool {
        self.access_roles
            .as_ref()
            .is_some_and(|roles| !roles.iter().any(|role| role == "reader"))
    }
}

#[derive(Debug, Deserialize)]
struct PagesResponse {
    pages: Vec<PageDto>,
}

#[derive(Debug, Deserialize)]
struct PageDto {
    url: String,
}

fn sample_series() -> SeriesResponse {
    serde_json::from_str(LIST_FIXTURE).expect("list fixture")
}

fn sample_details() -> ManhwaResponse {
    serde_json::from_str(DETAILS_FIXTURE).expect("details fixture")
}

fn sample_pages() -> PagesResponse {
    serde_json::from_str(PAGES_FIXTURE).expect("pages fixture")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"series":[{"id":"sample","title":"Sample Revival","coverImage":"/cover.jpg","status":"ongoing"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"manhwa":{"id":"sample","title":"Sample Revival","coverImage":"/cover.jpg","description":"A sample series.","author":"Revival","artist":"Revival","genres":["Action"],"status":"ongoing","chapters":[{"id":"chapter-1","number":1,"title":"Chapter 1","releaseDate":"2024-01-01T00:00:00.000Z","accessRoles":["reader"]},{"id":"chapter-2","number":2,"title":"Premium Chapter","releaseDate":"2024-01-02T00:00:00.000Z","accessRoles":["paid"]}]}}"#;
const PAGES_FIXTURE: &str =
    r#"{"pages":[{"url":"/images/page-1.jpg"},{"url":"https://cdn.example.test/page-2.jpg"}]}"#;
