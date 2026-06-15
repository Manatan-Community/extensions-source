use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Source = Source;
const BASE_URL: &str = "https://phenix-scans.com";
const API_URL: &str = "https://phenix-scans.com/api";

struct Source;

impl MangaSource for Source {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_top(LIST_FIXTURE));
        }
        let page = page(&request);
        if listing(&request) == "latest" {
            Ok(parse_latest(&fetch(
                &format!("{API_URL}/front/homepage?page={page}&section=latest&limit=12"),
                LATEST_FIXTURE,
            )))
        } else {
            Ok(parse_top(&fetch(
                &format!("{API_URL}/front/homepage?section=top"),
                LIST_FIXTURE,
            )))
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if let Some(key) = deeplink(query) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch(&details_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(parse_search(&fetch(
                &format!(
                    "{API_URL}/front/manga/search?query={}",
                    url::query_escape(query)
                ),
                SEARCH_FIXTURE,
            )));
        }
        Ok(parse_search(&fetch(
            &browse_url(page(&request), request.get("filters")),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch(&details_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch(&details_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/1".into());
        Ok(parse_pages(&fetch(&chapter_url_api(&key), PAGES_FIXTURE)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/manga/{}", slug(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (slug, number) = chapter_parts(&key);
            format!("{BASE_URL}/manga/{slug}/chapitre/{number}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = deeplink(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch(&details_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
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

fn fetch(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_top(body: &str) -> Paged<CatalogItem> {
    let root = json(body, LIST_FIXTURE);
    Paged {
        entries: root
            .get("top")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(item)
            .collect(),
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let root = json(body, LATEST_FIXTURE);
    let current = root
        .pointer("/pagination/currentPage")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let total = root
        .pointer("/pagination/totalPages")
        .and_then(Value::as_u64)
        .unwrap_or(current);
    Paged {
        entries: root
            .get("latest")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(item)
            .collect(),
        has_next_page: current < total,
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let root = json(body, SEARCH_FIXTURE);
    let current = root
        .pointer("/pagination/page")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let total = root
        .pointer("/pagination/totalPages")
        .and_then(Value::as_u64)
        .unwrap_or(current);
    Paged {
        entries: root
            .get("mangas")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(item)
            .collect(),
        has_next_page: current < total,
    }
}

fn item(value: Value) -> CatalogItem {
    let slug = str_field(&value, "slug").unwrap_or_else(|| "sample".into());
    CatalogItem {
        key: format!("/manga/{slug}"),
        title: str_field(&value, "title").unwrap_or_else(|| slug.clone()),
        cover: str_field(&value, "coverImage").map(|cover| url::join_url(API_URL, &cover)),
        url: Some(format!("{BASE_URL}/manga/{slug}")),
        language: Some("fr".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let root = json(body, DETAILS_FIXTURE);
    let manga_value = root.get("manga").cloned().unwrap_or_default();
    let slug = str_field(&manga_value, "slug")
        .unwrap_or_else(|| slug(&key.unwrap_or_else(|| "/manga/sample".into())));
    CatalogItem {
        key: format!("/manga/{slug}"),
        title: str_field(&manga_value, "title").unwrap_or_else(|| slug.clone()),
        cover: str_field(&manga_value, "coverImage").map(|cover| url::join_url(API_URL, &cover)),
        description: str_field(&manga_value, "synopsis"),
        status: match str_field(&manga_value, "status").as_deref() {
            Some("Ongoing") => ItemStatus::Ongoing,
            Some("Hiatus") => ItemStatus::Hiatus,
            Some("Completed") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/manga/{slug}")),
        language: Some("fr".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let root = json(body, DETAILS_FIXTURE);
    let slug = root
        .pointer("/manga/slug")
        .and_then(Value::as_str)
        .unwrap_or("sample");
    root.get("chapters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|chapter| chapter.get("price").and_then(Value::as_u64).unwrap_or(0) == 0)
        .filter_map(|chapter| {
            let number = chapter.get("number")?;
            let number_text = number
                .as_str()
                .map(ToString::to_string)
                .unwrap_or_else(|| number.to_string());
            Some(MangaChapter {
                key: format!("/manga/{slug}/{number_text}"),
                title: Some(format!("Chapter {number_text}")),
                chapter_number: number_text.parse().ok(),
                date_uploaded: str_field(&chapter, "createdAt").and_then(|date| {
                    manatan_shared::dates::parse_ymd(date.get(0..10).unwrap_or(&date))
                }),
                url: Some(format!("{BASE_URL}/manga/{slug}/chapitre/{number_text}")),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    json(body, PAGES_FIXTURE)
        .pointer("/chapter/images")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|image| image.as_str().map(ToString::to_string))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(API_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn browse_url(page: u64, filters: Option<&Value>) -> String {
    let mut params = vec![("limit", "18".to_string()), ("page", page.to_string())];
    for key in ["sort", "genre", "type", "status"] {
        if let Some(value) = filters
            .and_then(|f| f.get(key))
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            params.push((key, value.to_string()));
        }
    }
    format!(
        "{API_URL}/front/manga?{}",
        params
            .into_iter()
            .map(|(k, v)| format!("{k}={}", url::query_escape(&v)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn details_url(key: &str) -> String {
    format!("{API_URL}/front/manga/{}", slug(key))
}
fn chapter_url_api(key: &str) -> String {
    let (slug, number) = chapter_parts(key);
    format!("{API_URL}/front/manga/{slug}/chapter/{number}")
}
fn chapter_parts(key: &str) -> (String, String) {
    let parts = key.trim_matches('/').split('/').collect::<Vec<_>>();
    (
        parts.get(1).unwrap_or(&"sample").to_string(),
        parts.get(2).unwrap_or(&"1").to_string(),
    )
}
fn deeplink(input: &str) -> Option<String> {
    (input.starts_with(BASE_URL) && input.contains("/manga/")).then(|| {
        format!(
            "/manga/{}",
            input
                .split("/manga/")
                .nth(1)
                .unwrap_or("sample")
                .split('/')
                .next()
                .unwrap_or("sample")
        )
    })
}
fn slug(key: &str) -> String {
    key.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("sample")
        .to_string()
}
fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}
fn listing(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}
fn str_field(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(ToString::to_string)
}
fn json(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .or_else(|_| serde_json::from_str(fixture))
        .unwrap_or_else(|_| json!({}))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"top":[{"title":"Sample Phenix","coverImage":"cover.jpg","slug":"sample","status":"Ongoing"}]}"#;
const LATEST_FIXTURE: &str = r#"{"pagination":{"currentPage":1,"totalPages":1},"latest":[{"title":"Sample Phenix","coverImage":"cover.jpg","slug":"sample"}]}"#;
const SEARCH_FIXTURE: &str = r#"{"mangas":[{"title":"Sample Phenix","coverImage":"cover.jpg","slug":"sample"}],"pagination":{"page":1,"totalPages":1}}"#;
const DETAILS_FIXTURE: &str = r#"{"manga":{"title":"Sample Phenix","coverImage":"cover.jpg","slug":"sample","synopsis":"Summary","status":"Ongoing"},"chapters":[{"number":1,"createdAt":"2024-01-01T00:00:00.000Z","price":0}]}"#;
const PAGES_FIXTURE: &str = r#"{"chapter":{"images":["page1.jpg"]}}"#;
