use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: HentaiZone = HentaiZone;
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: "https://hentaizone.xyz",
    lang: "fr",
    content_rating: "adult",
    manga_path: "tous-les-mangas",
    popular_url_marker: "post-title",
    use_load_more: false,
    latest_enabled: true,
};

struct HentaiZone;

impl MangaSource for HentaiZone {
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(CONFIG.base_url) {
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
        Ok(page_from_body(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &filtered_search_url(page, query, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/tous-les-mangas/sample".into());
        Ok(manga::Madara::parse_details(
            &manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
            &CONFIG,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/tous-les-mangas/sample".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_chapters(&body, &key, &CONFIG))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/tous-les-mangas/sample/chapter-1".into());
        Ok(manga::Madara::parse_pages(
            &manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), PAGES_FIXTURE),
            &CONFIG,
        ))
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
        if input.starts_with(CONFIG.base_url) {
            let key = CONFIG.normalize_manga_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(manga::Madara::parse_details(
                    &manga::Madara::fetch_document_or_fixture(&CONFIG, input, DETAILS_FIXTURE),
                    Some(key),
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

fn page_from_body(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: manga::Madara::parse_listing(body, &CONFIG),
        has_next_page: manga::Madara::has_next_page(body, &CONFIG),
    }
}

fn filtered_search_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    let page_path = if page <= 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    let mut pairs = vec![
        ("s".to_string(), query.to_string()),
        ("post_type".to_string(), "wp-manga".to_string()),
    ];
    for name in ["author", "artist", "release", "adult", "op"] {
        push_filter(&mut pairs, filters, name, name);
    }
    push_filter(&mut pairs, filters, "order", "m_orderby");
    push_filter_array(&mut pairs, filters, "status", "status[]");
    push_csv(&mut pairs, filters, "genres", "genre[]");
    format!(
        "{}/{}?{}",
        CONFIG.base_url.trim_end_matches('/'),
        page_path,
        pairs
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(key, value)| format!(
                "{}={}",
                url::query_escape(&key),
                url::query_escape(&value)
            ))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn push_filter(pairs: &mut Vec<(String, String)>, filters: Option<&Value>, id: &str, key: &str) {
    if let Some(value) = filter_string(filters, id).filter(|value| !value.is_empty()) {
        pairs.push((key.to_string(), value));
    }
}

fn push_filter_array(
    pairs: &mut Vec<(String, String)>,
    filters: Option<&Value>,
    id: &str,
    key: &str,
) {
    for value in filter_values(filters, id) {
        if !value.is_empty() {
            pairs.push((key.to_string(), value));
        }
    }
}

fn push_csv(pairs: &mut Vec<(String, String)>, filters: Option<&Value>, id: &str, key: &str) {
    for value in filter_string(filters, id).unwrap_or_default().split(',') {
        let value = value.trim();
        if !value.is_empty() {
            pairs.push((key.to_string(), value.to_string()));
        }
    }
}

fn filter_string(filters: Option<&Value>, id: &str) -> Option<String> {
    filters?
        .get(id)?
        .as_str()
        .map(|value| value.trim().to_string())
}

fn filter_values(filters: Option<&Value>, id: &str) -> Vec<String> {
    match filters.and_then(|filters| filters.get(id)) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(|value| value.trim().to_string())
            .collect(),
        Some(Value::String(value)) => vec![value.trim().to_string()],
        _ => Vec::new(),
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/tous-les-mangas/sample/">Sample HentaiZone</a></h3><img src="/cover.jpg"></div>
<div class="nav-previous"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="post-title">Sample HentaiZone</h1><div class="summary_image"><img src="/cover.jpg"></div><div class="description-summary">Resume</div>
<ul><li class="wp-manga-chapter"><a href="/tous-les-mangas/sample/chapter-1/">Chapitre 1</a><span class="chapter-release-date">2024-01-01</span></li></ul>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;
