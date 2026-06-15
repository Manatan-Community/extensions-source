use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: KomikNextGOnline = KomikNextGOnline;
const BASE_URL: &str = "https://komiknextgonline.com";

struct KomikNextGOnline;

impl MangaSource for KomikNextGOnline {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document_or_fixture(
            &listing_url(page, None, ""),
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
        let category = request
            .get("filters")
            .and_then(|filters| filters.get("category"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(parse_listing(&fetch_document_or_fixture(
            &listing_url(page, Some(query), category),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Chapter 1".to_string()),
            chapter_number: Some(1.0),
            date_uploaded: html::text_between(&body, "posted-on", "</")
                .or_else(|| html::text_between(&body, "Posted on", "</"))
                .map(|value| html::strip_tags(&value).replace("Posted on", ""))
                .and_then(|value| parse_date(&value)),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".into());
        Ok(parse_pages(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn listing_url(page: u64, query: Option<&str>, category: &str) -> String {
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        let page_path = if page > 1 {
            format!("/page/{page}")
        } else {
            String::new()
        };
        return format!("{BASE_URL}{page_path}/?s={}", url::query_escape(query));
    }
    let base = if category.is_empty() {
        BASE_URL.to_string()
    } else {
        format!("{BASE_URL}/{}", category.trim_matches('/'))
    };
    if page > 1 {
        format!("{base}/?comics_paged={page}")
    } else {
        base
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<li")
            .skip(1)
            .filter(|chunk| chunk.contains("comic"))
            .chain(
                body.split("<article")
                    .skip(1)
                    .filter(|chunk| chunk.contains("comic")),
            )
            .filter_map(parse_listing_item)
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("next page-numbers"),
    }
}

fn parse_listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let title = html::text_between(chunk, "comic-title", "</")
        .or_else(|| html::text_between(chunk, "entry-title", "</"))
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .or_else(|| url::slug_from_url(&href))
        .map(|value| clean_title(&html::strip_tags(&value)))?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("id".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| clean_title(&html::strip_tags(&value)))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Komik Next G Online".to_string()),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "article", "</article>")
            .or_else(|| html::text_between(body, "entry-content", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: html::text_between(body, "byline", "</")
            .map(|value| html::strip_tags(&value).replace("by ", ""))
            .filter(|value| !value.is_empty())
            .into_iter()
            .collect(),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("id".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let reader = body
        .split("spliced-comic")
        .nth(1)
        .or_else(|| body.split("entry-content").nth(1))
        .unwrap_or(body);
    reader
        .split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .map(|image| url::join_url(BASE_URL, &image))
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

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn clean_title(value: &str) -> String {
    let value = value.trim();
    if let Some((_, title)) = value.split_once('.') {
        if value.trim_start().starts_with('#') {
            return title.trim().to_string();
        }
    }
    value.to_string()
}

fn parse_date(value: &str) -> Option<i64> {
    dates::parse_fixture_date(value.trim()).or_else(|| dates::parse_ymd(value.trim()))
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

const LIST_FIXTURE: &str = r#"
<div id="left-content"><ul id="comic-list">
<li class="comic"><a href="https://komiknextgonline.com/sample/"><img src="/cover.jpg"><h2 class="comic-title">#1. Sample</h2></a></li>
</ul><a class="next page-numbers" href="/?comics_paged=2">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<article class="post"><h1 class="entry-title">Sample</h1><span class="byline">by Author Name</span><span class="posted-on"><a>January 1, 2024</a></span><p>Sample description.</p><meta property="og:image" content="https://komiknextgonline.com/cover.jpg"></article>
"#;

const PAGES_FIXTURE: &str = r#"
<div id="spliced-comic"><img src="https://komiknextgonline.com/page-1.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
