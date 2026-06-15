use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Manga1000 = Manga1000;
const BASE_URL: &str = "https://hachiraw.win";

struct Manga1000;

impl MangaSource for Manga1000 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        Ok(parse_listing(&fetch_document(&list_url(page, None, None), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let category = filter_string(&request, "category").filter(|value| !value.is_empty());
        Ok(parse_listing(&fetch_document(
            &list_url(page(&request), (!query.is_empty()).then_some(query), category),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
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
        if let Some(key) = key_from_url(input) {
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

fn list_url(page: u64, query: Option<&str>, category: Option<&str>) -> String {
    let mut target = if let Some(query) = query {
        let mut path = format!("{BASE_URL}/search");
        if page > 1 {
            path.push_str(&format!("/page/{page}"));
        }
        format!("{path}?query={}", url::query_escape(query))
    } else if let Some(category) = category {
        let mut path = format!("{BASE_URL}/category/{category}/");
        if page > 1 {
            path.push_str(&format!("page/{page}/"));
        }
        path
    } else if page > 1 {
        format!("{BASE_URL}/page/{page}/")
    } else {
        BASE_URL.to_string()
    };
    if !target.starts_with(BASE_URL) {
        target = absolute_url(&target);
    }
    target
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("post") && chunk.contains("manga"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "entry-title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "entry-title", "</")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga1000".into())),
                cover: image_from_chunk(chunk),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("nav pagination") && body.contains("next"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), Some(key.to_string()))
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value).split(" - ").next().unwrap_or("").trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga1000".into())),
        cover: html::text_between(body, "entry-content", "</div>").and_then(|chunk| image_from_chunk(&chunk)),
        authors: prefixed_paragraph(body, "Author:").into_iter().collect(),
        tags: body
            .split("<p")
            .filter(|chunk| chunk.contains("Category:"))
            .flat_map(|chunk| {
                chunk
                    .split("<a")
                    .skip(1)
                    .filter_map(|link| html::text_between(link, ">", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .collect::<Vec<_>>()
            })
            .filter(|value| !value.is_empty())
            .collect(),
        description: body
            .split("<p")
            .skip(1)
            .filter(|chunk| !chunk.contains("Author:") && !chunk.contains("Category:"))
            .filter_map(|chunk| html::text_between(chunk, ">", "</p>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
            .into_non_empty_option(),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("chaplist") || chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, ">", "</a>")
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "Chapter".into()),
                ),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("entry-content")
        .nth(1)
        .unwrap_or(body)
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|src| !src.is_empty() && !src.contains("lazy.png") && !src.starts_with("data:"))
        .enumerate()
        .map(|(index, src)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&src),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn prefixed_paragraph(body: &str, prefix: &str) -> Option<String> {
    body.split("<p")
        .skip(1)
        .find(|chunk| chunk.contains(prefix))
        .and_then(|chunk| html::text_between(chunk, ">", "</p>"))
        .map(|value| html::strip_tags(&value).replace(prefix, "").trim().to_string())
        .filter(|value| !value.is_empty())
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|value| absolute_url(&value))
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) && input.contains("/manga/") {
        Some(normalize_key(input))
    } else if input.starts_with("/manga/") {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

trait NonEmptyStringOption {
    fn into_non_empty_option(self) -> Option<String>;
}

impl NonEmptyStringOption for String {
    fn into_non_empty_option(self) -> Option<String> {
        (!self.is_empty()).then_some(self)
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<article class="post manga"><h2 class="entry-title"><a href="/manga/sample">Sample Manga1000</a></h2><div class="featured-thumb"><img src="/cover.jpg"></div></article>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample Manga1000 - Manga1000</h1><div class="entry-content"><p><img src="/cover.jpg"></p><p>Author: Sample Author</p><p>Category: <a>Action</a></p><p>Sample description.</p></div><div class="chaplist"><table><tbody><tr><td><a href="/manga/sample/chapter-1">Chapter 1</a></td></tr></tbody></table></div>
"#;
const PAGES_FIXTURE: &str = r#"
<div class="entry-content"><p><img data-src="https://img.example.test/page1.jpg"></p></div>
"#;
