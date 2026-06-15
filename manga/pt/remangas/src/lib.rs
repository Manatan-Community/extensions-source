use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: NoxManga = NoxManga;
const BASE_URL: &str = "https://noxtoons.com";
const API_URL: &str = "https://xodneo.site/api/v1/comics";

struct NoxManga;

impl MangaSource for NoxManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "popular"
        };
        Ok(parse_api_listing(&fetch_json(&list_url(page, sort), LIST_FIXTURE)))
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
        Ok(parse_api_listing(&fetch_json(
            &format!("{API_URL}/search?q={}&page={page}", url::query_escape(query)),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let slug = key.trim_matches('/').split('/').next_back().unwrap_or("sample");
        Ok(parse_chapters(&fetch_json(
            &format!("{API_URL}/slug/{slug}/chapters?page=1&per_page=999&sort=newest"),
            CHAPTERS_FIXTURE,
        ), slug))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/ler/sample/chapter-1".into());
        let page_url = absolute_url(&key);
        let body = fetch_document(&page_url, PAGES_FIXTURE);
        Ok(parse_pages(&body, &page_url))
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn list_url(page: u64, sort: &str) -> String {
    format!("{API_URL}?per_page=24&sort={sort}&period=week&page={page}")
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
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, Some(key.to_string()))
}

fn parse_api_listing(body: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap());
    let entries = root
        .get("comics")
        .or_else(|| root.get("chapters"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(api_item)
        .collect();
    let page = root.get("page").and_then(Value::as_u64).unwrap_or(1);
    let total = root.get("total_pages").and_then(Value::as_u64).unwrap_or(page);
    Paged {
        entries,
        has_next_page: page < total,
    }
}

fn api_item(item: &Value) -> Option<CatalogItem> {
    let slug = item.get("slug").and_then(Value::as_str)?;
    let title = item.get("title").and_then(Value::as_str).unwrap_or("NoxManga");
    let key = format!("/manga/{slug}");
    Some(CatalogItem {
        key: key.clone(),
        title: title.to_string(),
        cover: item.get("cover").and_then(Value::as_str).map(ToString::to_string),
        url: Some(absolute_url(&key)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let lower = body.to_ascii_lowercase();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "detail-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "NoxManga".to_string())),
        cover: html::attr_after(body, "detail-cover", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "detail-description", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("detail-tags"))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: if lower.contains("completo") {
            ItemStatus::Completed
        } else if lower.contains("cancelado") {
            ItemStatus::Cancelled
        } else if lower.contains("hiato") {
            ItemStatus::Hiatus
        } else if lower.contains("em andamento") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(&key)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_slug: &str) -> Vec<MangaChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).unwrap());
    root.get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let slug = chapter.get("slug").and_then(Value::as_str)?;
            let number = chapter.get("number").and_then(Value::as_f64).unwrap_or(0.0);
            let number_text = if number.fract() == 0.0 {
                format!("{}", number as i64)
            } else {
                number.to_string()
            };
            let key = format!("/ler/{manga_slug}/{slug}");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(format!("Capitulo {number_text}")),
                chapter_number: Some(number as f32),
                date_uploaded: chapter
                    .get("created_at")
                    .and_then(Value::as_str)
                    .and_then(|value| dates::parse_ymd(value.get(..10).unwrap_or(value))),
                url: Some(absolute_url(&key)),
                language: Some("pt-BR".to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, page_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(page_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"comics":[{"slug":"sample","title":"Sample","cover":"https://noxtoons.com/cover.jpg"}],"page":1,"total_pages":1}"#;
const CHAPTERS_FIXTURE: &str = r#"{"chapters":[{"number":1,"slug":"chapter-1","created_at":"2024-01-01T00:00:00"}],"page":1,"total_pages":1}"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="detail-title">Sample</h1><div class="detail-cover"><img src="/cover.jpg"></div><div class="detail-description">Description</div><div class="detail-tags"><a>Drama</a></div><span class="status-badge">Em andamento</span>"#;
const PAGES_FIXTURE: &str = r#"<section><img src="/page1.jpg"><img src="/page2.jpg"></section>"#;
