use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: DomalFansub = DomalFansub;
const LOGIN_REQUIRED: &str = "Okumak icin WebView uzerinden giris yapin";
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: "https://dom4lfansub.online",
    lang: "tr",
    content_rating: "adult",
    manga_path: "manga",
    popular_url_marker: "post-title",
    use_load_more: false,
    latest_enabled: true,
};

struct DomalFansub;

impl MangaSource for DomalFansub {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        let body = if request.as_object().is_some_and(|object| object.is_empty()) {
            LIST_FIXTURE.to_string()
        } else {
            manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &CONFIG.list_url(page, order),
                LIST_FIXTURE,
            )
        };
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
            .unwrap_or_default();
        if query.starts_with(CONFIG.base_url) {
            let key = CONFIG.normalize_manga_key(query);
            let body = manga::Madara::fetch_document_or_fixture(&CONFIG, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(&body, Some(key), &CONFIG)],
                has_next_page: false,
            });
        }
        let body = manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.search_url(page, query),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &CONFIG),
            has_next_page: manga::Madara::has_next_page(&body, &CONFIG),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.absolute_url(&key),
            DETAILS_FIXTURE,
        );
        Ok(manga::Madara::parse_details(&body, Some(key), &CONFIG))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let manga_url = CONFIG.absolute_url(&key);
        let detail_body =
            manga::Madara::fetch_document_or_fixture(&CONFIG, &manga_url, DETAILS_FIXTURE);
        if requires_login(&manga_url, &detail_body) {
            return Ok(vec![MangaChapter {
                key,
                title: Some(LOGIN_REQUIRED.to_string()),
                url: Some(manga_url),
                is_locked: true,
                ..MangaChapter::default()
            }]);
        }
        let ajax = manga::Madara::browser_client(&CONFIG)
            .post(format!("{}/ajax/chapters", manga_url.trim_end_matches('/')))
            .form(&[])
            .xhr()
            .send_text()
            .unwrap_or_else(|_| detail_body.clone());
        let chapters = manga::Madara::parse_chapters(&ajax, &key, &CONFIG);
        if chapters.len() == 1 && chapters[0].key == key {
            Ok(manga::Madara::parse_chapters(&detail_body, &key, &CONFIG))
        } else {
            Ok(chapters)
        }
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let chapter_url = CONFIG.absolute_url(&key);
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &chapter_url, PAGES_FIXTURE);
        if requires_login(&chapter_url, &body) {
            return Ok(vec![MangaPage {
                content: PageContent::Text {
                    text: LOGIN_REQUIRED.to_string(),
                },
                description: Some(LOGIN_REQUIRED.to_string()),
                ..MangaPage::default()
            }]);
        }
        Ok(manga::Madara::parse_pages(&body, &CONFIG))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(CONFIG.base_url) {
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

fn requires_login(url: &str, body: &str) -> bool {
    url.contains("/giris-korumasi") || body.contains("login-required")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div><div class="nav-previous"></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="post-title">Sample Manga</h1><div class="summary_image"><img src="/cover.jpg"></div><ul><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">1 Ocak 2024</span></li></ul>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;
