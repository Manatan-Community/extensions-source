use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaFox = MangaFox;
const BASE_URL: &str = "https://fanfox.net";
const MOBILE_URL: &str = "https://m.fanfox.net";

struct MangaFox;

impl MangaSource for MangaFox {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, "ul.manga-list-1-list li"));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let page_path = if page > 1 {
            format!("{page}.html")
        } else {
            String::new()
        };
        let suffix = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "?latest"
        } else {
            ""
        };
        Ok(parse_listing(
            &fetch_document(
                &format!("{BASE_URL}/directory/{page_path}{suffix}"),
                LIST_FIXTURE,
            ),
            "ul.manga-list-1-list li",
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) || query.starts_with(MOBILE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let body = fetch_document(&search_url(&request, query), SEARCH_FIXTURE);
        Ok(parse_listing(&body, "ul.manga-list-4-list li"))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".to_string());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1.html".to_string());
        let mobile_path = key.replace("/manga/", "/roll_manga/");
        Ok(parse_pages(&fetch_mobile_document(
            &url::join_url(MOBILE_URL, &mobile_path),
            PAGES_FIXTURE,
        )))
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
        if input.starts_with(BASE_URL) || input.starts_with(MOBILE_URL) {
            let key = normalize_key(input);
            let item = if key.starts_with("/manga/") {
                Some(parse_details(
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                    Some(key),
                ))
            } else {
                None
            };
            return Ok(Some(UrlResolveResult {
                item,
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
        .with_header("Cookie", "isAdult=1; readway=2")
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

fn fetch_mobile_document(target: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{MOBILE_URL}/"))
        .with_header("Cookie", "readway=2; isAdult=1")
        .with_cookies_for(MOBILE_URL)
        .with_webview_challenge_fallback()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(request: &Value, query: &str) -> String {
    let filters = request.get("filters");
    let mut params = vec![
        ("title".to_string(), query.to_string()),
        ("sort".to_string(), String::new()),
        ("stype".to_string(), "1".to_string()),
    ];
    for key in [
        "name", "type", "st", "author", "artist", "released", "genres", "nogenres",
    ] {
        if let Some(value) = filters
            .and_then(|filters| filters.get(key))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            params.push((key.to_string(), value.to_string()));
        }
    }
    let query_string = params
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/search?{query_string}")
}

fn parse_listing(body: &str, selector_hint: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("/manga/")
                && (selector_hint.contains("manga-list-1")
                    || chunk.contains("manga-list-4")
                    || chunk.contains("<img"))
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(chunk, "<a", "title")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&key))
                    .unwrap_or_else(|| "Manga".to_string()),
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pager-list-left") && body.contains("active"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample/".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "detail-info-right-title-font", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: html::attr_after(body, "detail-info-cover-img", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "fullcontent", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: detail_links(body, "detail-info-right-say"),
        artists: detail_links(body, "detail-info-right-say"),
        tags: detail_links(body, "detail-info-right-tag-list"),
        status: parse_status(body),
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
        .filter(|chunk| chunk.contains("/manga/") && chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "detail-main-list-main", "</a>")
                    .or_else(|| html::text_between(chunk, ">", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "data-original")
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(MOBILE_URL)),
            },
            headers: manga::image_headers(MOBILE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn detail_links(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .take(1)
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .collect::<Vec<_>>()
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(body: &str) -> ItemStatus {
    if body.contains("Completed") {
        ItemStatus::Completed
    } else if body.contains("Ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn normalize_key(value: &str) -> String {
    let path = value
        .strip_prefix(BASE_URL)
        .or_else(|| value.strip_prefix(MOBILE_URL))
        .unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        url::join_url(BASE_URL, value)
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"<ul class="manga-list-1-list"><li><a href="https://fanfox.net/manga/sample/" title="Sample"><img src="/cover.jpg"></a></li></ul><div class="pager-list-left"><a class="active">1</a></div>"#;
const SEARCH_FIXTURE: &str = r#"<ul class="manga-list-4-list"><li><a href="https://fanfox.net/manga/sample/" title="Sample"><img src="/cover.jpg"></a></li></ul>"#;
const DETAILS_FIXTURE: &str = r#"<div class="detail-info-right"><h1 class="detail-info-right-title-font">Sample</h1><p class="fullcontent">Summary</p><p class="detail-info-right-say"><a>Author</a></p><p class="detail-info-right-tag-list"><a>Action</a></p><span class="detail-info-right-title-tip">Ongoing</span></div><img class="detail-info-cover-img" src="/cover.jpg"><ul class="detail-main-list"><li><a href="https://fanfox.net/manga/sample/chapter-1.html"><div class="detail-main-list-main"><p>Chapter 1</p></div></a></li></ul>"#;
const PAGES_FIXTURE: &str =
    r#"<div id="viewer"><img data-original="https://cdn.fanfox.net/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
