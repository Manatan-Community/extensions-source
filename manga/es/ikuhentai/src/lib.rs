use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Ikuhentai = Ikuhentai;
const BASE_URL: &str = "https://ikuhentai.net";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct Ikuhentai;

impl MangaSource for Ikuhentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: manga::Madara::parse_listing(LIST_FIXTURE, &config),
                has_next_page: manga::Madara::has_next_page(LIST_FIXTURE, &config),
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if listing_id(&request) == "latest" {
            "latest"
        } else {
            "views"
        };
        let body = manga::Madara::fetch_document_or_fixture(&config, &list_url(page, order), LIST_FIXTURE);
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &config),
            has_next_page: manga::Madara::has_next_page(&body, &config),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = config.normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &manga::Madara::fetch_document_or_fixture(&config, query, DETAILS_FIXTURE),
                    Some(key),
                    &config,
                )],
                has_next_page: false,
            });
        }
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &search_url(page, query, request.get("filters").unwrap_or(&Value::Null)),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &config),
            has_next_page: manga::Madara::has_next_page(&body, &config),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
            &config,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let manga_url = config.absolute_url(&key);
        let body = manga::Madara::browser_client(&config)
            .post(format!(
                "{}/ajax/chapters/",
                manga_url.trim_end_matches('/')
            ))
            .form(&[])
            .xhr()
            .send_text()
            .unwrap_or_else(|_| DETAILS_FIXTURE.to_string());
        Ok(manga::Madara::parse_chapters(&body, &key, &config)
            .into_iter()
            .map(|mut chapter| {
                let clean_url = chapter
                    .url
                    .clone()
                    .unwrap_or_else(|| config.absolute_url(&chapter.key));
                let mut normalized = clean_url
                    .split('?')
                    .next()
                    .unwrap_or(&clean_url)
                    .to_string();
                if !normalized.contains("style=list") {
                    normalized.push_str(if normalized.contains('?') {
                        "&style=list"
                    } else {
                        "?style=list"
                    });
                }
                chapter.key = config.normalize_manga_key(&normalized);
                chapter.url = Some(normalized);
                chapter.language = Some(LANG.to_string());
                chapter
            })
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config();
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), PAGES_FIXTURE);
        Ok(manga::Madara::parse_pages(&body, &config))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = config();
        Ok(manga::request_key(&request, "manga").map(|key| config.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = config();
        Ok(manga::request_key(&request, "chapter").map(|key| config.absolute_url(&key)))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let config = config();
        let popular = manga::Madara::parse_listing(
            &manga::Madara::fetch_document_or_fixture(&config, &list_url(1, "views"), LIST_FIXTURE),
            &config,
        );
        let latest = manga::Madara::parse_listing(
            &manga::Madara::fetch_document_or_fixture(&config, &list_url(1, "latest"), LIST_FIXTURE),
            &config,
        );
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular,
                has_more: true,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest,
                has_more: true,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = config();
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
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

fn config() -> MadaraConfig {
    MadaraConfig {
        base_url: BASE_URL,
        lang: LANG,
        content_rating: CONTENT_RATING,
        manga_path: "manga",
        popular_url_marker: "post-title",
        use_load_more: false,
        latest_enabled: true,
    }
}

fn list_url(page: u64, order: &str) -> String {
    let page_path = if page > 1 {
        format!("page/{page}/")
    } else {
        String::new()
    };
    format!("{BASE_URL}/{page_path}?s=&post_type=wp-manga&m_orderby={order}")
}

fn search_url(page: u64, query: &str, filters: &Value) -> String {
    let page_path = if page > 1 {
        format!("page/{page}/")
    } else {
        String::new()
    };
    let mut params = vec![
        format!("s={}", url::query_escape(query)),
        "post_type=wp-manga".to_string(),
    ];
    for genre in filter_array(filters, "genres") {
        params.push(format!("genre%5B%5D={}", url::query_escape(&genre)));
    }
    for status in filter_array(filters, "status") {
        params.push(format!("status%5B%5D={}", url::query_escape(&status)));
    }
    if let Some(sort) = filter_str(filters, "sort").filter(|value| !value.is_empty()) {
        params.push(format!("m_orderby={}", url::query_escape(&sort)));
    }
    for field in ["author", "release"] {
        if let Some(value) = filter_str(filters, field) {
            params.push(format!("{field}={}", url::query_escape(&value)));
        }
    }
    format!("{BASE_URL}/{page_path}?{}", params.join("&"))
}

fn parse_details(body: &str, key: Option<String>, config: &MadaraConfig) -> CatalogItem {
    let mut item = manga::Madara::parse_details(body, key, config);
    item.status = status_from_body(body);
    item
}

fn status_from_body(body: &str) -> ItemStatus {
    let status = body
        .split("post-content_item")
        .find(|chunk| chunk.to_ascii_lowercase().contains("estado"))
        .map(html::strip_tags)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if status.contains("completado") || status.contains("finalizado") {
        ItemStatus::Completed
    } else if status.contains("pausado") || status.contains("on-hold") {
        ItemStatus::Hiatus
    } else if status.contains("cancelado") {
        ItemStatus::Cancelled
    } else if status.contains("ongoing") || status.contains("emision") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn filter_str(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn filter_array(filters: &Value, key: &str) -> Vec<String> {
    filters
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

const LIST_FIXTURE: &str = r#"<div class="page-listing-item"><div class="page-item-detail"><div class="item-thumb"><a href="/manga/sample/" title="Sample"><img src="/cover.jpg"></a></div><h3 class="post-title"><a href="/manga/sample/">Sample</a></h3></div></div><a class="nextpostslink"></a>"#;
const DETAILS_FIXTURE: &str = r#"<div class="site-content"><div class="post-title"><h1>Sample</h1></div><div class="summary_image"><img src="/cover.jpg"></div><div class="author-content">Author</div><div class="artist-content">Artist</div><div class="genres-content"><a>Tag</a></div><div class="post-content_item"><h5>Estado</h5><div class="summary-content">Completado</div></div><div class="description-summary">Summary</div><ul><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/?style=list">Chapter 1</a><span class="chapter-release-date"><i>enero 01, 2024</i></span></li></ul></div>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img data-lazy-src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
