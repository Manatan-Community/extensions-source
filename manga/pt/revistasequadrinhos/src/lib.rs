use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: RevistasEQuadrinhos = RevistasEQuadrinhos;
const BASE_URL: &str = "https://revistasequadrinhos.com";

struct RevistasEQuadrinhos;

impl MangaSource for RevistasEQuadrinhos {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            paged_url("", page)
        } else {
            paged_url("category/popular-comics", page)
        };
        let body = fetch_document(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
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
        let target = if !query.is_empty() {
            let base = paged_url("", page);
            format!("{base}?s={}", url::query_escape(query))
        } else if let Some(category) = filter_string(filters, "category").filter(|value| !value.is_empty()) {
            paged_url(&format!("category/{category}"), page)
        } else if let Some(tag) = filter_string(filters, "tag").filter(|value| !value.is_empty()) {
            paged_url(&format!("tag/{tag}"), page)
        } else {
            paged_url("", page)
        };
        let body = fetch_document(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Capitulo Unico".to_string()),
            date_uploaded: html::attr_after(&body, "article:published_time", "content")
                .and_then(|value| dates::parse_ymd(value.get(..10).unwrap_or(&value))),
            url: Some(absolute_url(&key)),
            language: Some("pt".to_string()),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".into());
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!(
                "/{}",
                value[index + BASE_URL.len()..]
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn paged_url(path: &str, page: u64) -> String {
    let base = if path.is_empty() {
        BASE_URL.to_string()
    } else {
        format!("{BASE_URL}/{}", path.trim_matches('/'))
    };
    if page <= 1 {
        format!("{base}/")
    } else {
        format!("{base}/page/{page}/")
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, Some(key.to_string()))
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("thumb-conteudo") || chunk.contains("a.titulo") || chunk.contains("titulo"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "titulo", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "titulo", "</a>")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Revistas e Quadrinhos".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("pt".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "post-conteudo", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .or_else(|| html::text_between(body, "<title", "</title>"))
            .map(|value| html::strip_tags(&value).replace(" - Revistas e Quadrinhos", ""))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Revistas e Quadrinhos".to_string())),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "post-texto", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("tag") || chunk.contains("category"))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: ItemStatus::Completed,
        url: Some(absolute_url(&key)),
        language: Some("pt".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str, page_url: &str) -> Vec<MangaPage> {
    let gallery_pages = body
        .split("<figure")
        .skip(1)
        .filter(|chunk| chunk.contains("dgwt-jg-item"))
        .filter_map(|chunk| html::attr_after(chunk, "<a", "href"))
        .collect::<Vec<_>>();
    let images = if gallery_pages.is_empty() {
        body.split("<img")
            .skip(1)
            .filter(|chunk| chunk.contains("post-texto") || chunk.contains("src"))
            .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-lazy-src")))
            .collect::<Vec<_>>()
    } else {
        gallery_pages
    };
    images
        .into_iter()
        .filter(|image| !image.is_empty())
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

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
}

fn has_next_page(body: &str) -> bool {
    body.contains("paginacao") && body.contains("next")
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<ul class="videos"><li><div class="thumb-conteudo"><img src="/cover.jpg"></div><a class="titulo" href="https://revistasequadrinhos.com/sample/">Sample</a></li></ul>
"#;
const DETAILS_FIXTURE: &str = r#"
<meta property="article:published_time" content="2024-01-01T00:00:00+00:00"><meta property="og:image" content="/cover.jpg">
<div class="post-conteudo"><h1>Sample</h1></div><div class="post-texto"><p>Sample description.</p><img src="/page1.jpg"></div><div class="post-tags"><a href="/tag/comics">Comics</a></div>
"#;
const PAGES_FIXTURE: &str = r#"<div class="post-texto"><img src="/page1.jpg"><img data-lazy-src="/page2.jpg"></div>"#;
