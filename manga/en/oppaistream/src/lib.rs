use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: OppaiStream = OppaiStream;
const BASE_URL: &str = "https://read.oppai.stream";
const CDN_URL: &str = "https://myspacecat.pictures";
const SEARCH_LIMIT: u64 = 36;

struct OppaiStream;

impl MangaSource for OppaiStream {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "uploaded"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document(&search_url(page, "", Some(order), None), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let slug = query.split("m=").nth(1).and_then(|part| part.split('&').next()).unwrap_or_default();
            if !slug.is_empty() {
                return Ok(Paged {
                    entries: vec![parse_details(
                        &fetch_document(&format!("{BASE_URL}/manhwa?m={slug}"), DETAILS_FIXTURE),
                        Some(format!("/manhwa?m={slug}")),
                    )],
                    has_next_page: false,
                });
            }
        }
        let filters = request.get("filters");
        let order = filter(filters, "sort");
        Ok(parse_listing(&fetch_document(&search_url(page, query, order.as_deref(), filters), LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manhwa?m=sample".to_string());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manhwa?m=sample".to_string());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/page?m=sample&c=1".to_string());
        let full = absolute_url(&key);
        let slug = query_param(&full, "m").unwrap_or_else(|| "sample".to_string());
        let chapter = query_param(&full, "c").unwrap_or_else(|| "1".to_string());
        Ok(parse_pages(&fetch_document(&format!("{CDN_URL}/manhwa/im.php?f-m={slug}&c={chapter}"), PAGES_FIXTURE)))
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
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key))),
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

fn search_url(page: u64, query: &str, order: Option<&str>, filters: Option<&Value>) -> String {
    let mut params = vec![
        format!("text={}", url::query_escape(query)),
        format!("page={page}"),
        format!("limit={SEARCH_LIMIT}"),
    ];
    if let Some(order) = order.filter(|value| !value.is_empty()) {
        params.push(format!("order={}", url::query_escape(order)));
    }
    if let Some(genres) = filter(filters, "genres").filter(|value| !value.is_empty()) {
        params.push(format!("genres={}", url::query_escape(&genres)));
    }
    if let Some(blacklist) = filter(filters, "blacklist").filter(|value| !value.is_empty()) {
        params.push(format!("blacklist={}", url::query_escape(&blacklist)));
    }
    format!("{BASE_URL}/api-search.php?{}", params.join("&"))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("div class=\"in-grid")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let href = if href.contains("/fw?to=") {
                decode_fw(&href)
            } else {
                href
            };
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "man-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Oppai Stream".to_string()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged { has_next_page: entries.len() as u64 >= SEARCH_LIMIT, entries }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manhwa?m=sample".to_string());
    let info = body.split("manhwa-info-in").nth(1).unwrap_or(body);
    let raw_title = html::text_between(info, "<h1", "</h1>").map(|value| html::strip_tags(&value)).unwrap_or_default();
    let author = info.split("red").nth(1).and_then(|chunk| html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)));
    CatalogItem {
        key: key.clone(),
        title: raw_title.split("By").next().unwrap_or(&raw_title).trim().to_string(),
        cover: html::attr_after(body, "cover-img", "src").map(|image| absolute_url(&image)),
        authors: author.clone().into_iter().collect(),
        artists: author.into_iter().collect(),
        tags: info.split("genres").nth(1).map(link_texts).unwrap_or_default(),
        description: html::text_between(info, "description", "</").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body
        .split("sort-chapters")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<h4", "</h4>")
                    .or_else(|| html::text_between(chunk, "<a", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: absolute_url(&image), context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn link_texts(chunk: &str) -> Vec<String> {
    chunk
        .split("<a")
        .skip(1)
        .filter_map(|part| html::text_between(part, "<a", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn filter(filters: Option<&Value>, key: &str) -> Option<String> {
    filters.and_then(|filters| filters.get(key)).and_then(Value::as_str).map(ToString::to_string)
}

fn query_param(input: &str, key: &str) -> Option<String> {
    input.split('?').nth(1)?.split('&').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == key).then(|| value.to_string())
    })
}

fn decode_fw(value: &str) -> String {
    let Some(encoded) = value.split("/fw?to=").nth(1) else {
        return value.to_string();
    };
    encoded.replace("%3A", ":").replace("%2F", "/").replace("%3F", "?").replace("%3D", "=").replace("%26", "&")
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        url::join_url(BASE_URL, value)
    }
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="in-grid"><a href="/manhwa?m=sample"><img class="read-cover" src="/cover.jpg"><h3 class="man-title">Sample</h3></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<img class="cover-img" src="/cover.jpg"><div class="manhwa-info-in"><h1>Sample By <a class="red">Author</a></h1><div class="genres"><h5>Drama</h5></div><div class="description">Summary</div></div><div class="sort-chapters"><a href="/page?m=sample&c=1"><div><h4>Chapter 1</h4></div><h6>1 day ago</h6></a></div>"#;
const PAGES_FIXTURE: &str = r#"<img src="https://myspacecat.pictures/page1.jpg">"#;
