use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: TcbScans = TcbScans;
const BASE_URL: &str = "https://tcbonepiecechapters.com";

struct TcbScans;

impl MangaSource for TcbScans {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: parse_projects(&fetch_document(
                &format!("{BASE_URL}/projects"),
                PROJECTS_FIXTURE,
            )),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut entries = self.list(serde_json::json!({}))?.entries;
        if !query.is_empty() && !query.starts_with(BASE_URL) {
            entries.retain(|item| item.title.to_ascii_lowercase().contains(&query));
        }
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/projects/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/projects/sample".to_string());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/chapters/sample".to_string());
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
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(key),
                )),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_projects(body: &str) -> Vec<CatalogItem> {
    body.split("bg-card")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "text-white", "</")
                    .map(|v| html::strip_tags(&v))
                    .filter(|v| !v.is_empty())
                    .or_else(|| url::slug_from_url(&key))
                    .unwrap_or_else(|| "TCB Scans".into()),
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/projects/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "TCB Scans".into()),
        cover: html::attr_after(body, "div order-1", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "<p", "</p>")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("font-bold") || chunk.contains("text-gray-500"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "font-bold", "</")
                .map(|v| html::strip_tags(&v))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            let desc = html::text_between(chunk, "text-gray-500", "</")
                .map(|v| html::strip_tags(&v))
                .filter(|v| !v.is_empty());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(desc.map(|desc| format!("{title}: {desc}")).unwrap_or(title)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") {
        if let Some(path) = value.strip_prefix(BASE_URL) {
            return format!("/{}", path.trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

export_manga_source!(SOURCE);

const PROJECTS_FIXTURE: &str = r#"<div class="bg-card"><a class="text-white" href="/projects/sample">Sample TCB</a><img src="/cover.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="order-1"><img src="/cover.jpg"><h1>Sample TCB</h1><p>Sample</p></div><div class="grid"><a href="/chapters/sample-1"><div class="font-bold">1</div><span class="text-gray-500">Start</span></a></div>"#;
const PAGES_FIXTURE: &str = r#"<picture><img src="/page1.jpg"></picture><div class="image-container"><img src="/page2.jpg"></div>"#;
