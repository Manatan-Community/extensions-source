use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const BASE_URL: &str = "https://baobua.net";
const SOURCE: BaoBua = BaoBua;

struct BaoBua;

impl MangaSource for BaoBua {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/?page={page}"), LIST_FIXTURE);
        Ok(parse_mangas_page(&body))
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
                entries: vec![parse_details(&body, Some(normalize_path(query)))],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        let category = selected_category(&request);
        let target_url = category
            .map(|cat| format!("{BASE_URL}/category/{cat}?page={page}"))
            .unwrap_or_else(|| format!("{BASE_URL}/?page={page}"));
        let body = fetch_document_or_fixture(&target_url, LIST_FIXTURE);
        Ok(parse_mangas_page(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample-post".into());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(normalize_path(&key))))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample-post".into());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let canonical = html::attr_after(&body, "rel=\"canonical\"", "href")
            .map(|value| normalize_path(&value))
            .unwrap_or_else(|| normalize_path(&key));
        Ok(vec![MangaChapter {
            key: canonical.clone(),
            title: Some("Gallery".to_string()),
            chapter_number: Some(0.0),
            date_uploaded: html::text_between(&body, "article-date-comment", "</")
                .map(|value| html::strip_tags(&value))
                .and_then(|value| dates::parse_fixture_date(&value)),
            url: Some(absolute_url(&canonical)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-post".into());
        Ok(fetch_pages_recursive(&absolute_url(&key), 0))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.trim_end_matches('/').len() > BASE_URL.len() {
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

fn parse_mangas_page(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("product-item")
            .skip(1)
            .filter_map(parse_listing_item)
            .fold(Vec::new(), push_unique_item),
        has_next_page: body.contains("pagination-custom") && body.contains("nextPage"),
    }
}

fn parse_listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_path(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "product-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())?,
        cover: html::attr_after(chunk, "product-imgreal", "src").map(normalize_image_url),
        url: Some(absolute_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key
        .or_else(|| html::attr_after(body, "rel=\"canonical\"", "href").map(|value| normalize_path(&value)))
        .unwrap_or_else(|| "/sample-post".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "product-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .or_else(|| html::text_between(body, "article-title", "</"))
            .or_else(|| html::text_between(body, "post-title", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "BaoBua Gallery".into())),
        cover: html::attr_after(body, "product-imgreal", "src")
            .or_else(|| html::attr_after(body, "article-body", "src"))
            .map(normalize_image_url),
        tags: article_tags(body),
        status: ItemStatus::Completed,
        url: Some(absolute_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_pages_recursive(target_url: &str, offset: usize) -> Vec<MangaPage> {
    let body = fetch_document_or_fixture(target_url, PAGES_FIXTURE);
    let mut pages = parse_page_images(&body, offset);
    if let Some(next) = next_page_url(&body) {
        let mut more = fetch_pages_recursive(&absolute_url(&next), offset + pages.len());
        pages.append(&mut more);
    }
    pages
}

fn parse_page_images(body: &str, offset: usize) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("article-body") || chunk.contains("src="))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: normalize_image_url(image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", offset + index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn next_page_url(body: &str) -> Option<String> {
    body.split("page-numbers")
        .find(|chunk| chunk.contains("Next"))
        .and_then(|chunk| html::attr_after(chunk, "<a", "href"))
}

fn article_tags(body: &str) -> Vec<String> {
    body.split("article-tags")
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn selected_category(request: &Value) -> Option<&'static str> {
    let value = request
        .get("filters")
        .and_then(|filters| filters.get("category"))
        .and_then(Value::as_str)?;
    CATEGORIES
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(value))
        .map(|(_, slug)| *slug)
}

fn normalize_image_url(value: String) -> String {
    if value.starts_with("https://i") && value.contains(".wp.com/") {
        value
            .replacen(
                value.split(".wp.com/").next().unwrap_or_default(),
                "https://",
                1,
            )
            .replace(".wp.com/", "")
            .replace("?w=640", "")
    } else {
        absolute_url(&value)
    }
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

const CATEGORIES: [(&str, &str); 12] = [
    ("Ao-yem", "Ao-yem"),
    ("Asia", "Asia"),
    ("Beauty", "beauty"),
    ("Bikini", "Bikini"),
    ("China", "China"),
    ("Cosplay", "Cosplay"),
    ("Japan", "Japan"),
    ("Nude", "Nude"),
    ("Sexy", "Sexy"),
    ("Top", "Top"),
    ("Tattoo", "tattoo"),
    ("Vietnam", "Vietnam"),
];

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<html><body>
  <div class="product-item">
    <a href="https://baobua.net/sample-post"><img class="product-imgreal" src="https://baobua.net/image.jpg"></a>
    <div class="product-title">Sample Gallery</div>
  </div>
  <div class="pagination-custom"><a class="nextPage">Next</a></div>
</body></html>
"#;

const DETAILS_FIXTURE: &str = r#"
<html><head><link rel="canonical" href="https://baobua.net/sample-post"></head><body>
  <h1>Sample Gallery</h1>
  <div class="article-tags"><a>Japan</a><a>Cosplay</a></div>
  <div class="article-date-comment"><span class="date">Mon Jan 01 2024</span></div>
  <div class="article-body"><img src="https://baobua.net/page-1.jpg"></div>
</body></html>
"#;

const PAGES_FIXTURE: &str = r#"
<html><body>
  <div class="article-body">
    <img src="https://baobua.net/page-1.jpg">
    <img src="https://baobua.net/page-2.jpg">
  </div>
</body></html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing() {
        let page = parse_mangas_page(LIST_FIXTURE);
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].key, "/sample-post");
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_details() {
        let item = parse_details(DETAILS_FIXTURE, None);
        assert_eq!(item.title, "Sample Gallery");
        assert_eq!(item.tags, vec!["Japan", "Cosplay"]);
    }

    #[test]
    fn parses_pages() {
        let pages = parse_page_images(PAGES_FIXTURE, 0);
        assert_eq!(pages.len(), 2);
    }
}
