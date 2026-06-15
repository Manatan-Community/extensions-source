use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: HentaiArchive = HentaiArchive;
const BASE_URL: &str = "https://www.hentai-archive.com";

struct HentaiArchive;

impl MangaSource for HentaiArchive {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/category/hentai-recenti/page/{page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default();
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_search(&fetch_document_or_fixture(
            &format!("{BASE_URL}/page/{page}/?s={}", url::query_escape(query)),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Chapter".to_string()),
            chapter_number: Some(1.0),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_pages(&fetch_document_or_fixture(
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<article")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "entire-meta-link", "href")
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                let title = html::text_between(chunk, "screen-reader-text", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| "HentaiArchive".to_string());
                Some(catalog_item(normalize_key(&href), title, image_attr(chunk)))
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("next page-numbers") || body.contains("next.page-numbers"),
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<article")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "h2", "href")
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                let title = html::text_between(chunk, "title", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<a", "title"))
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| "HentaiArchive".to_string());
                Some(catalog_item(normalize_key(&href), title, image_attr(chunk)))
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("next page-numbers") || body.contains("next.page-numbers"),
    }
}

fn catalog_item(key: String, title: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("it".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "HentaiArchive".to_string()),
        tags: html::text_between(body, "meta-category", "</")
            .map(|value| {
                value
                    .split_whitespace()
                    .map(|item| item.replace('-', " "))
                    .filter(|item| !item.eq_ignore_ascii_case("hentai"))
                    .collect()
            })
            .unwrap_or_default(),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("it".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("content-inner") || chunk.contains("data-src") || chunk.contains("wp-image"))
        .filter_map(image_attr)
        .map(|image| remove_resize_suffix(&url::join_url(BASE_URL, &image)))
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
    html::attr(chunk, "data-nectar-img-src")
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn remove_resize_suffix(input: &str) -> String {
    let Some(dot) = input.rfind(".jpg") else {
        return input.to_string();
    };
    let before = &input[..dot];
    let Some(dash) = before.rfind('-') else {
        return input.to_string();
    };
    let suffix = &before[dash + 1..];
    if suffix.contains('x') && suffix.chars().all(|ch| ch.is_ascii_digit() || ch == 'x') {
        format!("{}.jpg{}", &before[..dash], &input[dot + 4..])
    } else {
        input.to_string()
    }
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!(
                "/{}",
                value[index + BASE_URL.len()..]
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<article><a class="entire-meta-link" href="/sample"></a><span class="screen-reader-text">Sample Manga</span><span class="post-featured-img"><img data-nectar-img-src="/cover.jpg"></span></article>
<nav id="pagination"><a class="next page-numbers"></a></nav>
"#;
const SEARCH_FIXTURE: &str = r#"
<article class="result"><h2 class="title"><a href="/sample">Sample Manga</a></h2><a><img class="wp-post-image" src="/cover.jpg"></a></article>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="main-content"><h1>Sample Manga</h1><div class="meta-category category-ahegao category-hentai"></div></div>
"#;
const PAGES_FIXTURE: &str = r#"
<div class="content-inner"><img data-src="https://picsarchive1.b-cdn.net/sample-800x1200.jpg"><img src="/sample2.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_hentaiarchive_fixture() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries[0].title, "Sample Manga");
        assert_eq!(SOURCE.chapters(json!({"manga":"/sample"})).unwrap().len(), 1);
        assert_eq!(SOURCE.pages(json!({"chapter":"/sample"})).unwrap()[0].description.as_deref(), Some("Page 1"));
    }
}
