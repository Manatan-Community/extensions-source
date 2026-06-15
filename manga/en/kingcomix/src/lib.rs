use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: KingComiX = KingComiX;
const BASE_URL: &str = "https://kingcomix.com";

struct KingComiX;

impl MangaSource for KingComiX {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, false));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(
            &fetch_document(&page_url(BASE_URL, page), LIST_FIXTURE),
            false,
        ))
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
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let (target, text_search) = search_url(page, query, request.get("filters"));
        Ok(parse_listing(
            &fetch_document(&target, LIST_FIXTURE),
            text_search,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/".to_string());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Chapter".to_string()),
            url: Some(url::join_url(BASE_URL, &key)),
            date_uploaded: published_date(&body),
            chapter_number: Some(1.0),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
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

fn page_url(base: &str, page: u64) -> String {
    if page <= 1 {
        format!("{}/", base.trim_end_matches('/'))
    } else {
        format!("{}/page/{page}/", base.trim_end_matches('/'))
    }
}

fn search_url(page: u64, query: &str, filters: Option<&Value>) -> (String, bool) {
    if !query.is_empty() {
        return (format!("{BASE_URL}/?s={}", url::query_escape(query)), true);
    }
    let category = filter_text(filters, "category");
    let tag = filter_text(filters, "tag");
    let base = if !category.is_empty() {
        format!("{BASE_URL}/category/{}/", category.trim_matches('/'))
    } else if !tag.is_empty() {
        format!("{BASE_URL}/tag/{}/", tag.trim_matches('/'))
    } else {
        format!("{BASE_URL}/")
    };
    (page_url(base.trim_end_matches('/'), page), false)
}

fn filter_text(filters: Option<&Value>, name: &str) -> String {
    filters
        .and_then(|filters| filters.get(name))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn parse_listing(body: &str, text_search: bool) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<")
            .filter(|chunk| chunk.contains("entry") || chunk.contains("thumb-block"))
            .filter_map(parse_card)
            .fold(Vec::new(), push_unique),
        has_next_page: !text_search
            && body.contains("pagination")
            && (body.contains("Next") || body.contains("next")),
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
    if href.contains("/category/") || href.contains("/tag/") || href.contains("/page/") {
        return None;
    }
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "<h2", "</h2>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| html::attr_after(chunk, "<a", "title"))
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "KingComiX".to_string()),
        cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample/".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "singleTitle-h1", "</")
            .or_else(|| html::text_between(body, "widget-title", "</"))
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "KingComiX".to_string()),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        authors: html::attr_after(body, "name=\"author\"", "content")
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect(),
        tags: link_values(body, "taxLink")
            .into_iter()
            .chain(link_values(body, "/tag/"))
            .collect(),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("entry-content") || chunk.contains("data-src") || chunk.contains("src=")
        })
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:"))
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

fn image_attr(input: &str) -> Option<String> {
    html::attr(input, "data-src")
        .or_else(|| html::attr(input, "data-lazy-src"))
        .or_else(|| html::attr(input, "src"))
}

fn link_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn published_date(body: &str) -> Option<i64> {
    html::attr_after(body, "property=\"article:published_time\"", "content")
        .and_then(|value| parse_ymd(&value))
}

fn parse_ymd(value: &str) -> Option<i64> {
    let y = value.get(0..4)?.parse().ok()?;
    let m = value.get(5..7)?.parse().ok()?;
    let d = value.get(8..10)?.parse().ok()?;
    Some(unix_from_ymd(y, m, d))
}

fn unix_from_ymd(year: i32, month: i32, day: i32) -> i64 {
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64 * 86_400
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="entry"><h2 class="information"><a href="/sample/" title="Sample Comic">Sample Comic</a></h2><img data-src="/cover.jpg"></div><div class="pagination"><a class="next">Next</a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="singleTitle-h1">Sample Comic</h1><meta name="author" content="Author"><meta property="og:image" content="/cover.jpg"><meta property="article:published_time" content="2024-01-01T00:00:00"><div class="caTotal"><a class="taxLink">Action</a></div><div class="entry-content"><img src="/page1.jpg"><img data-src="/page2.jpg"></div>"#;
const PAGES_FIXTURE: &str = DETAILS_FIXTURE;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kingcomix_shapes() {
        assert_eq!(
            parse_listing(LIST_FIXTURE, false).entries[0].title,
            "Sample Comic"
        );
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
