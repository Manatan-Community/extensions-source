use manatan_extension::{
    abi::ExtensionResult, export_manga_source, source::MangaSource, CatalogItem, MangaChapter,
    MangaPage, Paged, UrlResolveResult,
};
use manatan_shared::{manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Holotoon = Holotoon;
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: "https://01.holotoon.site",
    lang: "id",
    content_rating: "adult",
    manga_path: "komik",
    popular_url_marker: "post-title",
    use_load_more: false,
    latest_enabled: true,
};

struct Holotoon;

impl MangaSource for Holotoon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(page_from_body(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        Ok(page_from_body(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.list_url(page, order),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        madara_search(&CONFIG, request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        madara_details(&CONFIG, request)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        madara_chapters(&CONFIG, request)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        madara_pages(&CONFIG, request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| CONFIG.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| CONFIG.absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        madara_handle_url(&CONFIG, request)
    }
}

fn page_from_body(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: manga::Madara::parse_listing(body, &CONFIG),
        has_next_page: manga::Madara::has_next_page(body, &CONFIG),
    }
}

fn madara_search(config: &MadaraConfig, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
    let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
    let query = request
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if query.starts_with(config.base_url) {
        let key = config.normalize_manga_key(query);
        return Ok(Paged {
            entries: vec![manga::Madara::parse_details(
                &manga::Madara::fetch_document_or_fixture(config, query, DETAILS_FIXTURE),
                Some(key),
                config,
            )],
            has_next_page: false,
        });
    }
    let body =
        manga::Madara::fetch_document_or_fixture(config, &config.search_url(page, query), LIST_FIXTURE);
    Ok(Paged {
        entries: manga::Madara::parse_listing(&body, config),
        has_next_page: manga::Madara::has_next_page(&body, config),
    })
}

fn madara_details(config: &MadaraConfig, request: Value) -> ExtensionResult<CatalogItem> {
    let key = manga::request_key(&request, "manga")
        .unwrap_or_else(|| format!("/{}/sample", config.manga_path));
    Ok(manga::Madara::parse_details(
        &manga::Madara::fetch_document_or_fixture(config, &config.absolute_url(&key), DETAILS_FIXTURE),
        Some(key),
        config,
    ))
}

fn madara_chapters(config: &MadaraConfig, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
    let key = manga::request_key(&request, "manga")
        .unwrap_or_else(|| format!("/{}/sample", config.manga_path));
    Ok(manga::Madara::parse_chapters(
        &manga::Madara::fetch_document_or_fixture(config, &config.absolute_url(&key), DETAILS_FIXTURE),
        &key,
        config,
    ))
}

fn madara_pages(config: &MadaraConfig, request: Value) -> ExtensionResult<Vec<MangaPage>> {
    let key = manga::request_key(&request, "chapter")
        .unwrap_or_else(|| format!("/{}/sample/chapter-1", config.manga_path));
    Ok(manga::Madara::parse_pages(
        &manga::Madara::fetch_document_or_fixture(config, &config.absolute_url(&key), PAGES_FIXTURE),
        config,
    ))
}

fn madara_handle_url(
    config: &MadaraConfig,
    request: Value,
) -> ExtensionResult<Option<UrlResolveResult>> {
    let Some(input) = request.get("url").and_then(Value::as_str) else {
        return Ok(None);
    };
    if input.starts_with(config.base_url) {
        let key = config.normalize_manga_key(input);
        return Ok(Some(UrlResolveResult {
            item: Some(manga::Madara::parse_details(
                &manga::Madara::fetch_document_or_fixture(config, input, DETAILS_FIXTURE),
                Some(key),
                config,
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

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/komik/sample/">Sample Holotoon</a></h3><img src="/cover.jpg"></div><div class="nav-previous"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="post-title">Sample Holotoon</h1><div class="summary_image"><img src="/cover.jpg"></div><div class="description-summary">Sample description.</div><ul><li class="wp-manga-chapter"><a href="/komik/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">January 1, 2024</span></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"
<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"><img class="wp-manga-chapter-img" src="/page2.jpg"></div>
"#;
