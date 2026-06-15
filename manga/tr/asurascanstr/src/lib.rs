use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: AsuraScansTr = AsuraScansTr;

struct AsuraScansTr;

const CONFIG: MadaraConfig = MadaraConfig {
    base_url: "https://asurascans.com.tr",
    lang: "tr",
    content_rating: "safe",
    manga_path: "manga",
    popular_url_marker: "post-title",
    use_load_more: false,
    latest_enabled: true,
};

impl MangaSource for AsuraScansTr {
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
                entries: vec![parse_details_with_alt_names(&body, Some(key), &CONFIG)],
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
        Ok(parse_details_with_alt_names(&body, Some(key), &CONFIG))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.absolute_url(&key),
            DETAILS_FIXTURE,
        );
        Ok(manga::Madara::parse_chapters(&body, &key, &CONFIG))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let body = manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.absolute_url(&key),
            PAGES_FIXTURE,
        );
        Ok(manga::Madara::parse_pages(&body, &CONFIG))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(CONFIG.base_url) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details_with_alt_names(
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

export_manga_source!(SOURCE);

fn parse_details_with_alt_names(
    body: &str,
    key: Option<String>,
    config: &MadaraConfig,
) -> CatalogItem {
    let mut item = manga::Madara::parse_details(body, key, config);
    if let Some(alt_names) = info_value(body, "Diğer Adlar") {
        item.description = Some(match item.description.take() {
            Some(description) if !description.is_empty() => {
                format!("{description}\n\nDiğer Adlar: {alt_names}")
            }
            _ => format!("Diğer Adlar: {alt_names}"),
        });
    }
    item
}

fn info_value(body: &str, label: &str) -> Option<String> {
    let index = body.to_lowercase().find(&label.to_lowercase())?;
    let fragment = &body[index..body.len().min(index + 900)];
    html::text_between(fragment, "summary-content", "</")
        .or_else(|| html::text_between(fragment, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div><div class="nav-previous"></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="post-title">Sample Manga</h1><div class="summary_image"><img src="/cover.jpg"></div><div class="post-content_item"><div class="summary-heading">Diğer Adlar</div><div class="summary-content">Alt Name</div></div><ul><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">1 Ocak 2024</span></li></ul>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;
