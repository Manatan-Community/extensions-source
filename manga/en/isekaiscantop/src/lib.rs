use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: IsekaiScanTop = IsekaiScanTop;

struct IsekaiScanTop;

impl MangaSource for IsekaiScanTop {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(page_from_body(LIST_FIXTURE, &config));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request.get("listingId").and_then(Value::as_str);
        let path = if listing == Some("latest") {
            "latest-manga"
        } else {
            "popular-manga"
        };
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &format!("{}{path}?page={page}", base_prefix(&config)),
            LIST_FIXTURE,
        );
        Ok(page_from_body(&body, &config))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
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
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &format!(
                "{}search?page={page}&s={}",
                base_prefix(&config),
                url::query_escape(query)
            ),
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
        let mut chapters = manga::Madara::parse_chapters(&body, &key, &config);
        if chapters.len() == 1 && chapters[0].key == key {
            if let Some(id) = html::attr_after(&body, "manga-chapters-holder", "data-id") {
                let xhr = manga::Madara::browser_client(&config)
                    .get(format!(
                        "{}ajax-list-chapter?mangaID={id}",
                        base_prefix(&config)
                    ))
                    .xhr()
                    .send_text()
                    .unwrap_or_else(|_| AJAX_CHAPTERS_FIXTURE.to_string());
                chapters = manga::Madara::parse_chapters(&xhr, &key, &config);
            }
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config();
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), PAGES_FIXTURE);
        let array_pages = parse_arraydata_pages(&body, &config);
        if array_pages.is_empty() {
            Ok(manga::Madara::parse_pages(&body, &config))
        } else {
            Ok(array_pages)
        }
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
                query: input.to_string(),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn config() -> MadaraConfig {
    MadaraConfig {
        base_url: "https://isekaiscan.top",
        lang: "en",
        content_rating: "adult",
        manga_path: "manga",
        popular_url_marker: "post-title",
        use_load_more: false,
        latest_enabled: true,
    }
}

fn base_prefix(config: &MadaraConfig) -> String {
    format!("{}/", config.base_url.trim_end_matches('/'))
}

fn page_from_body(body: &str, config: &MadaraConfig) -> Paged<CatalogItem> {
    Paged {
        entries: manga::Madara::parse_listing(body, config),
        has_next_page: body.contains("pagination") && body.contains("next"),
    }
}

fn parse_arraydata_pages(body: &str, config: &MadaraConfig) -> Vec<MangaPage> {
    let Some(payload) = html::text_between(body, "id=\"arraydata\"", "</p>")
        .or_else(|| html::text_between(body, "id='arraydata'", "</p>"))
    else {
        return Vec::new();
    };
    payload
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: config.absolute_url(image),
                context: None,
            },
            headers: manga::image_headers(config.base_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail manga"><h3 class="post-title"><a href="/manga/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div>
<ul class="pagination"><li><a href="?page=2">next</a></li></ul>
"#;
const DETAILS_FIXTURE: &str = r#"
<div id="manga-chapters-holder" data-id="1"></div>
<div class="post-title"><h1>Sample Manga</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
"#;
const AJAX_CHAPTERS_FIXTURE: &str = r#"<ul><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<p id="arraydata">/page1.jpg,/page2.jpg</p>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_arraydata_pages() {
        let pages = SOURCE.pages(json!({})).unwrap();
        assert_eq!(pages.len(), 2);
    }
}
