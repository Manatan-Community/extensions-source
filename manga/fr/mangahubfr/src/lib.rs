use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    manga::{self, MadaraConfig},
    sdk::SearchRequest,
};
use serde_json::Value;

const SOURCE: MangaHubFr = MangaHubFr;
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: "https://mangahub.fr",
    lang: "fr",
    content_rating: "safe",
    manga_path: "manga",
    popular_url_marker: "post-title",
    use_load_more: false,
    latest_enabled: true,
};

struct MangaHubFr;

impl MangaSource for MangaHubFr {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, false));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let order = if latest { "latest" } else { "trending" };
        Ok(parse_listing(
            &manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.list_url(page, order), LIST_FIXTURE),
            latest,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = deeplink_key(query) {
            let body = manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &CONFIG.absolute_url(&key),
                DETAILS_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(&body, Some(key), &CONFIG)],
                has_next_page: false,
            });
        }
        let body = if query.is_empty() {
            let order = filter(&request, "order").unwrap_or("latest");
            manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.list_url(page, order), LIST_FIXTURE)
        } else {
            manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.search_url(page, query), LIST_FIXTURE)
        };
        Ok(parse_listing(&body, false))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_details(&body, Some(key), &CONFIG))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters_without_premium(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages_trimmed(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = deeplink_key(input) {
            let body = manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &CONFIG.absolute_url(&key),
                DETAILS_FIXTURE,
            );
            return Ok(Some(UrlResolveResult {
                item: Some(manga::Madara::parse_details(&body, Some(key), &CONFIG)),
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

export_manga_source!(SOURCE);

fn parse_listing(body: &str, latest: bool) -> Paged<CatalogItem> {
    let mut entries = manga::Madara::parse_listing(body, &CONFIG);
    if latest {
        entries.reverse();
    }
    Paged {
        entries,
        has_next_page: manga::Madara::has_next_page(body, &CONFIG),
    }
}

fn parse_chapters_without_premium(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let filtered = body
        .split("<li")
        .map(|chunk| {
            if chunk.contains("wp-manga-chapter") && chunk.contains("vip-permission") {
                ""
            } else {
                chunk
            }
        })
        .collect::<Vec<_>>()
        .join("<li");
    manga::Madara::parse_chapters(&filtered, manga_key, &CONFIG)
}

fn parse_pages_trimmed(body: &str) -> Vec<MangaPage> {
    let mut pages = manga::Madara::parse_pages(body, &CONFIG);
    for page in &mut pages {
        if let PageContent::Url { url, .. } = &mut page.content {
            *url = url.trim().to_string();
        }
    }
    pages
}

fn deeplink_key(input: &str) -> Option<String> {
    if input.starts_with(CONFIG.base_url) && input.contains("/manga/") {
        Some(CONFIG.normalize_manga_key(input))
    } else {
        None
    }
}

fn filter<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><div class="item-thumb"><a href="https://mangahub.fr/manga/sample/"><img src="/cover.jpg"></a></div><div class="post-title"><h3><a href="https://mangahub.fr/manga/sample/">Sample</a></h3></div></div>
<div class="nav-previous"><a href="/manga/page/2/?m_orderby=trending">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary"><p>Summary</p></div>
<div class="post-content_item"><div>Genres</div><div><a>Action</a></div></div>
<ul><li class="wp-manga-chapter"><a href="https://mangahub.fr/manga/sample/chapter-1/">Chapitre 1</a><span class="chapter-release-date">2024-01-01</span></li><li class="wp-manga-chapter vip-permission"><a href="https://mangahub.fr/manga/sample/chapter-2/">Premium</a></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"<div class="reading-content"><img class="wp-manga-chapter-img" data-src=" /page1.jpg "><img class="wp-manga-chapter-img" src="/page2.jpg"></div>"#;
