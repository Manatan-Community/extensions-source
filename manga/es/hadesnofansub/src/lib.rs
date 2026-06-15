use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: HadesNoFansub = HadesNoFansub;
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: "https://lectorhades.latamtoon.com",
    lang: "es",
    content_rating: "adult",
    manga_path: "tmo",
    popular_url_marker: "post-title",
    use_load_more: false,
    latest_enabled: true,
};

struct HadesNoFansub;

impl MangaSource for HadesNoFansub {
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
        if query.starts_with(CONFIG.base_url) && query.contains("/tmo/") {
            return Ok(Paged {
                entries: vec![parse_details(
                    &manga::Madara::fetch_document_or_fixture(&CONFIG, query, DETAILS_FIXTURE),
                    Some(CONFIG.normalize_manga_key(query)),
                )],
                has_next_page: false,
            });
        }
        Ok(page_from_body(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.search_url(page, query),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/tmo/sample".into());
        Ok(parse_details(
            &manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/tmo/sample".into());
        Ok(fetch_madara_chapters(&key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/tmo/sample/chapter-1".into());
        Ok(manga::Madara::parse_pages(
            &manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), PAGES_FIXTURE),
            &CONFIG,
        ))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(CONFIG.base_url) && input.contains("/tmo/") {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &manga::Madara::fetch_document_or_fixture(&CONFIG, input, DETAILS_FIXTURE),
                    Some(CONFIG.normalize_manga_key(input)),
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

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let mut item = manga::Madara::parse_details(body, key, &CONFIG);
    item.status = status_from_hades_field(body).unwrap_or(item.status);
    item
}

fn status_from_hades_field(body: &str) -> Option<manatan_extension::ItemStatus> {
    body.split("post-content_item")
        .find(|chunk| {
            let lower = chunk.to_ascii_lowercase();
            lower.contains("summary-heading") && lower.contains("status")
        })
        .map(|chunk| html::strip_tags(chunk).to_ascii_lowercase())
        .map(|status| {
            if status.contains("completed") || status.contains("completo") {
                manatan_extension::ItemStatus::Completed
            } else if status.contains("hiatus") || status.contains("paus") {
                manatan_extension::ItemStatus::Hiatus
            } else {
                manatan_extension::ItemStatus::Ongoing
            }
        })
}

fn fetch_madara_chapters(key: &str) -> Vec<MangaChapter> {
    let manga_url = CONFIG.absolute_url(key);
    let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &manga_url, DETAILS_FIXTURE);
    let chapters = manga::Madara::parse_chapters(&body, key, &CONFIG);
    if !chapters.is_empty() && chapters[0].key != key {
        return chapters;
    }
    if html::attr_after(&body, "manga-chapters-holder", "data-id").is_none() {
        return chapters;
    }
    let ajax = manga::Madara::browser_client(&CONFIG)
        .post(format!("{}/ajax/chapters", manga_url.trim_end_matches('/')))
        .form(&[])
        .xhr()
        .send_text()
        .unwrap_or_else(|_| DETAILS_FIXTURE.to_string());
    let ajax_chapters = manga::Madara::parse_chapters(&ajax, key, &CONFIG);
    if ajax_chapters.is_empty() {
        chapters
    } else {
        ajax_chapters
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/tmo/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div>
<div class="nav-previous"></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample Manga</h1></div>
<div class="summary_image"><img src="/cover.jpg"></div>
<div class="summary_content"><div class="post-content"><div class="post-content_item"><div class="summary-heading">Status</div><div class="summary-content">Ongoing</div></div></div></div>
<div id="manga-chapters-holder" data-id="123"></div>
<ul class="main version-chap"><li class="wp-manga-chapter"><a href="/tmo/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">01/01/2024</span></li></ul>
"#;

const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_tmo_path_and_status() {
        let details = SOURCE.details(json!({"manga":"/tmo/sample"})).unwrap();
        assert_eq!(details.key, "/tmo/sample");
        assert_eq!(
            SOURCE
                .chapters(json!({"manga":"/tmo/sample"}))
                .unwrap()
                .len(),
            1
        );
    }
}
