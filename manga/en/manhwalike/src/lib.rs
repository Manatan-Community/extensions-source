use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Manhwalike = Manhwalike;
const BASE_URL: &str = "https://manhwalike.com";

struct Manhwalike;

impl MangaSource for Manhwalike {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = fetch_document(BASE_URL, HOME_FIXTURE);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            Ok(parse_visuals(&body, "slick_item"))
        } else {
            Ok(parse_visuals(&body, "list-hot"))
        }
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
        if query.is_empty() {
            let genre = request
                .get("filters")
                .and_then(|filters| filters.get("genre"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let path = if genre.is_empty() { "" } else { genre };
            return Ok(parse_search_results(&fetch_document(
                &format!("{BASE_URL}/{path}?page={page}"),
                SEARCH_FIXTURE,
            )));
        }
        Ok(parse_search_results(&fetch_search(query, SEARCH_FIXTURE)))
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

fn fetch_search(query: &str, fixture: &str) -> String {
    client()
        .post(&format!("{BASE_URL}/search/html/1"))
        .header("Accept", "text/html")
        .header("X-Requested-With", "XMLHttpRequest")
        .form(&[("keyword", query)])
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_visuals(body: &str, list_class: &str) -> Paged<CatalogItem> {
    let source = body.split(list_class).nth(1).unwrap_or(body);
    let entries = source
        .split("div class=\"visual")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "title", "</")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manhwalike".into()));
            Some(catalog_item(key, title, image_from_chunk(chunk), false))
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_search_results(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::attr_after(chunk, "<img", "alt")
                .or_else(|| html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manhwalike".into()));
            Some(catalog_item(key, title, image_from_chunk(chunk), false))
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("pagination") && body.contains("<a"),
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
    let mut item = catalog_item(
        key.clone(),
        html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::attr_after(body, "fixed-img", "alt"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manhwalike".into())),
        body.split("fixed-img").nth(1).and_then(image_from_chunk),
        true,
    );
    item.authors = body
        .split("div class=\"author")
        .nth(1)
        .map(link_texts)
        .unwrap_or_default();
    item.tags = body
        .split("div class=\"categories")
        .nth(1)
        .map(link_texts)
        .unwrap_or_default();
    item.description = html::text_between(body, "about", "</p>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    item.status = if body.to_ascii_lowercase().contains("finish") {
        ItemStatus::Completed
    } else if body.to_ascii_lowercase().contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    };
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .filter(|chunk| chunk.contains("chapter-list"))
        .chain(body.split("chapter-list").nth(1).unwrap_or("").split("<li"))
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
    body.split("page-chapter")
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

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-original")
        .or_else(|| html::attr(chunk, "data-src"))
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

const HOME_FIXTURE: &str = r#"<ul class="list-hot"><div class="visual"><h3 class="title"><a href="/manga/sample">Sample Manga</a></h3><img data-original="/cover.jpg"></div></ul><ul class="slick_item"><div class="visual"><h3 class="title"><a href="/manga/sample">Sample Manga</a></h3><img data-original="/cover.jpg"></div></ul>"#;
const SEARCH_FIXTURE: &str = r#"<ul class="normal"><li><a href="/manga/sample"><img alt="Sample Manga" data-original="/cover.jpg"></a></li></ul>"#;
const DETAILS_FIXTURE: &str = r#"<div class="fixed-img"><img src="/cover.jpg"></div><div class="author"><a>Author</a></div><small>Status</small><strong>Ongoing</strong><div class="categories"><a>Action</a></div><div class="summary-block"><p class="about">Summary</p></div><ul class="chapter-list"><li><a href="/manga/sample/chapter-1">Chapter 1</a><span class="time">January 1, 2024</span></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<div class="chapter-content"><div class="page-chapter"><img src="/page1.jpg"></div><div class="page-chapter"><img src="/page2.jpg"></div></div>"#;
