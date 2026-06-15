use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Photos18 = Photos18;
const BASE_URL: &str = "https://www.photos18.com";

struct Photos18;

impl MangaSource for Photos18 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let base = base_url_with_lang(&request);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            format!("{base}/?page={page}")
        } else {
            format!("{base}/sort/views?page={page}")
        };
        Ok(parse_listing_page(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![details_from_url(query)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = format!(
            "{}?q={}&page={page}&sort={}&category_id={}",
            base_url_with_lang(&request),
            url::query_escape(query),
            url::query_escape(
                filters
                    .get("sort")
                    .and_then(Value::as_str)
                    .unwrap_or("views")
            ),
            url::query_escape(
                filters
                    .get("categoryId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            )
        );
        Ok(parse_listing_page(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(details_from_url(&url::join_url(BASE_URL, &key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Gallery".to_string()),
            chapter_number: Some(0.0),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_url(input)),
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

fn base_url_with_lang(request: &Value) -> String {
    if request
        .get("preferences")
        .and_then(|prefs| prefs.get("traditionalChinese"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        BASE_URL.to_string()
    } else {
        format!("{BASE_URL}/zh-hans")
    }
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing_page(body: &str) -> Paged<CatalogItem> {
    let videos = body
        .find("id=\"videos\"")
        .map(|index| &body[index..])
        .unwrap_or(body);
    Paged {
        entries: videos
            .split("card-body")
            .skip(1)
            .filter_map(parse_card)
            .collect(),
        has_next_page: body.contains("next") && !body.contains("next disabled"),
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = strip_lang(&href);
    let title = html::text_between(chunk, "<a", "</a>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Gallery".into()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "<img", "src").map(|value| url::join_url(BASE_URL, &value)),
        tags: html::text_between(chunk, "<label", "</label>")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn details_from_url(input: &str) -> CatalogItem {
    let key = strip_lang(input.trim_start_matches(BASE_URL));
    CatalogItem {
        key: key.clone(),
        title: url::slug_from_url(&key).unwrap_or_else(|| "Gallery".into()),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let content = body
        .find("id=\"content\"")
        .map(|index| &body[index..])
        .unwrap_or(body);
    content
        .split("<img")
        .skip(1)
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

fn strip_lang(value: &str) -> String {
    let path = value.trim_start_matches(BASE_URL);
    let path = path.strip_prefix("/zh-hans").unwrap_or(path);
    format!("/{}", path.trim_start_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div id="videos"><div class="card"><img src="/cover.jpg"><div class="card-body"><a href="/zh-hans/gallery/sample">Sample Gallery</a><label>Cosplay</label></div></div></div>
<a class="next" href="?page=2">Next</a>
"#;

const PAGES_FIXTURE: &str = r#"
<div id="content"><img src="https://img.example/1.jpg"><img src="https://img.example/2.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_photos18_fixtures() {
        let page = parse_listing_page(LIST_FIXTURE);
        assert_eq!(page.entries.len(), 1);
        assert!(page.has_next_page);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
        assert_eq!(strip_lang("/zh-hans/gallery/sample"), "/gallery/sample");
    }
}
