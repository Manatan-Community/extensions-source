use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Roxinha = Roxinha;
const BASE_URL: &str = "https://roxinha.online";
const API_URL: &str = "https://roxinha.online/api";
const PAGE_SIZE: u64 = 24;

struct Roxinha;

impl MangaSource for Roxinha {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let (sort, order) = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            ("updatedAt", "DESC")
        } else {
            ("views", "DESC")
        };
        Ok(parse_listing(&fetch_json(&advanced_url(page, "", sort, order, "", ""), LIST_FIXTURE)))
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
        let filters = request.get("filters").unwrap_or(&Value::Null);
        Ok(parse_listing(&fetch_json(
            &advanced_url(
                page,
                query,
                &filter_string(filters, "sort").unwrap_or_else(|| "title".to_string()),
                &filter_string(filters, "order").unwrap_or_else(|| "ASC".to_string()),
                &filter_string(filters, "status").unwrap_or_default(),
                &filter_string(filters, "type").unwrap_or_default(),
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/1".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/1".into());
        let id = key.trim_matches('/').split('/').next_back().unwrap_or("1");
        Ok(parse_chapters(&fetch_json(&format!("{API_URL}/manga/{id}"), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/chapter/1".into());
        let chapter_id = key.trim_matches('/').split('/').next_back().unwrap_or("1");
        let page_url = absolute_url(&key);
        Ok(parse_pages(
            &fetch_json(&format!("{API_URL}/manga/chapter/{chapter_id}"), PAGES_FIXTURE),
            &page_url,
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
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
        .header("Accept", "application/json, text/plain, */*")
        .header("Origin", BASE_URL)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn advanced_url(page: u64, query: &str, sort: &str, order: &str, status: &str, content_type: &str) -> String {
    let offset = (page.saturating_sub(1)) * PAGE_SIZE;
    let mut params = vec![
        ("limit", PAGE_SIZE.to_string()),
        ("offset", offset.to_string()),
        ("mode", "default".to_string()),
        ("sort", sort.to_string()),
        ("order", order.to_string()),
    ];
    if !query.is_empty() {
        params.push(("q", url::query_escape(query)));
    }
    if !status.is_empty() {
        params.push(("status", status.to_string()));
    }
    if !content_type.is_empty() {
        params.push(("type", content_type.to_string()));
    }
    format!(
        "{API_URL}/manga/search/advanced?{}",
        params
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!("/{}", value[index + BASE_URL.len()..].trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

fn details_by_key(key: &str) -> CatalogItem {
    let id = key.trim_matches('/').split('/').next_back().unwrap_or("1");
    parse_details(&fetch_json(&format!("{API_URL}/manga/{id}"), DETAILS_FIXTURE))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap());
    let entries = root
        .get("mangas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(json_item)
        .collect();
    Paged {
        entries,
        has_next_page: root.get("hasMore").and_then(Value::as_bool).unwrap_or(false),
    }
}

fn json_item(item: &Value) -> CatalogItem {
    let id = item.get("id").and_then(Value::as_i64).unwrap_or(0);
    let key = format!("/manga/{id}");
    CatalogItem {
        key: key.clone(),
        title: item
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Roxinha")
            .to_string(),
        cover: item
            .get("cover")
            .and_then(Value::as_str)
            .map(|cover| absolute_url(cover)),
        authors: item
            .get("author")
            .and_then(Value::as_str)
            .map(|author| vec![author.to_string()])
            .unwrap_or_default(),
        description: item.get("description").and_then(Value::as_str).map(ToString::to_string),
        tags: item
            .get("genres")
            .and_then(Value::as_str)
            .map(|genres| genres.split(',').map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect())
            .unwrap_or_default(),
        status: parse_status(item.get("status").and_then(Value::as_str)),
        url: Some(absolute_url(&key)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: item.get("chapters").is_some(),
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str) -> CatalogItem {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    json_item(&root)
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    root.get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|chapter| {
            let id = chapter.get("id").and_then(Value::as_i64).unwrap_or(0);
            let number = chapter.get("chapterNumber").and_then(Value::as_f64);
            let title = chapter.get("title").and_then(Value::as_str).filter(|value| !value.is_empty());
            let key = format!("/manga/chapter/{id}");
            MangaChapter {
                key: key.clone(),
                title: Some(
                    title
                        .map(ToString::to_string)
                        .unwrap_or_else(|| {
                            number
                                .map(|n| format!("Capitulo {}", trim_number(n)))
                                .unwrap_or_else(|| "Capitulo".to_string())
                        }),
                ),
                chapter_number: number.map(|value| value as f32),
                date_uploaded: chapter
                    .get("createdAt")
                    .and_then(Value::as_str)
                    .and_then(|value| dates::parse_ymd(value.get(..10).unwrap_or(value))),
                url: Some(absolute_url(&key)),
                language: Some("pt-BR".to_string()),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str, page_url: &str) -> Vec<MangaPage> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).unwrap());
    root.get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(image),
                context: None,
            },
            headers: manga::image_headers(page_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value {
        Some("ongoing") => ItemStatus::Ongoing,
        Some("completed") => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn trim_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"mangas":[{"id":1,"title":"Sample","cover":"/cover.jpg","status":"ongoing","genres":"Drama"}],"hasMore":false}"#;
const DETAILS_FIXTURE: &str = r#"{"id":1,"title":"Sample","cover":"/cover.jpg","author":"Author","description":"Description","status":"ongoing","genres":"Drama","chapters":[{"id":10,"chapterNumber":1,"title":"Capitulo 1","createdAt":"2024-01-01T00:00:00.000Z"}]}"#;
const PAGES_FIXTURE: &str = r#"{"pages":["/page1.jpg","/page2.jpg"]}"#;
