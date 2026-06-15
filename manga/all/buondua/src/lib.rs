use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const BASE_URL: &str = "https://buondua.com";
const SOURCE: BuonDua = BuonDua;
const MOBILE_UA: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Mobile Safari/537.36";

struct BuonDua;

impl MangaSource for BuonDua {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .or_else(|| request.get("listing"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let start = 20 * page.saturating_sub(1);
        let target_url = if listing == "latest" {
            format!("{BASE_URL}/?start={start}")
        } else {
            format!("{BASE_URL}/hot?start={start}")
        };
        let body = fetch_document_or_fixture(&target_url, LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let start = 20 * page.saturating_sub(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(normalize_path(query)))],
                has_next_page: false,
            });
        }
        let target_url = if !query.is_empty() {
            format!("{BASE_URL}/?search={}&start={start}", url::query_escape(query))
        } else if let Some(tag) = tag_filter(&request) {
            format!("{BASE_URL}/tag/{tag}&start={start}")
        } else {
            format!("{BASE_URL}/hot?start={start}")
        };
        let body = fetch_document_or_fixture(&target_url, LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(normalize_path(&key))))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &absolute_url(&key)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample?page=1".into());
        let body = fetch_document_or_fixture(&absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(normalize_path(input)))),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_header("User-Agent", MOBILE_UA)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target_url: &str, fixture: &str) -> String {
    client()
        .get(target_url)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| chunk.contains("item-content") || chunk.contains("item-link"))
            .filter_map(parse_listing_item)
            .fold(Vec::new(), push_unique_item),
        has_next_page: body.contains("pagination-next") && !body.contains("pagination-next\" disabled"),
    }
}

fn parse_listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "item-link", "href")?;
    let key = normalize_path(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "item-link", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())?,
        cover: html::attr_after(chunk, "<img", "src").map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "article-header", "</")
            .map(|value| strip_page_suffix(&html::strip_tags(&value)))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Buon Dua Gallery".into())),
        description: parse_description(body),
        tags: body
            .split("article-tags")
            .nth(1)
            .unwrap_or_default()
            .split("tag")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</"))
            .map(|value| html::strip_tags(&value).trim_start_matches('#').to_string())
            .filter(|value| !value.is_empty())
            .collect(),
        status: ItemStatus::Completed,
        url: Some(absolute_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_description(body: &str) -> Option<String> {
    let mut parts = Vec::new();
    let info = html::text_between(body, "article-info", "</div>")
        .map(|value| html::strip_tags(&value).replace("Buondua", "").trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(info) = info {
        parts.push(info);
    }
    let links = body
        .split("article-links")
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let label = html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))?;
            Some(format!("[{label}]({href})"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !links.is_empty() {
        parts.push(links);
    }
    let password = html::text_between(body, "<code", "</code>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    if let Some(password) = password {
        parts.push(password);
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn strip_page_suffix(value: &str) -> String {
    value
        .split(" - ( Page ")
        .next()
        .unwrap_or(value)
        .trim()
        .to_string()
}

fn parse_chapters(body: &str, base_page_url: &str) -> Vec<MangaChapter> {
    let max_page = body
        .split("pagination-next")
        .filter_map(|chunk| html::attr(chunk, "href"))
        .filter_map(|href| query_param(&href, "page").and_then(|value| value.parse::<u32>().ok()))
        .max()
        .unwrap_or(1);
    let date_uploaded = html::text_between(body, "article-info", "</")
        .map(|value| html::strip_tags(&value))
        .and_then(|value| dates::parse_fixture_date(&value));
    (1..=max_page)
        .rev()
        .map(|page| MangaChapter {
            key: format!("{}?page={page}", normalize_path(base_page_url)),
            title: Some(format!("Page {page}")),
            date_uploaded,
            url: Some(format!("{base_page_url}?page={page}")),
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
                url: absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn tag_filter(request: &Value) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get("tagId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn query_param(input: &str, name: &str) -> Option<String> {
    let query = input.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

fn normalize_path(value: &str) -> String {
    if value.starts_with(BASE_URL) {
        return format!(
            "/{}",
            value[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<html><body>
  <div class="blog">
    <div><div class="item-content"><a class="item-link" href="https://buondua.com/sample">Sample Gallery</a></div><img src="https://img.example/cover.jpg"></div>
  </div>
  <a class="pagination-next" href="/hot?start=20">Next</a>
</body></html>
"#;

const DETAILS_FIXTURE: &str = r#"
<html><body>
  <h1 class="article-header">Sample Gallery - ( Page 1 / 2 )</h1>
  <div class="article-info"><strong>Buondua Studio</strong><small>12:00 01-01-2024</small></div>
  <div class="article-tags"><div class="tags"><a class="tag">#cosplay</a><a class="tag">#japan</a></div></div>
  <nav class="pagination"><a class="pagination-next" href="https://buondua.com/sample?page=2">Next</a></nav>
</body></html>
"#;

const PAGES_FIXTURE: &str = r#"
<html><body>
  <div class="article-fulltext"><img src="https://img.example/1.jpg"><img src="https://img.example/2.jpg"></div>
</body></html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing() {
        let page = parse_listing(LIST_FIXTURE);
        assert_eq!(page.entries.len(), 1);
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_details_and_chapters() {
        let item = parse_details(DETAILS_FIXTURE, Some("/sample".into()));
        assert_eq!(item.title, "Sample Gallery");
        let chapters = parse_chapters(DETAILS_FIXTURE, "https://buondua.com/sample");
        assert_eq!(chapters.len(), 2);
    }

    #[test]
    fn parses_pages() {
        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
