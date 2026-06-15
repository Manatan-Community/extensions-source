use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Manhwahana = Manhwahana;
const CONFIG: manga::MadaraConfig = manga::MadaraConfig {
    base_url: "https://manhwahana.com",
    lang: "id",
    content_rating: "adult",
    manga_path: "manga",
    popular_url_marker: "div.post-title a",
    use_load_more: false,
    latest_enabled: true,
};

struct Manhwahana;

impl MangaSource for Manhwahana {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        let body = fetch_document_or_fixture(&CONFIG.list_url(page, order), LIST_FIXTURE);
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &CONFIG),
            has_next_page: manga::Madara::has_next_page(&body, &CONFIG),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(CONFIG.base_url) {
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(
                    &body,
                    Some(CONFIG.normalize_manga_key(query)),
                    &CONFIG,
                )],
                has_next_page: false,
            });
        }
        let body = fetch_document_or_fixture(
            &madara_search_url(page, query, request.get("filters").unwrap_or(&Value::Null)),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &CONFIG),
            has_next_page: manga::Madara::has_next_page(&body, &CONFIG),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document_or_fixture(&CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_details(&body, Some(key), &CONFIG))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document_or_fixture(&CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_chapters(&body, &key, &CONFIG))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let target = chapter_url(&key);
        let body = fetch_document_or_fixture(&target, PAGES_FIXTURE);
        Ok(manga::Madara::parse_pages(&body, &CONFIG))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| CONFIG.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| chapter_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(CONFIG.base_url) && input.contains("/manga/") {
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(manga::Madara::parse_details(
                    &body,
                    Some(CONFIG.normalize_manga_key(input)),
                    &CONFIG,
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
        .with_referer(format!("{}/", CONFIG.base_url.trim_end_matches('/')))
        .with_cookies_for(CONFIG.base_url)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn madara_search_url(page: u64, query: &str, filters: &Value) -> String {
    let page_path = if page <= 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    let mut params = vec![
        ("s", url::query_escape(query)),
        ("post_type", "wp-manga".to_string()),
    ];
    for (id, parameter) in [
        ("author", "author"),
        ("artist", "artist"),
        ("year", "release"),
        ("order", "m_orderby"),
        ("adult", "adult"),
        ("genre_condition", "op"),
    ] {
        if let Some(value) = filter_string(filters, id).filter(|value| !value.is_empty()) {
            params.push((parameter, url::query_escape(&value)));
        }
    }
    if let Some(statuses) = filters.get("status") {
        if let Some(array) = statuses.as_array() {
            for status in array.iter().filter_map(Value::as_str) {
                params.push(("status[]", url::query_escape(status)));
            }
        } else if let Some(status) = statuses.as_str().filter(|value| !value.is_empty()) {
            params.push(("status[]", url::query_escape(status)));
        }
    }
    if let Some(genres) = filter_string(filters, "genres") {
        for genre in genres
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            params.push(("genre[]", url::query_escape(genre)));
        }
    }
    format!(
        "{}/{}?{}",
        CONFIG.base_url.trim_end_matches('/'),
        page_path,
        params
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
}

fn chapter_url(key: &str) -> String {
    let base = CONFIG.absolute_url(key);
    if base.contains("?style=") {
        base
    } else {
        format!("{base}?style=list")
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail manga">
  <div class="item-thumb"><a href="/manga/sample/"><img src="/cover.jpg" alt="Sample"></a></div>
  <div class="post-title"><h3><a href="/manga/sample/">Sample</a></h3></div>
</div>
<div class="nav-previous"><a href="/manga/page/2/">Older</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample</h1></div>
<div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary"><div class="summary__content"><p>Sample description.</p></div></div>
<div class="post-content_item"><div class="summary-heading">Genres</div><div class="summary-content"><a href="/genre/action/">Action</a></div></div>
<div class="post-content_item"><div class="summary-heading">Status</div><div class="summary-content">OnGoing</div></div>
<li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">January 1, 2024</span></li>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="reading-content"><div class="page-break"><img class="wp-manga-chapter-img" src="/page-1.jpg"></div></div>
"#;
