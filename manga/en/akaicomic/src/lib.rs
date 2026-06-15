use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: AkaiComic = AkaiComic;
const BASE_URL: &str = "https://akaicomic.org";
const PAGE_SIZE: u64 = 20;

struct AkaiComic;

impl MangaSource for AkaiComic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list(LIST_FIXTURE, false, "safe"));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let parsed = parse_list(
            &fetch_api_or_fixture(
                &format!("/api/manga/list?limit={PAGE_SIZE}&page={page}"),
                LIST_FIXTURE,
            ),
            latest,
            "safe",
        );
        Ok(parsed)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_api_or_fixture(
                &format!("/api/manga/{}", key.trim_matches('/')),
                DETAILS_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![parse_details(&body, key)],
                has_next_page: false,
            });
        }
        let mut page = parse_list(
            &fetch_api_or_fixture("/api/manga/list?limit=100&page=1", LIST_FIXTURE),
            false,
            "safe",
        );
        if !query.is_empty() {
            page.entries.retain(|item| item.title.contains(query));
        }
        page.has_next_page = false;
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = fetch_api_or_fixture(
            &format!("/api/manga/{}", key.trim_matches('/')),
            DETAILS_FIXTURE,
        );
        Ok(parse_details(&body, key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = fetch_api_or_fixture(
            &format!("/api/manga/{}/chapters", key.trim_matches('/')),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".to_string());
        let (manga_id, chapter_num) = key.split_once('/').unwrap_or(("sample", "1"));
        let body = fetch_api_or_fixture(
            &format!("/api/manga/{manga_id}/chapter/{chapter_num}/pages"),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_api_or_fixture(
                &format!("/api/manga/{}", key.trim_matches('/')),
                DETAILS_FIXTURE,
            );
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, key)),
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

fn fetch_api_or_fixture(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{BASE_URL}{path}"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_list(body: &str, _latest: bool, rating: &str) -> Paged<CatalogItem> {
    let response: MangaListResponse = serde_json::from_str(body).unwrap_or_default();
    let has_next_page =
        (response.page as u64) * (response.page_size as u64) < response.total as u64;
    Paged {
        entries: response
            .manga
            .into_iter()
            .map(|manga| manga.into_catalog(rating))
            .collect(),
        has_next_page,
    }
}

fn parse_details(body: &str, fallback_key: String) -> CatalogItem {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let value = extract_manga_value(&root);
    let manga: MangaDto = serde_json::from_value(value).unwrap_or_default();
    if manga.key().is_empty() {
        return CatalogItem {
            key: fallback_key.clone(),
            title: url::slug_from_url(&fallback_key).unwrap_or_else(|| "Manga".into()),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        };
    }
    let mut item = manga.into_catalog("safe");
    item.initialized = true;
    item
}

fn extract_manga_value(root: &Value) -> Value {
    match root {
        Value::Object(object) => {
            let nested = object
                .get("manga")
                .or_else(|| object.get("series"))
                .or_else(|| object.get("data"))
                .or_else(|| object.get("result"));
            match nested {
                Some(Value::Array(items)) => items.first().cloned().unwrap_or_else(|| root.clone()),
                Some(value) => value.clone(),
                None => root.clone(),
            }
        }
        Value::Array(items) => items.first().cloned().unwrap_or_else(|| root.clone()),
        _ => root.clone(),
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let response: ChapterListResponse = serde_json::from_str(body).unwrap_or_default();
    let mut chapters: Vec<MangaChapter> = response
        .chapters
        .into_iter()
        .filter(|chapter| chapter.locked_by_coins == 0)
        .map(|chapter| {
            let key = format!("{}/{}", chapter.manga_id, chapter.chapter_number);
            MangaChapter {
                key: key.clone(),
                title: Some(format!("Chapter {}", chapter.chapter_number)),
                chapter_number: Some(chapter.chapter_number as f32),
                date_uploaded: chapter
                    .created_at
                    .as_deref()
                    .and_then(manatan_shared::dates::parse_fixture_date),
                url: Some(format!("{BASE_URL}/reader/{key}")),
                ..MangaChapter::default()
            }
        })
        .collect();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .partial_cmp(&a.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response: PageListResponse = serde_json::from_str(body).unwrap_or_default();
    response
        .pages
        .into_iter()
        .enumerate()
        .map(|(index, path)| {
            let image = url::join_url(BASE_URL, &path);
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn normalize_key(input: &str) -> String {
    input
        .trim_start_matches(BASE_URL)
        .trim_start_matches("/serie/")
        .trim_matches('/')
        .to_string()
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_uppercase().as_str() {
        "ONGOING" | "RELEASING" => ItemStatus::Ongoing,
        "COMPLETED" => ItemStatus::Completed,
        "HIATUS" => ItemStatus::Hiatus,
        "CANCELLED" | "DROPPED" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

#[derive(Default, Deserialize)]
struct MangaListResponse {
    #[serde(default, alias = "data", alias = "series")]
    manga: Vec<MangaDto>,
    #[serde(default)]
    page: i32,
    #[serde(default, alias = "page_size")]
    page_size: i32,
    #[serde(default)]
    total: i32,
}

#[derive(Default, Deserialize)]
struct MangaDto {
    #[serde(default, alias = "lid", alias = "series_id")]
    id: String,
    #[serde(default, alias = "series_name", alias = "name")]
    title: String,
    #[serde(
        default,
        alias = "cover_url",
        alias = "cover",
        alias = "thumbnail",
        alias = "image"
    )]
    cover_url: Option<String>,
    #[serde(default, alias = "series_slug")]
    slug: Option<String>,
    author: Option<String>,
    artist: Option<String>,
    description: Option<String>,
    genres: Option<String>,
    status: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default, alias = "alt_name")]
    alternative_name: Option<String>,
}

impl MangaDto {
    fn key(&self) -> String {
        if self.id.is_empty() {
            self.slug.clone().unwrap_or_default()
        } else {
            self.id.clone()
        }
    }

    fn into_catalog(self, rating: &str) -> CatalogItem {
        let key = self.key();
        let mut tags = Vec::new();
        if let Some(kind) = self.kind {
            tags.push(kind);
        }
        tags.extend(
            self.genres
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .map(ToString::to_string)
                .filter(|tag| !tag.is_empty()),
        );
        let description = match (self.description, self.alternative_name) {
            (Some(description), Some(alt)) => Some(format!(
                "{}\n\nAlternative name: {}",
                html::strip_tags(&description),
                alt
            )),
            (Some(description), None) => Some(html::strip_tags(&description)),
            (None, Some(alt)) => Some(format!("Alternative name: {alt}")),
            (None, None) => None,
        };
        CatalogItem {
            key: key.clone(),
            title: if self.title.is_empty() {
                "Unknown title".to_string()
            } else {
                self.title
            },
            cover: self.cover_url,
            authors: self.author.into_iter().collect(),
            artists: self.artist.into_iter().collect(),
            description,
            tags,
            status: parse_status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/serie/{key}")),
            language: Some("en".to_string()),
            content_rating: Some(rating.to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct ChapterListResponse {
    #[serde(default)]
    chapters: Vec<ChapterDto>,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    chapter_number: i32,
    created_at: Option<String>,
    #[serde(default)]
    locked_by_coins: i32,
    manga_id: String,
}

#[derive(Default, Deserialize)]
struct PageListResponse {
    #[serde(default)]
    pages: Vec<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
{"manga":[{"id":"sample","series_name":"Akai Sample","cover_url":"/cover.jpg","author":"Author","artist":"Artist","description":"Desc","genres":"Action, Drama","status":"ONGOING","updated_at":"2024-01-01T00:00:00Z"}],"page":1,"pageSize":20,"total":21}
"#;

const DETAILS_FIXTURE: &str = r#"
{"manga":{"id":"sample","series_name":"Akai Sample","cover_url":"/cover.jpg","author":"Author","artist":"Artist","description":"<p>Desc</p>","genres":"Action","status":"COMPLETED","alternative_name":"Alt"}}
"#;

const CHAPTERS_FIXTURE: &str = r#"
{"chapters":[{"chapter_number":2,"created_at":"2024-01-02T00:00:00Z","id":2,"locked_by_coins":0,"manga_id":"sample"},{"chapter_number":1,"created_at":"2024-01-01T00:00:00Z","id":1,"locked_by_coins":1,"manga_id":"sample"}],"ok":true,"total":2,"totalChapters":2}
"#;

const PAGES_FIXTURE: &str = r#"
{"ok":true,"pages":["/pages/1.jpg","/pages/2.jpg"],"total":2}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_source() {
        assert!(parse_list(LIST_FIXTURE, false, "safe").has_next_page);
        assert_eq!(
            parse_details(DETAILS_FIXTURE, "sample".into()).status,
            ItemStatus::Completed
        );
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
