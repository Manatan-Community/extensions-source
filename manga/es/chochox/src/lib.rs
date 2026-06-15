use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Chochox = Chochox;
const BASE_URL: &str = "https://chochox.com";
const NAME: &str = "ChoChoX";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct Chochox;

impl MangaSource for Chochox {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/porno/page/{page}"),
            LIST_FIXTURE,
        )))
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
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }

        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if query.is_empty() {
            selected_genre(&request)
                .filter(|genre| !genre.is_empty())
                .map(|genre| format!("{BASE_URL}/tag/{genre}/page/{page}"))
                .unwrap_or_else(|| format!("{BASE_URL}/porno/page/{page}"))
        } else {
            format!("{BASE_URL}/page/{page}?s={}", url::query_escape(query))
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
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let title = parse_details(&body, &key).title;
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some(title),
            chapter_number: Some(1.0),
            url: Some(absolute_url(&key)),
            language: Some(LANG.to_string()),
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
                    &key,
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: entry_blocks(body)
            .into_iter()
            .filter_map(|chunk| {
                let href = html::attr_after(&chunk, "popimg", "href")
                    .or_else(|| html::attr_after(&chunk, "<a", "href"))?;
                let title = html::attr_after(&chunk, "<img", "alt")
                    .or_else(|| {
                        html::text_between(&chunk, "<h2", "</h2>").map(|v| html::strip_tags(&v))
                    })
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&href).unwrap_or_else(|| NAME.to_string())
                    });
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_from_chunk(&chunk),
                    url: Some(absolute_url(&key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: body.contains("wp-pagenavi") && body.contains("current"),
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: html::attr_after(body, "property=\"og:title\"", "content")
            .or_else(|| {
                html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value))
            })
            .or_else(|| url::slug_from_url(key))
            .unwrap_or_else(|| NAME.to_string()),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| image_from_chunk(body)),
        tags: link_values(body, "/tag/"),
        status: ItemStatus::Completed,
        url: Some(absolute_url(key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("wp-content")
                || chunk.contains("post-imgs")
                || chunk.contains("data-src")
                || chunk.contains("data-lazy-src")
        })
        .filter_map(image_src)
        .filter(|image| !image.starts_with("data:"))
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

fn entry_blocks(body: &str) -> Vec<String> {
    body.split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("entry"))
        .map(|chunk| format!("<article{chunk}"))
        .collect()
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    chunk
        .split("<img")
        .nth(1)
        .and_then(image_src)
        .map(|value| absolute_url(&value))
}

fn image_src(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| srcset_first(html::attr(chunk, "srcset")))
        .or_else(|| html::attr(chunk, "data-cfsrc"))
        .or_else(|| html::attr(chunk, "src"))
}

fn srcset_first(value: Option<String>) -> Option<String> {
    value.and_then(|srcset| {
        srcset
            .split(',')
            .find_map(|candidate| candidate.split_whitespace().next().map(ToString::to_string))
    })
}

fn link_values(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn selected_genre(request: &Value) -> Option<&str> {
    request
        .get("filters")
        .and_then(|filters| filters.get("genre").or_else(|| filters.get("tag")))
        .and_then(Value::as_str)
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        )
    } else {
        format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

const LIST_FIXTURE: &str = r#"<article class="entry"><a class="popimg" href="https://chochox.com/sample"><img alt="Sample" src="https://chochox.com/sample.jpg"></a></article>"#;
const DETAILS_FIXTURE: &str = r#"<html><head><meta property="og:title" content="Sample"><meta property="og:image" content="https://chochox.com/sample.jpg"></head><body><div class="tax_box"><a href="https://chochox.com/tag/full-color">Full Color</a></div></body></html>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="wp-content"><p><img data-src="https://chochox.com/page-1.jpg"></p></div>"#;

export_manga_source!(SOURCE);
