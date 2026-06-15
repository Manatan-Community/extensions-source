use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{
    html, manga,
    manga::{MangaCatalog, MangaCatalogConfig},
    sdk::SearchRequest,
    url,
};
use serde_json::Value;

const SOURCE: LunaPieces = LunaPieces;
const BASE_URL: &str = "https://lunapiecesfansub.com";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct LunaPieces;

impl MangaSource for LunaPieces {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        Ok(parse_listing(&MangaCatalog::fetch_document_or_fixture(
            &config,
            &catalog_url(page, order, ""),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = MangaCatalog::fetch_document_or_fixture(&config, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![MangaCatalog::parse_details(&body, Some(key), &config)],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&MangaCatalog::fetch_document_or_fixture(
            &config,
            &catalog_url(request.get("page").and_then(Value::as_u64).unwrap_or(1), "latest", query),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config();
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/doujinshi/sample".into());
        let body = MangaCatalog::fetch_document_or_fixture(&config, &url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(MangaCatalog::parse_details(&body, Some(key), &config))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config();
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/doujinshi/sample".into());
        let body = MangaCatalog::fetch_document_or_fixture(&config, &url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(MangaCatalog::parse_chapters(&body, &key, &config))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config();
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/doujinshi/sample/chapter-1".into());
        let body = MangaCatalog::fetch_document_or_fixture(&config, &url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(MangaCatalog::parse_pages(&body, &config))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = config();
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(MangaCatalog::parse_details(
                    &MangaCatalog::fetch_document_or_fixture(&config, input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
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

fn config() -> MangaCatalogConfig {
    MangaCatalogConfig {
        base_url: BASE_URL,
        name: "Luna Pieces",
        lang: LANG,
        content_rating: CONTENT_RATING,
    }
}

fn catalog_url(page: u64, order: &str, query: &str) -> String {
    let page_path = if page <= 1 { String::new() } else { format!("page/{page}/") };
    let mut out = format!("{BASE_URL}/doujinshi/{page_path}?m_orderby={order}");
    if !query.is_empty() {
        out.push_str("&s=");
        out.push_str(&url::query_escape(query));
        out.push_str("&post_type=wp-manga");
    }
    out
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<")
            .filter(|chunk| chunk.contains("/doujinshi/") && chunk.contains("href"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                if !href.contains("/doujinshi/") || href.contains("chapter") {
                    return None;
                }
                let key = normalize_key(&href);
                let title = html::attr_after(chunk, "<img", "alt")
                    .or_else(|| html::attr(chunk, "title"))
                    .or_else(|| url::slug_from_url(&key))
                    .unwrap_or_else(|| "Luna Pieces".into());
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: html::attr_after(chunk, "<img", "data-src")
                        .or_else(|| html::attr_after(chunk, "<img", "src"))
                        .map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("next") || body.contains("rel=\"next\""),
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!("/{}", input[BASE_URL.len()..].trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div><a href="/doujinshi/sample" title="Sample Luna"><img alt="Sample Luna" src="/cover.jpg"></a></div><a rel="next">Next</a>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample Luna</h1><img src="/cover.jpg"><div>Description</div><div>Summary.</div><div class="col-span-4"><a href="/doujinshi/sample/chapter-1">Chapter 1</a><span class="text-xs">1 enero, 2024</span></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="readerarea"><img src="/page1.jpg"><img data-src="/page2.jpg"></div>"#;
