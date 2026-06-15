use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Toon18 = Toon18;

struct Toon18;

impl MangaSource for Toon18 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(page_from_body(LIST_FIXTURE, &config));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.list_url(page, order), LIST_FIXTURE);
        Ok(page_from_body(&body, &config))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(config.base_url) {
            let key = config.normalize_manga_key(query);
            let body = manga::Madara::fetch_document_or_fixture(&config, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(&body, Some(key), &config)],
                has_next_page: false,
            });
        }
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.search_url(page, query),
            LIST_FIXTURE,
        );
        Ok(page_from_body(&body, &config))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_details(&body, Some(key), &config))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_chapters(&body, &key, &config))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config();
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), PAGES_FIXTURE);
        Ok(manga::Madara::parse_pages(&body, &config))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let config = config();
        let popular = page_from_body(
            &manga::Madara::fetch_document_or_fixture(&config, &config.list_url(1, "views"), LIST_FIXTURE),
            &config,
        );
        let latest = page_from_body(
            &manga::Madara::fetch_document_or_fixture(&config, &config.list_url(1, "latest"), LIST_FIXTURE),
            &config,
        );
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = config();
        Ok(manga::request_key(&request, "manga").map(|key| config.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = config();
        Ok(manga::request_key(&request, "chapter").map(|key| config.absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = config();
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(config.base_url) {
            return Ok(Some(UrlResolveResult {
                item: Some(manga::Madara::parse_details(
                    &manga::Madara::fetch_document_or_fixture(&config, input, DETAILS_FIXTURE),
                    Some(config.normalize_manga_key(input)),
                    &config,
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

fn page_from_body(body: &str, config: &MadaraConfig) -> Paged<CatalogItem> {
    Paged {
        entries: manga::Madara::parse_listing(body, config),
        has_next_page: manga::Madara::has_next_page(body, config),
    }
}

fn config() -> MadaraConfig {
    MadaraConfig {
        base_url: "https://toon18.to",
        lang: "en",
        content_rating: "adult",
        manga_path: "manga",
        popular_url_marker: "post-title",
        use_load_more: false,
        latest_enabled: true,
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div><div class="navigation-ajax"></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="post-title"><h1>Sample Manga</h1></div><div class="summary_image"><img src="/cover.jpg"></div><ul class="main version-chap"><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">2024-01-01</span></li></ul>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;
