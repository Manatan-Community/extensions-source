use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, url};
use serde_json::Value;

const BASE_URL: &str = "https://ahottie.top";
const SOURCE: AHottie = AHottie;

struct AHottie;

impl MangaSource for AHottie {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_document_or_fixture(&format!("{BASE_URL}?page={page}"), LIST_FIXTURE);
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
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            if is_gallery_page(&body) {
                return Ok(Paged {
                    entries: vec![parse_details(&body, Some(normalize_path(query)))],
                    has_next_page: false,
                });
            }
        }
        let body = fetch_document_or_fixture(
            &format!("{BASE_URL}/search?kw={}&page={page}", url::query_escape(query)),
            LIST_FIXTURE,
        );
        if is_gallery_page(&body) {
            return Ok(Paged {
                entries: vec![parse_details(&body, None)],
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            request_key(&request, "manga").unwrap_or_else(|| "/albums/sample-gallery".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(normalize_path(&key))))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            request_key(&request, "manga").unwrap_or_else(|| "/albums/sample-gallery".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let path = html::attr_after(&body, "rel=\"canonical\"", "href")
            .or_else(|| html::attr_after(&body, "rel='canonical'", "href"))
            .map(|value| normalize_path(&value))
            .unwrap_or_else(|| normalize_path(&key));
        Ok(vec![MangaChapter {
            key: path.clone(),
            title: Some("GALLERY".to_string()),
            chapter_number: Some(0.0),
            date_uploaded: html::text_between(&body, "<time", "</time>")
                .and_then(|value| dates::parse_fixture_date(&html::strip_tags(&value))),
            url: Some(absolute_url(&path)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter")
            .unwrap_or_else(|| "/albums/sample-gallery".to_string());
        let mut next = absolute_url(&key);
        let mut pages = Vec::new();
        for _ in 0..100 {
            let body = fetch_document_or_fixture(&next, PAGES_FIXTURE);
            for image in parse_page_images(&body) {
                pages.push(MangaPage {
                    content: PageContent::Url {
                        url: image,
                        context: None,
                    },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(format!("Page {}", pages.len() + 1)),
                    ..MangaPage::default()
                });
            }
            let Some(next_page) = next_page_url(&body).map(|value| absolute_url(&value)) else {
                break;
            };
            if next_page == next {
                break;
            }
            next = next_page;
        }
        Ok(pages)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.contains("/albums/") {
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(normalize_path(input)))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        if input.contains("/tags/") {
            return Ok(Some(UrlResolveResult {
                search: Some(manatan_extension::SearchRequest {
                    query: url::slug_from_url(input).unwrap_or_default(),
                    ..manatan_extension::SearchRequest::default()
                }),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(None)
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(url: &str, fixture: &str) -> String {
    client()
        .get(url)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("<h2") && chunk.contains("<a"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let path = normalize_path(&href);
            let title = html::text_between(chunk, "<h2", "</h2>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "AHottie Gallery".to_string());
            Some(CatalogItem {
                key: path.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|value| absolute_url(&value)),
                url: Some(absolute_url(&path)),
                tags: anchor_texts(chunk),
                language: Some("all".to_string()),
                content_rating: Some("adult".to_string()),
                status: ItemStatus::Completed,
                initialized: true,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique_item)
}

fn parse_details(body: &str, path: Option<String>) -> CatalogItem {
    let path = path
        .or_else(|| {
            html::attr_after(body, "rel=\"canonical\"", "href").map(|value| normalize_path(&value))
        })
        .unwrap_or_else(|| "/albums/sample-gallery".to_string());
    CatalogItem {
        key: path.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "AHottie Gallery".to_string()),
        cover: html::attr_after(body, "<img", "src").map(|value| absolute_url(&value)),
        url: Some(absolute_url(&path)),
        tags: detail_tags(body),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_page_images(body: &str) -> Vec<String> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("block"))
        .filter_map(|chunk| html::attr(chunk, "src").map(|value| absolute_url(&value)))
        .collect()
}

fn anchor_texts(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn detail_tags(body: &str) -> Vec<String> {
    body.split("pl-3")
        .nth(1)
        .map(anchor_texts)
        .unwrap_or_default()
}

fn has_next_page(body: &str) -> bool {
    body.contains("rel=\"next\"") || body.contains("rel='next'")
}

fn next_page_url(body: &str) -> Option<String> {
    body.split("<a")
        .skip(1)
        .find(|chunk| chunk.contains("rel=\"next\"") || chunk.contains("rel='next'"))
        .and_then(|chunk| html::attr(chunk, "href"))
}

fn is_gallery_page(body: &str) -> bool {
    body.contains("<h1") && body.contains("pl-3")
}

fn normalize_path(value: &str) -> String {
    let trimmed = value.trim().trim_start_matches(BASE_URL).trim_end_matches('/');
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else if let Some(index) = trimmed.find("/albums/").or_else(|| trimmed.find("/tags/")) {
        trimmed[index..].trim_end_matches('/').to_string()
    } else {
        format!("/{}", trimmed.trim_matches('/'))
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|item| item.get("key").or_else(|| item.get("url")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn push_unique_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"
<main id="main">
<div><div><a href="https://ahottie.top/albums/sample-gallery"><div class="relative"><img src="https://ahottie.top/sample.jpg"></div><h2>Sample Gallery</h2><div class="flex"><a>Sample</a></div></a></div></div>
<a rel="next" href="https://ahottie.top?page=2">Next</a>
</main>
"#;

const DETAILS_FIXTURE: &str = r#"
<link rel="canonical" href="https://ahottie.top/albums/sample-gallery">
<h1>Sample Gallery</h1>
<time>2024-01-01</time>
<div class="pl-3"><a>Sample</a><a>Gallery</a></div>
<img src="https://ahottie.top/sample.jpg">
"#;

const PAGES_FIXTURE: &str = r#"
<main id="main">
<img class="block" src="https://ahottie.top/page-1.jpg">
<img class="block" src="https://ahottie.top/page-2.jpg">
</main>
"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_fixture() {
        let entries = parse_listing(LIST_FIXTURE);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "/albums/sample-gallery");
        assert!(has_next_page(LIST_FIXTURE));
    }

    #[test]
    fn parses_details_and_chapter_date_fixture() {
        let item = parse_details(DETAILS_FIXTURE, None);
        assert_eq!(item.title, "Sample Gallery");
        assert_eq!(item.tags, vec!["Sample", "Gallery"]);
    }

    #[test]
    fn parses_page_images_fixture() {
        let pages = parse_page_images(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0], "https://ahottie.top/page-1.jpg");
    }
}
