use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaPill = MangaPill;
const BASE_URL: &str = "https://mangapill.com";

struct MangaPill;

impl MangaSource for MangaPill {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged { entries: parse_cards(LIST_FIXTURE), has_next_page: false });
        }
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest { format!("{BASE_URL}/chapters") } else { format!("{BASE_URL}/") };
        Ok(Paged {
            entries: parse_cards(&fetch_document(&target, LIST_FIXTURE)),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), Some(key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_document(&search_url(page, query, request.get("filters")), LIST_FIXTURE);
        Ok(Paged {
            has_next_page: body.contains("btn btn-sm") && body.contains("Next"),
            entries: parse_cards(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapters/sample-1".to_string());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
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
            return Ok(Some(UrlResolveResult {
                item: input.contains("/manga/").then(|| parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(normalize_key(input)))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
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
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn search_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    let mut params = vec![format!("page={page}"), format!("q={}", url::query_escape(query))];
    for key in ["status", "type"] {
        let value = filter(filters, key);
        if !value.is_empty() {
            params.push(format!("{key}={}", url::query_escape(&value)));
        }
    }
    for genre in filter(filters, "genre").split(',').map(str::trim).filter(|value| !value.is_empty()) {
        params.push(format!("genre={}", url::query_escape(genre)));
    }
    format!("{BASE_URL}/search?{}", params.join("&"))
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("/manga/") && (chunk.contains("line-clamp-2") || chunk.contains("data-src")))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "/manga/", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "line-clamp-2", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let detail = body.split("div.container").nth(1).unwrap_or(body);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "text-2xl", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: image_attr(detail).map(|image| absolute_url(&image)),
        description: html::text_between(detail, "<p", "</p>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        tags: link_texts(body, "genre"),
        status: parse_status(&html::strip_tags(detail)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/chapters/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::strip_tags(chunk).trim().to_string();
            Some(MangaChapter {
                key: key.clone(),
                title: Some(if title.is_empty() { "Chapter".to_string() } else { title }),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: absolute_url(&image), context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn link_texts(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .map(|chunk| html::strip_tags(chunk))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("publishing") {
        ItemStatus::Ongoing
    } else if lower.contains("finished") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
}

fn filter(filters: Option<&Value>, id: &str) -> String {
    filters.and_then(|value| value.get(id)).and_then(Value::as_str).unwrap_or_default().trim().to_string()
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(".com/") {
            return format!("/{}", value[index + 5..].trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"
<div><a href="/manga/sample"><img data-src="/cover.jpg"><div class="line-clamp-2">Sample Manga</div></a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="container"><div><img data-src="/cover.jpg"></div><div><h1>Sample Manga</h1><p>Summary</p><a href="/search?genre=Action">Action</a><div>Publishing</div></div></div>
<div id="chapters"><div><a href="/chapters/sample-1">Chapter 1</a></div></div>
"#;
const PAGES_FIXTURE: &str = r#"<picture><img data-src="/page1.jpg"></picture>"#;

export_manga_source!(SOURCE);
