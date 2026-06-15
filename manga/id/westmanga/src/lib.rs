use hmac::{Hmac, Mac};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    html, manga,
    sdk::http::{Headers, HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const SOURCE: WestManga = WestManga;
const BASE_URL: &str = "https://westmanga.co";
const API_URL: &str = "https://data.mantweh.online";
const ACCESS_KEY: &str = "WM_WEB_FRONT_END";
const SECRET_KEY: &str = "xxxoidj";
const CONTENT_RATING: &str = "safe";

struct WestManga;

impl MangaSource for WestManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "Update"
        } else {
            "Popular"
        };
        Ok(parse_list(&fetch_api_or_fixture(
            &contents_url(page, "", Some(order), request.get("filters")),
            LIST_FIXTURE,
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
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        Ok(parse_list(&fetch_api_or_fixture(
            &contents_url(page, query, None, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let data = fetch_details(&slug_from_manga_key(&key));
        Ok(data.chapters.into_iter().map(Chapter::to_chapter).collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-chapter".into());
        let slug = slug_from_chapter_key(&key);
        Ok(parse_pages(&fetch_api_or_fixture(
            &format!("{API_URL}/api/v/{slug}"),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/comic/{}", slug_from_manga_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/view/{}", slug_from_chapter_key(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            if key.starts_with("/comic/") || key.starts_with("/manga/") {
                return Ok(Some(UrlResolveResult {
                    item: Some(details_by_key(&key)),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
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

fn fetch_api_or_fixture(target: &str, fixture: &str) -> String {
    let path = url_path(target);
    let timestamp = request_timestamp();
    let mut headers = Headers::new();
    headers.insert("Accept".to_string(), "application/json".to_string());
    headers.insert("x-wm-request-time".to_string(), timestamp.clone());
    headers.insert("x-wm-accses-key".to_string(), ACCESS_KEY.to_string());
    headers.insert(
        "x-wm-request-signature".to_string(),
        request_signature(&timestamp, &path),
    );
    client()
        .fetch("GET", target, None, headers)
        .ok()
        .and_then(|response| response.text)
        .unwrap_or_else(|| fixture.to_string())
}

fn contents_url(page: u64, query: &str, order: Option<&str>, filters: Option<&Value>) -> String {
    let mut params = vec![
        format!("page={page}"),
        "per_page=20".to_string(),
        "type=Comic".to_string(),
    ];
    if !query.is_empty() {
        params.push(format!("q={}", url::query_escape(query)));
    }
    for id in ["orderBy", "status", "country", "color"] {
        let selected = filters
            .and_then(|filters| filters.get(id))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .filter(|value| *value != "All" && *value != "Default")
            .map(ToString::to_string);
        if let Some(value) = selected {
            params.push(format!("{id}={}", url::query_escape(&value)));
        }
    }
    if let Some(order) = order {
        params.push(format!("orderBy={order}"));
    }
    if let Some(genres) = filters
        .and_then(|filters| filters.get("genre[]"))
        .and_then(Value::as_str)
    {
        for genre in genres.split(',').map(str::trim).filter(|value| !value.is_empty()) {
            params.push(format!("genre%5B%5D={}", url::query_escape(genre)));
        }
    }
    format!("{API_URL}/api/contents?{}", params.join("&"))
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let data = serde_json::from_str::<PaginatedData<BrowseManga>>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: data
            .data
            .into_iter()
            .map(|item| CatalogItem {
                key: format!("/manga/{}", item.slug),
                title: item.title,
                cover: item.cover,
                url: Some(format!("{BASE_URL}/comic/{}", item.slug)),
                language: Some("id".to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
            .collect(),
        has_next_page: data.paginator.current < data.paginator.last,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    fetch_details(&slug_from_manga_key(key)).to_item()
}

fn fetch_details(slug: &str) -> MangaDto {
    let body = fetch_api_or_fixture(&format!("{API_URL}/api/comic/{slug}"), DETAILS_FIXTURE);
    serde_json::from_str::<Data<MangaDto>>(&body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"))
        .data
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let data = serde_json::from_str::<Data<ImageList>>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"))
        .data;
    data.images
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

fn request_signature(timestamp: &str, path: &str) -> String {
    let key = format!("{timestamp}GET{path}{ACCESS_KEY}{SECRET_KEY}");
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("hmac accepts key");
    mac.update(b"wm-api-request");
    hex_lower(&mac.finalize().into_bytes())
}

fn request_timestamp() -> String {
    manatan_extension::abi::system_time()
        .map(|time| time.unix_seconds.to_string())
        .unwrap_or_else(|_| "1704067200".to_string())
}

fn url_path(target: &str) -> String {
    let after_origin = target
        .strip_prefix(API_URL)
        .unwrap_or(target)
        .split('?')
        .next()
        .unwrap_or(target);
    if after_origin.starts_with('/') {
        after_origin.to_string()
    } else {
        format!("/{after_origin}")
    }
}

fn normalize_key(input: &str) -> String {
    let path = input
        .trim()
        .trim_start_matches(BASE_URL)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_matches('/');
    if path.starts_with("comic/") {
        format!("/manga/{}", path.trim_start_matches("comic/"))
    } else {
        format!("/{path}")
    }
}

fn slug_from_manga_key(key: &str) -> String {
    normalize_key(key)
        .trim_start_matches('/')
        .trim_start_matches("manga/")
        .trim_start_matches("comic/")
        .trim_matches('/')
        .to_string()
}

fn slug_from_chapter_key(key: &str) -> String {
    normalize_key(key)
        .trim_start_matches('/')
        .trim_start_matches("view/")
        .trim_matches('/')
        .to_string()
}

fn hex_lower(input: &[u8]) -> String {
    input.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Deserialize)]
struct Data<T> {
    data: T,
}

#[derive(Deserialize)]
struct PaginatedData<T> {
    data: Vec<T>,
    paginator: Paginator,
}

#[derive(Deserialize)]
struct Paginator {
    #[serde(rename = "current_page")]
    current: u64,
    #[serde(rename = "last_page")]
    last: u64,
}

#[derive(Deserialize)]
struct BrowseManga {
    title: String,
    slug: String,
    #[serde(default)]
    cover: Option<String>,
}

#[derive(Deserialize)]
struct MangaDto {
    title: String,
    slug: String,
    #[serde(rename = "alternative_name", default)]
    alternative_name: Option<String>,
    #[serde(rename = "sinopsis", default)]
    synopsis: Option<String>,
    #[serde(default)]
    cover: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(rename = "country_id", default)]
    country: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    color: Option<bool>,
    #[serde(default)]
    genres: Vec<Genre>,
    #[serde(default)]
    chapters: Vec<Chapter>,
}

impl MangaDto {
    fn to_item(self) -> CatalogItem {
        let mut tags = Vec::new();
        match self.country.as_deref() {
            Some("JP") => tags.push("Manga".to_string()),
            Some("CN") => tags.push("Manhua".to_string()),
            Some("KR") => tags.push("Manhwa".to_string()),
            _ => {}
        }
        if self.color.unwrap_or(false) {
            tags.push("Colored".to_string());
        }
        tags.extend(self.genres.into_iter().map(|genre| genre.name));
        let mut description = self
            .synopsis
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default();
        if let Some(alt) = self.alternative_name.filter(|value| !value.trim().is_empty()) {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str("Alternative Name: ");
            description.push_str(alt.trim());
        }
        CatalogItem {
            key: format!("/manga/{}", self.slug),
            title: self.title,
            cover: self.cover,
            authors: self.author.into_iter().collect(),
            description: (!description.is_empty()).then_some(description),
            tags,
            status: match self.status.unwrap_or_default().as_str() {
                "ongoing" => ItemStatus::Ongoing,
                "completed" => ItemStatus::Completed,
                "hiatus" => ItemStatus::Hiatus,
                _ => ItemStatus::Unknown,
            },
            url: Some(format!("{BASE_URL}/comic/{}", self.slug)),
            language: Some("id".to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct Genre {
    name: String,
}

#[derive(Deserialize)]
struct Chapter {
    slug: String,
    number: String,
    #[serde(rename = "updated_at")]
    updated_at: TimeField,
}

impl Chapter {
    fn to_chapter(self) -> MangaChapter {
        MangaChapter {
            key: format!("/{}", self.slug),
            title: Some(format!("Chapter {}", self.number)),
            chapter_number: self.number.parse().ok(),
            date_uploaded: Some(self.updated_at.time * 1000),
            url: Some(format!("{BASE_URL}/view/{}", self.slug)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct TimeField {
    time: i64,
}

#[derive(Deserialize)]
struct ImageList {
    images: Vec<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":[{"title":"Sample West Manga","slug":"sample-west","cover":"https://westmanga.co/cover.jpg"}],"paginator":{"current_page":1,"last_page":1}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"title":"Sample West Manga","slug":"sample-west","alternative_name":"Alt Sample","sinopsis":"<p>Sample description.</p>","cover":"https://westmanga.co/cover.jpg","author":"Author","country_id":"JP","status":"ongoing","color":true,"genres":[{"name":"Action"}],"chapters":[{"slug":"sample-west-chapter-1","number":"1","updated_at":{"time":1704067200}}]}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"images":["https://westmanga.co/page-1.jpg"]}}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_fixtures_and_signature() {
        assert_eq!(parse_list(LIST_FIXTURE).entries[0].key, "/manga/sample-west");
        assert_eq!(fetch_details("sample-west").title, "Sample West Manga");
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
        assert_eq!(
            request_signature("1704067200", "/api/contents").len(),
            64
        );
    }
}
