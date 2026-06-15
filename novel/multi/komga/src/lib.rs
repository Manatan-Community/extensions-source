use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    novel,
    sdk::{
        SearchRequest,
        http::{HttpClient, base64_encode},
    },
    url,
};
use serde_json::Value;

const SOURCE: Komga = Komga;

struct Komga;

impl NovelSource for Komga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let Some(config) = KomgaConfig::from_request(&request) else {
            return Ok(Paged {
                entries: parse_series_list(&KomgaConfig::fixture(), SERIES_LIST_FIXTURE),
                has_next_page: false,
            });
        };
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort = if listing == "latest" {
            "lastModified,desc"
        } else {
            "name,asc"
        };
        let target = config.url(&format!(
            "api/v1/series?page={}&sort={}",
            page.saturating_sub(1),
            url::query_escape(sort)
        ));
        let body = config.get_or_fixture(&target, SERIES_LIST_FIXTURE);
        let entries = parse_series_list(&config, &body);
        Ok(Paged {
            has_next_page: !entries.is_empty(),
            entries,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let Some(config) = KomgaConfig::from_request(&request) else {
            return Ok(Paged {
                entries: parse_series_list(&KomgaConfig::fixture(), SERIES_LIST_FIXTURE),
                has_next_page: false,
            });
        };
        if let Some(key) = key_from_url(&config, query) {
            return Ok(Paged {
                entries: vec![fetch_details(&config, &key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = config.url(&format!(
            "api/v1/series?search={}&page={}",
            url::query_escape(query),
            page.saturating_sub(1)
        ));
        let body = config.get_or_fixture(&target, SERIES_LIST_FIXTURE);
        Ok(Paged {
            entries: parse_series_list(&config, &body),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = KomgaConfig::from_request(&request).unwrap_or_else(KomgaConfig::fixture);
        let key = novel::request_key(&request, "novel")
            .unwrap_or_else(|| "api/v1/series/sample".to_string());
        Ok(fetch_details(&config, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let config = KomgaConfig::from_request(&request).unwrap_or_else(KomgaConfig::fixture);
        let key = novel::request_key(&request, "novel")
            .unwrap_or_else(|| "api/v1/series/sample".to_string());
        Ok(fetch_chapters(&config, &series_id(&key)))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let config = KomgaConfig::from_request(&request).unwrap_or_else(KomgaConfig::fixture);
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "opds/v2/books/sample/pages/1".to_string());
        let body = config.get_or_fixture(&config.url(&key), CHAPTER_TEXT_FIXTURE);
        let html = normalize_reader_assets(&body, &config.url(&parent_path(&key)));
        Ok(NovelText {
            html: Some(html.clone()),
            text: Some(novel::cleanup_text(&html)),
            base_url: Some(config.base_url.clone()),
            css: Some(
                "body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string(),
            ),
            image_headers: novel::image_headers(&config.base_url),
            ..NovelText::default()
        })
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Series".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Recently Updated".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let Some(config) = KomgaConfig::from_request(&request) else {
            return Ok(Some(UrlResolveResult {
                search: Some(SearchRequest {
                    query: input.to_string(),
                    ..SearchRequest::default()
                }),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        };
        if let Some(key) = key_from_url(&config, input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&config, &key)),
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

#[derive(Clone)]
struct KomgaConfig {
    base_url: String,
    email: Option<String>,
    password: Option<String>,
}

impl KomgaConfig {
    fn from_request(request: &Value) -> Option<Self> {
        let prefs = request.get("preferences").unwrap_or(request);
        let raw_url = string_pref(prefs, "url").or_else(|| string_pref(prefs, "server_url"))?;
        let base_url = ensure_trailing_slash(&raw_url);
        if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
            return None;
        }
        Some(Self {
            base_url,
            email: string_pref(prefs, "email"),
            password: string_pref(prefs, "password"),
        })
    }

    fn fixture() -> Self {
        Self {
            base_url: "https://komga.local/".to_string(),
            email: None,
            password: None,
        }
    }

    fn client(&self) -> HttpClient {
        let mut client = HttpClient::browser()
            .with_desktop_user_agent()
            .with_referer(&self.base_url)
            .with_cookies_for(&self.base_url);
        if let (Some(email), Some(password)) = (&self.email, &self.password) {
            let token = base64_encode(format!("{email}:{password}").as_bytes());
            client = client.with_header("Authorization", format!("Basic {token}"));
        }
        client
    }

    fn url(&self, path: &str) -> String {
        url::join_url(&self.base_url, path)
    }

    fn get_or_fixture(&self, target: &str, fixture: &str) -> String {
        self.client()
            .get(target)
            .xhr()
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }
}

fn string_pref(prefs: &Value, key: &str) -> Option<String> {
    prefs
        .get(key)
        .and_then(|value| value.get("value").or(Some(value)))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn parse_series_list(config: &KomgaConfig, body: &str) -> Vec<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    root.get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|series| {
            let id = series.get("id").and_then(Value::as_str)?;
            Some(CatalogItem {
                key: format!("api/v1/series/{id}"),
                title: series
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Komga Series")
                    .to_string(),
                cover: Some(config.url(&format!("api/v1/series/{id}/thumbnail"))),
                url: Some(config.url(&format!("api/v1/series/{id}"))),
                language: Some("multi".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn fetch_details(config: &KomgaConfig, key: &str) -> CatalogItem {
    let id = series_id(key);
    let body = config.get_or_fixture(
        &config.url(&format!("api/v1/series/{id}")),
        SERIES_DETAILS_FIXTURE,
    );
    let root = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
    let metadata = root.get("metadata").unwrap_or(&Value::Null);
    let books_metadata = root.get("booksMetadata").unwrap_or(&Value::Null);
    let authors = books_metadata
        .get("authors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|author| author.get("role").and_then(Value::as_str) == Some("writer"))
        .filter_map(|author| {
            author
                .get("name")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect();
    CatalogItem {
        key: format!("api/v1/series/{id}"),
        title: root
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Komga Series")
            .to_string(),
        cover: Some(config.url(&format!("api/v1/series/{id}/thumbnail"))),
        authors,
        tags: metadata
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        description: books_metadata
            .get("summary")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        status: parse_status(metadata.get("status").and_then(Value::as_str)),
        url: Some(config.url(&format!("api/v1/series/{id}"))),
        language: Some("multi".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapters(config: &KomgaConfig, id: &str) -> Vec<NovelChapter> {
    let body = config.get_or_fixture(
        &config.url(&format!("api/v1/series/{id}/books?unpaged=true")),
        BOOKS_FIXTURE,
    );
    let root = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
    let books: Vec<_> = root
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut chapters = Vec::new();
    for book in books {
        let Some(book_id) = book.get("id").and_then(Value::as_str) else {
            continue;
        };
        let book_title = book
            .get("metadata")
            .and_then(|metadata| metadata.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("Book");
        let manifest = config.get_or_fixture(
            &config.url(&format!("opds/v2/books/{book_id}/manifest")),
            MANIFEST_FIXTURE,
        );
        let manifest = serde_json::from_str::<Value>(&manifest).unwrap_or(Value::Null);
        let reading_order = manifest
            .get("readingOrder")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let total = reading_order.len();
        for (index, page) in reading_order.iter().enumerate() {
            let Some(href) = page.get("href").and_then(Value::as_str) else {
                continue;
            };
            let key = if href.starts_with("opds/v2") {
                href.to_string()
            } else {
                format!("opds/v2{}", href.split("opds/v2").last().unwrap_or(href))
            };
            chapters.push(NovelChapter {
                key: key.clone(),
                title: Some(format!("{}/{} - {}", index + 1, total, book_title)),
                chapter_number: Some((index + 1) as f32),
                url: Some(config.url(&key)),
                language: Some("multi".to_string()),
                ..NovelChapter::default()
            });
        }
    }
    chapters
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.unwrap_or_default() {
        "ENDED" => ItemStatus::Completed,
        "ABANDONED" => ItemStatus::Cancelled,
        "HIATUS" => ItemStatus::Hiatus,
        _ => ItemStatus::Ongoing,
    }
}

fn normalize_reader_assets(body: &str, base: &str) -> String {
    body.replace("xlink:href=\"", "src=\"")
        .replace("href=\"", "src=\"")
        .replace(
            "src=\"/",
            &format!("src=\"{}", ensure_no_trailing_slash(base)),
        )
}

fn key_from_url(config: &KomgaConfig, input: &str) -> Option<String> {
    input
        .strip_prefix(&config.base_url)
        .map(|value| value.trim_start_matches('/').to_string())
        .filter(|value| value.starts_with("api/v1/series/"))
}

fn series_id(key: &str) -> String {
    key.trim_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("sample")
        .to_string()
}

fn parent_path(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default()
}

fn ensure_trailing_slash(value: &str) -> String {
    if value.ends_with('/') {
        value.to_string()
    } else {
        format!("{value}/")
    }
}

fn ensure_no_trailing_slash(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    request["listing"] = Value::String(listing.to_string());
    request
}

const SERIES_LIST_FIXTURE: &str = r#"{"content":[{"id":"sample","name":"Sample Series","metadata":{"status":"ONGOING","genres":["Novel"]},"booksMetadata":{"authors":[{"name":"Author","role":"writer"}],"summary":"Fixture summary."}}]}"#;
const SERIES_DETAILS_FIXTURE: &str = r#"{"id":"sample","name":"Sample Series","metadata":{"status":"ONGOING","genres":["Novel"]},"booksMetadata":{"authors":[{"name":"Author","role":"writer"}],"summary":"Fixture summary."}}"#;
const BOOKS_FIXTURE: &str = r#"{"content":[{"id":"book-1","metadata":{"title":"Book One"}}]}"#;
const MANIFEST_FIXTURE: &str =
    r#"{"toc":[],"readingOrder":[{"href":"/opds/v2/books/book-1/pages/1"}]}"#;
const CHAPTER_TEXT_FIXTURE: &str = r#"<html><body><p>Fixture Komga page.</p></body></html>"#;

export_novel_source!(SOURCE);
