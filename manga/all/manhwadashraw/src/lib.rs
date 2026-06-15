use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, url};
use serde_json::Value;

const SOURCE: ManhwaDashRaw = ManhwaDashRaw;
const CONFIG: manga::MadaraConfig = manga::MadaraConfig {
    base_url: "https://manhwa-raw.com",
    lang: "all",
    content_rating: "adult",
    manga_path: "manga",
    popular_url_marker: "<a",
    use_load_more: false,
    latest_enabled: true,
};

struct ManhwaDashRaw;

impl MangaSource for ManhwaDashRaw {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = CONFIG.list_url(page, if latest { "latest" } else { "views" });
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &target, LIST_FIXTURE);
        Ok(Paged {
            entries: filter_source(manga::Madara::parse_listing(&body, &CONFIG)),
            has_next_page: true,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(CONFIG.base_url) && query.contains("/manga/") {
            let body = manga::Madara::fetch_document_or_fixture(&CONFIG, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(
                    &body,
                    Some(CONFIG.normalize_manga_key(query)),
                    &CONFIG,
                )],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(request);
        }
        let target = CONFIG.search_url(page, query);
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &target, LIST_FIXTURE);
        Ok(Paged {
            entries: filter_source(manga::Madara::parse_listing(&body, &CONFIG)),
            has_next_page: true,
        })
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
        Ok(manga::Madara::parse_chapters(&body, &key, &CONFIG))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), PAGES_FIXTURE);
        Ok(manga::Madara::parse_pages(&body, &CONFIG))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(CONFIG.base_url) && input.contains("/manga/") {
            let body = manga::Madara::fetch_document_or_fixture(&CONFIG, input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(manga::Madara::parse_details(
                    &body,
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
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

fn filter_source(entries: Vec<CatalogItem>) -> Vec<CatalogItem> {
    entries
        .into_iter()
        .map(|mut entry| {
            entry.language = Some("all".into());
            entry
        })
        .collect()
}

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail manga"><a href="https://manhwa-raw.com/manga/sample" title="Sample Raw">Sample Raw</a><img data-src="/cover.jpg"></div>
<div class="nav-previous"><a href="/manga/page/2">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample Raw</h1></div>
<div class="summary_image"><img src="/cover.jpg"></div>
<div class="summary__content">Adult sample.</div>
<li class="wp-manga-chapter"><a href="https://manhwa-raw.com/manga/sample/chapter-1">Chapter 1</a><span class="chapter-release-date">01/01/2024</span></li>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="page-break"><img class="wp-manga-chapter-img" src="/page-1.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manhwadashraw() {
        assert_eq!(
            filter_source(manga::Madara::parse_listing(LIST_FIXTURE, &CONFIG)).len(),
            1
        );
        assert_eq!(
            manga::Madara::parse_details(DETAILS_FIXTURE, None, &CONFIG).title,
            "Sample Raw"
        );
        assert_eq!(
            manga::Madara::parse_chapters(DETAILS_FIXTURE, "/manga/sample", &CONFIG).len(),
            1
        );
        assert_eq!(manga::Madara::parse_pages(PAGES_FIXTURE, &CONFIG).len(), 1);
    }
}
