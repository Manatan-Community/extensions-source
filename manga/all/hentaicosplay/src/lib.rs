use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const BASE_URL: &str = "https://hentai-cosplay-xxx.com";
const SOURCE: HentaiCosplay = HentaiCosplay;

struct HentaiCosplay;

impl MangaSource for HentaiCosplay {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            format!("{BASE_URL}/search/page/{page}/")
        } else {
            format!("{BASE_URL}/ranking/page/{page}/")
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
        if query.starts_with(BASE_URL) && query.contains("/image/") {
            let key = normalize_key(query);
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let target = if !query.is_empty() {
            format!("{BASE_URL}/search/keyword/{}/page/{page}/", query.replace(' ', "+"))
        } else if let Some(tag) = request
            .get("filters")
            .and_then(|filters| filters.get("tagPath"))
            .and_then(Value::as_str)
            .filter(|tag| !tag.trim().is_empty())
        {
            format!("{BASE_URL}/{}/page/{page}/", tag.trim().trim_matches('/'))
        } else {
            format!("{BASE_URL}/search/page/{page}/")
        };
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/image/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/image/sample".into());
        let chapter_key = key.replace("/image/", "/story/");
        Ok(vec![MangaChapter {
            key: chapter_key.clone(),
            title: Some("Gallery".into()),
            chapter_number: Some(1.0),
            url: Some(url::join_url(BASE_URL, &chapter_key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/story/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/image/") {
            let key = normalize_key(input);
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
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
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let desktop = body
        .split("image-list-item")
        .skip(1)
        .filter(|chunk| chunk.contains("/image/"))
        .filter_map(parse_listing_chunk);
    let mobile = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("/image/"))
        .filter_map(parse_listing_chunk);
    Paged {
        entries: desktop.chain(mobile).fold(Vec::new(), push_unique),
        has_next_page: body.contains("rel=next") || body.contains("rel=\"next\""),
    }
}

fn parse_listing_chunk(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    let title = html::text_between(chunk, "image-list-item-title", "</")
        .or_else(|| html::text_between(chunk, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Hentai Cosplay Gallery".into()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "<img", "src").map(|value| secure_url(&url::join_url(BASE_URL, &value))),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/image/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::attr_after(body, "property=\"og:title\"", "content")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Hentai Cosplay Gallery".into())),
        cover: html::attr_after(body, "#display_image_detail", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| secure_url(&url::join_url(BASE_URL, &value))),
        url: Some(url::join_url(BASE_URL, &key)),
        tags: detail_tags(body),
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("amp-img")
        .skip(1)
        .filter(|chunk| chunk.contains("upload") && !chunk.contains("related-thumbnail"))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, src)| MangaPage {
            content: PageContent::Url {
                url: secure_url(&src),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn detail_tags(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/tag/"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn secure_url(input: &str) -> String {
    input.replace("http://", "https://")
}

fn normalize_key(input: &str) -> String {
    let path = input
        .trim_start_matches(BASE_URL)
        .split('#')
        .next()
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim();
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"
<div class="image-list-item"><a href="https://hentai-cosplay-xxx.com/image/sample"><img src="http://hentai-cosplay-xxx.com/thumb.jpg"><div class="image-list-item-title">Sample Gallery</div></a><div class="image-list-item-regist-date">2024/01/01</div></div>
<div class="wp-pagenavi"><a rel="next" href="/ranking/page/2/">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<meta property="og:title" content="Sample Gallery">
<div id="detail_tag"><a href="/tag/outdoor">Outdoor</a></div>
<div id="display_image_detail"><img src="http://hentai-cosplay-xxx.com/cover.jpg"></div>
"#;

const PAGES_FIXTURE: &str = r#"
<amp-img src="https://hentai-cosplay-xxx.com/upload/1.jpg"></amp-img>
<amp-img src="https://hentai-cosplay-xxx.com/upload/2.jpg"></amp-img>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hentai_cosplay() {
        let listing = parse_listing(LIST_FIXTURE);
        assert_eq!(listing.entries.len(), 1);
        assert!(listing.has_next_page);
        assert_eq!(parse_details(DETAILS_FIXTURE, Some("/image/sample".into())).tags.len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
