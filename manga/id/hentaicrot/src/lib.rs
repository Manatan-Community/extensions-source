use manatan_extension::{
    abi::ExtensionResult, export_manga_source, source::MangaSource, CatalogItem, ItemStatus,
    MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: HentaiCrot = HentaiCrot;
const BASE_URL: &str = "https://hentaicrot.com";

struct HentaiCrot;

impl MangaSource for HentaiCrot {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document_or_fixture(
            &page_url(BASE_URL, page),
            LIST_FIXTURE,
        )))
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
                entries: vec![parse_details(
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            page_url(BASE_URL, page)
        } else {
            let page_path = if page > 1 {
                format!("page/{page}/")
            } else {
                String::new()
            };
            format!("{BASE_URL}/{page_path}?s={}", url::query_escape(query))
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Chapter 1".to_string()),
            url: Some(absolute_url(&key)),
            chapter_number: Some(1.0),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_pages(&fetch_document_or_fixture(
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
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    Some(key),
                )),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn page_url(base: &str, page: u64) -> String {
    if page > 1 {
        format!("{}/page/{page}/", base.trim_end_matches('/'))
    } else {
        base.to_string()
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<article")
            .skip(1)
            .filter(|chunk| chunk.contains("blog-entry") || chunk.contains("search-entry"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "entry-title", "href")
                    .or_else(|| html::attr_after(chunk, "<h2", "href"))
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                let title = html::text_between(chunk, "entry-title", "</a>")
                    .or_else(|| html::text_between(chunk, "<h2", "</h2>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Manga".into()));
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: html::attr_after(chunk, "thumbnail", "src")
                        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
                        .or_else(|| html::attr_after(chunk, "<img", "src"))
                        .map(|value| absolute_url(&value)),
                    url: Some(absolute_url(&key)),
                    language: Some("id".to_string()),
                    content_rating: Some("adult".to_string()),
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: body.contains("page-numbers") && body.contains("next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Hentai Crot".to_string()),
        cover: html::attr_after(body, "thumbnail", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        description: html::text_between(body, "entry-content", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: body
            .split("<li")
            .filter(|chunk| chunk.contains("meta-cat") || chunk.contains("meta-category"))
            .flat_map(|chunk| {
                chunk
                    .split("<a")
                    .skip(1)
                    .filter_map(|link| html::text_between(link, ">", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .collect::<Vec<_>>()
            })
            .collect(),
        status: ItemStatus::Completed,
        url: Some(absolute_url(&key)),
        language: Some("id".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("entry-content"))
        .chain(
            body.split("<div")
                .filter(|chunk| chunk.contains("entry-content")),
        )
        .flat_map(|chunk| chunk.split("<img").skip(1).collect::<Vec<_>>())
        .filter_map(|chunk| {
            html::attr(chunk, "data-src")
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|value| !value.is_empty() && !value.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            let path = &value[index + BASE_URL.len()..];
            return format!("/{}", path.trim_matches('/'));
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<article class="blog-entry"><div class="thumbnail"><img src="/cover.jpg"></div><h2 class="blog-entry-title"><a href="/sample/">Sample Hentai Crot</a></h2></article><ul class="page-numbers"><li><a class="next" href="/page/2/">Next</a></li></ul>
"#;
const DETAILS_FIXTURE: &str = r#"
<div id="content"><h1 class="entry-title">Sample Hentai Crot</h1><div class="thumbnail"><img src="/cover.jpg"></div><div class="entry-content"><p>Sample description.</p><img src="/page1.jpg"><img src="/page2.jpg"></div><li class="meta-cat"><a>Adult</a></li></div>
"#;
const PAGES_FIXTURE: &str = DETAILS_FIXTURE;
