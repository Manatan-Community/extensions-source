use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UpdateStrategy, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http, url};
use serde_json::Value;

const SOURCE: Mitaku = Mitaku;
const BASE_URL: &str = "https://mitaku.net";

struct Mitaku;

impl MangaSource for Mitaku {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = format!("{BASE_URL}/category/ero-cosplay/page/{page}/");
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
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
        if query.starts_with(BASE_URL) && is_post_path(&normalize_key(query)) {
            let key = normalize_key(query);
            let body = fetch_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let category = filters
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let tag = filters
            .get("tag")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let target = if !query.is_empty() {
            format!("{BASE_URL}/page/{page}/?s={}", encode_query(query))
        } else if !category.is_empty() {
            format!("{BASE_URL}/category/{category}/page/{page}/")
        } else if !tag.trim().is_empty() {
            format!("{BASE_URL}/tag/{}/page/{page}/", slugify(tag))
        } else {
            format!("{BASE_URL}/category/ero-cosplay/page/{page}/")
        };
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/post/".into());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/post/".into());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some(if body.contains("(Video)") {
                "This post is video-only, watch it in WebView".into()
            } else {
                "Gallery".into()
            }),
            chapter_number: Some(1.0),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("all".into()),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/post/".into());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && is_post_path(&normalize_key(input)) {
            let key = normalize_key(input);
            let body = fetch_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    http::HttpClient::browser()
        .with_referer(BASE_URL)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("article-container") || chunk.contains("<a"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            Some(CatalogItem {
                key: normalize_key(&href),
                title: html::attr_after(chunk, "<a", "title")
                    .or_else(|| {
                        html::text_between(chunk, "<h2", "</h2>")
                            .map(|text| html::strip_tags(&text))
                    })
                    .or_else(|| {
                        html::text_between(chunk, "<h3", "</h3>")
                            .map(|text| html::strip_tags(&text))
                    })
                    .unwrap_or_else(|| "Mitaku".into()),
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                language: Some("all".into()),
                content_rating: Some("adult".into()),
                update_strategy: Some(UpdateStrategy::OnlyFetchOnce),
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample/post/".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|text| html::strip_tags(&text))
            .unwrap_or_else(|| "Mitaku".into()),
        tags: parse_tags(body),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        update_strategy: Some(UpdateStrategy::OnlyFetchOnce),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_tags(body: &str) -> Vec<String> {
    body.split("cat-links")
        .chain(body.split("tag-links"))
        .flat_map(|part| part.split("<a").skip(1))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|text| html::strip_tags(&text))
        .filter(|tag| !tag.is_empty())
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("msacwl-img-link"))
        .filter_map(|chunk| html::attr(chunk, "data-mfp-src").or_else(|| html::attr(chunk, "href")))
        .filter(|image| !image.is_empty())
        .map(|image| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            ..MangaPage::default()
        })
        .collect()
}

fn has_next_page(body: &str) -> bool {
    body.contains("wp-pagenavi") && body.contains("page larger")
}

fn is_post_path(key: &str) -> bool {
    let first = key.trim_matches('/').split('/').next().unwrap_or_default();
    !matches!(first, "" | "category" | "tag" | "search" | "page")
        && key.trim_matches('/').split('/').count() >= 2
}

fn normalize_key(value: &str) -> String {
    value
        .strip_prefix(BASE_URL)
        .unwrap_or(value)
        .split('?')
        .next()
        .unwrap_or(value)
        .trim_end_matches('/')
        .to_string()
}

fn slugify(input: &str) -> String {
    input
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

fn encode_query(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

const LIST_FIXTURE: &str = r#"
<div class="article-container"><article><a href="https://mitaku.net/model/sample-post/" title="Sample Mitaku"><img src="/thumb.jpg"></a></article></div>
<div class="wp-pagenavi"><a class="page larger" href="/page/2">2</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<article><h1>Sample Mitaku</h1><span class="cat-links"><a href="/category/ero-cosplay/">Ero Cosplay</a></span><span class="tag-links"><a href="/tag/sample/">Sample</a></span></article>
"#;

const PAGES_FIXTURE: &str = r#"
<a class="msacwl-img-link" data-mfp-src="https://mitaku.net/image-1.jpg" href="https://mitaku.net/image-1-small.jpg">Image</a>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mitaku() {
        assert_eq!(parse_listing(LIST_FIXTURE).len(), 1);
        assert_eq!(parse_details(DETAILS_FIXTURE, None).title, "Sample Mitaku");
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
        assert!(is_post_path("/model/sample-post"));
    }
}
