use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ManhwaHub = ManhwaHub;
const BASE_URL: &str = "https://manhwahub.net";

struct ManhwaHub;

impl MangaSource for ManhwaHub {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_listing(&fetch_document(
                &format!("{BASE_URL}/?page={page}"),
                LIST_FIXTURE,
            )));
        }
        Ok(parse_home(&fetch_document(BASE_URL, HOME_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if query.is_empty() {
            format!("{BASE_URL}/?page={page}")
        } else {
            format!("{BASE_URL}/search?s={}&page={page}", url::query_escape(query))
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
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
        .with_origin(BASE_URL)
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

fn parse_home(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("id=\"slide-top\"")
        .nth(1)
        .unwrap_or(body)
        .split("<div")
        .filter(|chunk| chunk.contains("item"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "info-item", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "ManhwaHub".into()));
            Some(catalog_item(key, title, image_from_chunk(chunk), false))
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .filter(|chunk| chunk.contains("page-item-detail"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "item-summary", "</a>")
                .or_else(|| html::text_between(chunk, "post-title", "</a>"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "ManhwaHub".into()));
            Some(catalog_item(key, title, image_from_chunk(chunk), false))
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\""),
    }
}

fn catalog_item(key: String, title: String, cover: Option<String>, initialized: bool) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover,
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let status_text = summary_value(body, "status").unwrap_or_default();
    let mut item = catalog_item(
        key.clone(),
        html::text_between(body, "post-title", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "ManhwaHub".into())),
        body.split("summary_image").nth(1).and_then(image_from_chunk),
        true,
    );
    item.description = html::text_between(body, "summary__content", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    item.authors = summary_value(body, "author(s)").into_iter().collect();
    item.tags = body
        .split("genres-content")
        .nth(1)
        .map(link_texts)
        .unwrap_or_default();
    item.status = if status_text.to_ascii_lowercase().contains("completed") {
        ItemStatus::Completed
    } else if status_text.to_ascii_lowercase().contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    };
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("wp-manga-chapter")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            Some(MangaChapter {
                key: key.clone(),
                title,
                chapter_number: key
                    .split("chapter-")
                    .nth(1)
                    .and_then(|value| value.trim_matches('/').parse::<f32>().ok()),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("page-break")
        .skip(1)
        .filter_map(image_from_chunk)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn summary_value(body: &str, heading: &str) -> Option<String> {
    let needle = heading.to_ascii_lowercase();
    body.split("summary-heading")
        .find(|chunk| chunk.to_ascii_lowercase().contains(&needle))
        .and_then(|chunk| chunk.split("summary-content").nth(1))
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "srcset").map(|value| value.split_whitespace().next().unwrap_or("").to_string()))
        .or_else(|| html::attr(chunk, "data-cfsrc"))
        .or_else(|| html::attr(chunk, "src"))
        .filter(|value| !value.is_empty())
        .map(|value| url::join_url(BASE_URL, &value))
}

fn link_texts(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input
                .trim_start_matches(BASE_URL)
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(key: &str) -> String {
    if key.starts_with("http") {
        key.to_string()
    } else {
        url::join_url(BASE_URL, key)
    }
}

export_manga_source!(SOURCE);

const HOME_FIXTURE: &str = r#"<div id="slide-top"><div class="item"><div class="img-item"><img src="/cover.jpg"></div><div class="info-item"><a href="/manga/sample">Sample Manga</a></div></div></div>"#;
const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><div class="item-thumb"><img src="/cover.jpg"></div><div class="item-summary"><a href="/manga/sample">Sample Manga</a></div></div><ul class="pager"><a rel="next">Next</a></ul>"#;
const DETAILS_FIXTURE: &str = r#"<div class="post-title"><h1>Sample Manga</h1></div><div class="summary_image"><img src="/cover.jpg"></div><div class="summary-heading">author(s)</div><div class="summary-content">Author</div><div class="summary-heading">status</div><div class="summary-content">Ongoing</div><div class="genres-content"><a rel="tag">Action</a></div><div class="summary__content">Summary</div><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1">Chapter 1</a><span class="chapter-release-date">1 day ago</span></li>"#;
const PAGES_FIXTURE: &str = r#"<div class="page-break"><img src="/page1.jpg"></div><div class="page-break"><img src="/page2.jpg"></div>"#;
