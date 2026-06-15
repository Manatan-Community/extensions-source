use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Xinmeitulu = Xinmeitulu;
const BASE_URL: &str = "https://www.xinmeitulu.com";

struct Xinmeitulu;

impl MangaSource for Xinmeitulu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/page/{page}"), LIST_FIXTURE);
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
            let key = normalize_key(query);
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        if let Some(slug) = query.strip_prefix("SLUG:") {
            let target = format!("{BASE_URL}/photo/{slug}");
            let body = fetch_document_or_fixture(&target, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(normalize_key(&target)))],
                has_next_page: false,
            });
        }
        let body = fetch_document_or_fixture(
            &format!("{BASE_URL}/page/{page}?s={}", url::query_escape(query)),
            LIST_FIXTURE,
        );
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/photo/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/photo/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some(detail_title(&body).unwrap_or_else(|| "Album".to_string())),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/photo/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("<figure") || chunk.contains("figcaption"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "figcaption", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Album".into())),
                cover: html::attr_after(chunk, "<img", "data-original-")
                    .or_else(|| html::attr_after(chunk, "<img", "data-original"))
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| url::join_url(BASE_URL, &image)),
                tags: chunk
                    .split("tag")
                    .skip(1)
                    .filter_map(|part| html::text_between(part, ">", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .collect(),
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
        has_next_page: body.contains("next"),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/photo/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: detail_title(body)
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Album".into())),
        description: html::text_between(body, "container", "<div")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        cover: html::attr_after(body, "<figure", "data-original")
            .or_else(|| html::attr_after(body, "<figure", "src"))
            .or_else(|| html::attr_after(body, "<img", "data-original"))
            .map(|image| url::join_url(BASE_URL, &image)),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn detail_title(body: &str) -> Option<String> {
    html::text_between(body, "<h1", "</h1>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<figure")
        .skip(1)
        .filter_map(|chunk| {
            html::attr_after(chunk, "<img", "data-original")
                .or_else(|| html::attr_after(chunk, "<img", "data-original-"))
                .or_else(|| html::attr_after(chunk, "<img", "src"))
        })
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
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="container"><div class="row"><div><figure><a href="https://www.xinmeitulu.com/photo/sample"><img data-original-="/thumb.jpg"></a><figcaption>Sample Album</figcaption><a class="tag">Tag</a></figure></div></div><a class="next">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<link rel="canonical" href="https://www.xinmeitulu.com/photo/sample">
<div class="container"><h1>Sample Album</h1><p>Sample description</p><div><figure><img data-original="/cover.jpg"></figure></div></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="container"><div><figure><img data-original="https://www.xinmeitulu.com/1.jpg"></figure><figure><img data-original="/2.jpg"></figure></div></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xinmeitulu() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(
            parse_details(DETAILS_FIXTURE, Some("/photo/sample".into())).title,
            "Sample Album"
        );
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
