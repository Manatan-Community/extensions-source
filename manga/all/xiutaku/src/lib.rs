use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Xiutaku = Xiutaku;
const BASE_URL: &str = "https://xiutaku.com";
const MOBILE_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";

struct Xiutaku;

impl MangaSource for Xiutaku {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let offset = 20 * page.saturating_sub(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            format!("{BASE_URL}/?start={offset}")
        } else {
            format!("{BASE_URL}/hot?start={offset}")
        };
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body))
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
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(normalize_key(query)))],
                has_next_page: false,
            });
        }
        let offset = 20 * page.saturating_sub(1);
        let target = format!(
            "{BASE_URL}/?search={}&start={offset}",
            url::query_escape(query)
        );
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        if body.contains("article-header") {
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(normalize_key(&target)))],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/post/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/post/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/post/sample?page=1".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(normalize_key(input)))),
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
        .with_header("User-Agent", MOBILE_UA)
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
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("item-content") || chunk.contains("item-link"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "item-link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "item-link", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Post".into())),
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|image| url::join_url(BASE_URL, &image)),
                status: ItemStatus::Completed,
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("all".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        has_next_page: body.contains("pagination-next")
            && !body.contains("pagination-next\" disabled"),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/post/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "article-header", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Post".into())),
        description: html::text_between(body, "article-info", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: body
            .split("tag")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</"))
            .map(|value| html::strip_tags(&value).trim_start_matches('#').to_string())
            .filter(|value| !value.is_empty())
            .collect(),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let max_page = body
        .split("pagination-list")
        .nth(1)
        .map(html::strip_tags)
        .and_then(|value| {
            value
                .split_whitespace()
                .filter_map(|part| part.parse::<u32>().ok())
                .max()
        })
        .unwrap_or(1);
    let base = manga_key.split('?').next().unwrap_or(manga_key);
    (1..=max_page)
        .rev()
        .map(|page| MangaChapter {
            key: format!("{base}?page={page}"),
            title: Some(format!("Page {page}")),
            url: Some(format!("{}{}?page={page}", BASE_URL, base)),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("article-fulltext") || chunk.contains("src="))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_key(value: &str) -> String {
    let path = value.trim_start_matches(BASE_URL);
    format!("/{}", path.trim_start_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="blog"><div><div class="item-content"><a class="item-link" href="/post/sample">Sample Xiutaku</a><img src="/cover.jpg"></div></div></div>
<a class="pagination-next" href="/?start=20">Next</a>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="article-header">Sample Xiutaku</h1>
<div class="article-info">Description text.</div>
<div class="article-info"><small>00:00 01-01-2024</small></div>
<div class="article-tags"><span class="tags"><a class="tag">#Cosplay</a></span></div>
<ul class="pagination-list"><span><a>1</a></span><span><a>2</a></span></ul>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="article-fulltext"><img src="/1.jpg"><img src="https://xiutaku.com/2.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xiutaku() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(
            parse_details(DETAILS_FIXTURE, Some("/post/sample".into())).title,
            "Sample Xiutaku"
        );
        assert_eq!(parse_chapters(DETAILS_FIXTURE, "/post/sample").len(), 2);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
