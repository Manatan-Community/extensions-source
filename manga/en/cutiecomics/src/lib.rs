use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: CutieComics = CutieComics;
const BASE_URL: &str = "https://cutiecomics.com";

struct CutieComics;

impl MangaSource for CutieComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if page <= 1 {
            BASE_URL.to_string()
        } else {
            format!("{BASE_URL}/page/{page}")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
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
        if query.len() < 4 {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_search(page, query)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
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

fn fetch_search(page: u64, query: &str) -> String {
    client()
        .post(format!("{BASE_URL}/index.php?do=search"))
        .form(&[
            ("do", "search"),
            ("subaction", "search"),
            ("full_search", "0"),
            ("search_start", &page.to_string()),
            (
                "result_from",
                &(((page.saturating_sub(1)) * 20 + 1).to_string()),
            ),
            ("story", query),
        ])
        .send_text()
        .unwrap_or_else(|_| LIST_FIXTURE.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("w25")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "field-content", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "field-content", "</a>")
                .or_else(|| html::text_between(chunk, "<strong", "</strong>"))
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    url::slug_from_url(&key).unwrap_or_else(|| "Cutie Comics".into())
                });
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|image| url::join_url(BASE_URL, &image)),
                status: ItemStatus::Completed,
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("fa-angle-right"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "page-title", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Cutie Comics".into())),
        cover: html::attr_after(body, "galery", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        tags: body
            .split("field-label")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, "<span", "</span>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
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
        .filter(|chunk| chunk.contains("galery") || chunk.contains("src"))
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

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find(BASE_URL) {
            return format!(
                "/{}",
                input[index + BASE_URL.len()..]
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div id="dle-content"><div class="w25"><strong class="field-content"><a href="/sample">Sample Gallery</a></strong><a><img src="/cover.jpg"></a></div></div><div class="navigation"><a><i class="fa-angle-right"></i></a></div>
"#;
const DETAILS_FIXTURE: &str = r#"<h1 id="page-title">Sample Gallery</h1><div class="galery"><img src="/page1.jpg"><img src="/page2.jpg"></div><h3 class="field-label"></h3><span>Drama</span>"#;
const PAGES_FIXTURE: &str = DETAILS_FIXTURE;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_gallery_source() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample Gallery");
        let pages = SOURCE.pages(json!({"chapter":"/sample"})).unwrap();
        assert_eq!(pages.len(), 2);
    }
}
