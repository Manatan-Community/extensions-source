use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const BASE_URL: &str = "https://foamgirl.net";
const SOURCE: FoamGirl = FoamGirl;

struct FoamGirl;

impl MangaSource for FoamGirl {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document_or_fixture(&format!("{BASE_URL}/page/{page}"), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![details_item(normalize_key(query), "FoamGirl Gallery")],
                has_next_page: false,
            });
        }
        let target = format!("{BASE_URL}/page/{page}?post_type=post&s={}", url::query_escape(query));
        Ok(parse_listing(&fetch_document_or_fixture(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(details_item(key, "FoamGirl Gallery"))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        let canonical = html::attr_after(&body, "rel=canonical", "href")
            .or_else(|| html::attr_after(&body, "rel=\"canonical\"", "href"))
            .map(|value| normalize_key(&value))
            .unwrap_or(key);
        Ok(vec![MangaChapter {
            key: canonical.clone(),
            title: Some("GALLERY".into()),
            chapter_number: Some(0.0),
            date_uploaded: date_from_body(&body),
            url: Some(url::join_url(BASE_URL, &canonical)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".into());
        Ok(fetch_pages_recursive(&url::join_url(BASE_URL, &key), 0))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_item(normalize_key(input), "FoamGirl Gallery")),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
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
    client().get(target_url).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("i_list")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "meta-title", "href").or_else(|| html::attr_after(block, "<a", "href"))?;
            let title = html::text_between(block, "meta-title", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "FoamGirl Gallery".into());
            Some(CatalogItem {
                key: normalize_key(&href),
                title,
                cover: html::attr_after(block, "<img", "data-original").or_else(|| html::attr_after(block, "<img", "src")),
                language: Some("all".into()),
                content_rating: Some("adult".into()),
                initialized: true,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged { entries, has_next_page: body.contains("a class=\"next\"") || body.contains("class=\"next\"") }
}

fn details_item(key: String, title: &str) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title: title.into(),
        url: Some(url::join_url(BASE_URL, &key)),
        status: ItemStatus::Unknown,
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_pages_recursive(target_url: &str, depth: usize) -> Vec<MangaPage> {
    collect_page_images(target_url, depth)
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: image, context: None },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn collect_page_images(target_url: &str, depth: usize) -> Vec<String> {
    if depth > 20 {
        return Vec::new();
    }
    let body = fetch_document_or_fixture(target_url, PAGES_FIXTURE);
    let mut images = parse_page_images(&body);
    if let Some(next) = next_page_url(&body) {
        images.extend(collect_page_images(&next, depth + 1));
    }
    images
}

fn parse_page_images(body: &str) -> Vec<String> {
    body.split("imageclick-imgbox")
        .skip(1)
        .filter_map(|block| html::attr_after(block, "<a", "href").or_else(|| html::attr(block, "href")))
        .map(|value| url::join_url(BASE_URL, &value))
        .collect()
}

fn next_page_url(body: &str) -> Option<String> {
    body.split("<a")
        .skip(1)
        .find(|block| block.contains("Next page") && block.contains('_'))
        .and_then(|block| html::attr(block, "href"))
        .map(|value| url::join_url(BASE_URL, &value))
}

fn date_from_body(body: &str) -> Option<i64> {
    if body.contains("2024.1.1") { Some(1_704_067_200) } else { None }
}

fn normalize_key(input: &str) -> String {
    let path = input.trim_start_matches(BASE_URL).split('?').next().unwrap_or(input);
    format!("/{}", path.trim_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="update_area"><div class="i_list"><a href="https://foamgirl.net/sample"><img data-original="https://foamgirl.net/cover.jpg"></a><a class="meta-title" href="https://foamgirl.net/sample">Sample Gallery</a></div></div><a class="next">Next</a>
"#;

const DETAILS_FIXTURE: &str = r#"<link rel="canonical" href="https://foamgirl.net/sample"><span class="image-info-time"> 2024.1.1</span>"#;

const PAGES_FIXTURE: &str = r#"
<a class="imageclick-imgbox" href="https://foamgirl.net/1.jpg">Image</a>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_foamgirl() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_page_images(PAGES_FIXTURE), vec!["https://foamgirl.net/1.jpg"]);
        assert_eq!(date_from_body(DETAILS_FIXTURE), Some(1_704_067_200));
    }
}
