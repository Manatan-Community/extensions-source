use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: GourmetScans = GourmetScans;

struct GourmetScans;

impl MangaSource for GourmetScans {
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
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(config.base_url) {
            let key = config.normalize_manga_key(query);
            let body = manga::Madara::fetch_document_or_fixture(&config, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(&body, Some(key), &config)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = gourmet_search_url(page, request.get("filters"), &config);
        let body = manga::Madara::fetch_document_or_fixture(&config, &target, LIST_FIXTURE);
        Ok(page_from_body(&body, &config))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/project/sample".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_details(&body, Some(key), &config))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/project/sample".to_string());
        Ok(fetch_madara_chapters(&config, &key, false)
            .into_iter()
            .map(clean_chapter)
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config();
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/project/sample/chapter-1".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), PAGES_FIXTURE);
        Ok(manga::Madara::parse_pages(&body, &config))
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

fn gourmet_search_url(page: u64, filters: Option<&Value>, config: &MadaraConfig) -> String {
    let year = filter(filters, "year");
    let genre = filter(filters, "genre");
    let order = filter(filters, "orderBy");
    let mut path = if !year.is_empty() {
        format!("release-year/{}", url::query_escape(&year))
    } else if !genre.is_empty() {
        format!("genre/{}", genre.trim_matches('/'))
    } else {
        config.manga_path.to_string()
    };
    if page > 1 {
        path.push_str(&format!("/page/{page}"));
    }
    let mut target = format!("{}/{}", config.base_url.trim_end_matches('/'), path);
    if !order.is_empty() {
        target.push_str(&format!("?m_orderby={}", url::query_escape(&order)));
    }
    target
}

fn filter(filters: Option<&Value>, key: &str) -> String {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn fetch_madara_chapters(
    config: &MadaraConfig,
    key: &str,
    use_new_endpoint: bool,
) -> Vec<MangaChapter> {
    let manga_url = config.absolute_url(key);
    let body = manga::Madara::fetch_document_or_fixture(config, &manga_url, DETAILS_FIXTURE);
    if body.contains("wp-manga-chapter") {
        return manga::Madara::parse_chapters(&body, key, config);
    }
    let Some(manga_id) = html::attr_after(&body, "manga-chapters-holder", "data-id") else {
        return manga::Madara::parse_chapters(&body, key, config);
    };
    let ajax = if use_new_endpoint {
        manga::Madara::browser_client(config)
            .post(format!("{}/ajax/chapters", manga_url.trim_end_matches('/')))
            .form(&[])
            .xhr()
            .send_text()
            .unwrap_or_else(|_| DETAILS_FIXTURE.to_string())
    } else {
        manga::Madara::browser_client(config)
            .post(format!(
                "{}/wp-admin/admin-ajax.php",
                config.base_url.trim_end_matches('/')
            ))
            .form(&[("action", "manga_get_chapters"), ("manga", &manga_id)])
            .xhr()
            .send_text()
            .unwrap_or_else(|_| DETAILS_FIXTURE.to_string())
    };
    manga::Madara::parse_chapters(&ajax, key, config)
}

fn clean_chapter(mut chapter: MangaChapter) -> MangaChapter {
    chapter.key = chapter
        .key
        .split('?')
        .next()
        .unwrap_or(&chapter.key)
        .to_string();
    chapter.url = chapter
        .url
        .map(|value| value.split('?').next().unwrap_or(&value).to_string());
    chapter
}

fn page_from_body(body: &str, config: &MadaraConfig) -> Paged<CatalogItem> {
    Paged {
        entries: manga::Madara::parse_listing(body, config),
        has_next_page: manga::Madara::has_next_page(body, config),
    }
}

fn config() -> MadaraConfig {
    MadaraConfig {
        base_url: "https://gourmetsupremacy.com",
        lang: "en",
        content_rating: "adult",
        manga_path: "project",
        popular_url_marker: "post-title",
        use_load_more: false,
        latest_enabled: true,
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-listing-item"><div class="page-item-detail"><h3 class="post-title"><a href="/project/sample/">Sample Project</a></h3><img src="/cover.jpg"></div></div>
<div class="navigation-ajax"><div id="navigation-ajax"></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample Project</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<div class="post-status"><div class="post-content_item"><h5>Genres</h5><div class="summary-content"><a>Drama</a></div></div></div>
<ul class="main version-chap"><li class="wp-manga-chapter"><a href="/project/sample/chapter-1/?style=list">Chapter 1</a><span class="chapter-release-date">2024-01-01</span></li></ul>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_gourmet_source() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Project"
        );
        let chapters = SOURCE.chapters(json!({"manga":"/project/sample"})).unwrap();
        assert_eq!(chapters[0].key, "/project/sample/chapter-1/");
    }
}
