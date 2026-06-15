use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: GhosToon = GhosToon;
const BASE_URL: &str = "https://ghostoon.com";
const LOGIN_REQUIRED: &str = "Log in with WebView to view this chapter";
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: BASE_URL,
    lang: "tr",
    content_rating: "adult",
    manga_path: "manga",
    popular_url_marker: "post-title",
    use_load_more: false,
    latest_enabled: true,
};

struct GhosToon;

impl MangaSource for GhosToon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        Ok(list_from_body(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.list_url(page, order),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = CONFIG.normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(
                    &manga::Madara::fetch_document_or_fixture(&CONFIG, query, DETAILS_FIXTURE),
                    Some(key),
                    &CONFIG,
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = genre_url(page, request.get("filters").unwrap_or(&Value::Null))
            .unwrap_or_else(|| CONFIG.search_url(page, query));
        Ok(list_from_body(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(manga::Madara::parse_details(
            &manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &CONFIG.absolute_url(&key),
                DETAILS_FIXTURE,
            ),
            Some(key),
            &CONFIG,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let chapters = fetch_ajax_chapters(&key);
        if !chapters.is_empty() {
            return Ok(chapters);
        }
        Ok(manga::Madara::parse_chapters(
            &manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &CONFIG.absolute_url(&key),
                DETAILS_FIXTURE,
            ),
            &key,
            &CONFIG,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let body = manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.absolute_url(&key),
            PAGES_FIXTURE,
        );
        let pages = manga::Madara::parse_pages(&body, &CONFIG);
        if pages.is_empty() && is_blocked(&body) {
            return Ok(vec![MangaPage {
                content: PageContent::Text {
                    text: LOGIN_REQUIRED.to_string(),
                },
                description: Some(LOGIN_REQUIRED.to_string()),
                ..MangaPage::default()
            }]);
        }
        Ok(pages)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| CONFIG.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| CONFIG.absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(manga::Madara::parse_details(
                    &manga::Madara::fetch_document_or_fixture(&CONFIG, input, DETAILS_FIXTURE),
                    Some(CONFIG.normalize_manga_key(input)),
                    &CONFIG,
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

fn list_from_body(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: manga::Madara::parse_listing(body, &CONFIG),
        has_next_page: manga::Madara::has_next_page(body, &CONFIG),
    }
}

fn fetch_ajax_chapters(manga_key: &str) -> Vec<MangaChapter> {
    let manga_url = CONFIG.absolute_url(manga_key);
    let body = manga::Madara::browser_client(&CONFIG)
        .post(format!("{}/ajax/chapters", manga_url.trim_end_matches('/')))
        .form(&[])
        .xhr()
        .send_text()
        .unwrap_or_default();
    if body.contains("wp-manga-chapter") {
        manga::Madara::parse_chapters(&body, manga_key, &CONFIG)
    } else {
        Vec::new()
    }
}

fn genre_url(page: u64, filters: &Value) -> Option<String> {
    let genre = filters.get("genre").and_then(Value::as_str)?.trim();
    if genre.is_empty() {
        return None;
    }
    let page_path = if page <= 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    Some(format!(
        "{}/manga-genre/{}/{}?m_orderby=views",
        BASE_URL.trim_end_matches('/'),
        url::query_escape(genre),
        page_path
    ))
}

fn is_blocked(body: &str) -> bool {
    body.contains("content-blocked") || body.contains("login-required")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail">
  <h3 class="post-title"><a href="/manga/sample/">Sample GhosToon</a></h3>
  <img src="/cover.jpg">
</div>
<div class="nav-previous"></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="post-title">Sample GhosToon</h1>
<div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary">Sample description.</div>
<div class="genres-content"><a href="/manga-genre/gri-melek/">Ghost Hentai</a></div>
<ul><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Bolum 1</a><span class="chapter-release-date">1 Ocak 2024</span></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="reading-content">
  <div class="page-break"><img class="wp-manga-chapter-img" src="/page-1.jpg"></div>
</div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture() {
        assert_eq!(list_from_body(LIST_FIXTURE).entries[0].title, "Sample GhosToon");
        assert_eq!(manga::Madara::parse_pages(PAGES_FIXTURE, &CONFIG).len(), 1);
        assert!(is_blocked("<div class=\"login-required\"></div>"));
    }
}
