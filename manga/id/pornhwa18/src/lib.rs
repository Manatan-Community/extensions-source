use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Pornhwa18 = Pornhwa18;
const CONFIG: manga::MadaraConfig = manga::MadaraConfig {
    base_url: "https://pornhwa18.com",
    lang: "id",
    content_rating: "adult",
    manga_path: "series",
    popular_url_marker: "<a",
    use_load_more: false,
    latest_enabled: true,
};

struct Pornhwa18;

impl MangaSource for Pornhwa18 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        Ok(parse_listing(&manga::Madara::fetch_document_or_fixture(
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
        Ok(parse_listing(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.search_url(page, query),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(manga::Madara::parse_details(
            &manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &url::join_url(CONFIG.base_url, &key),
                DETAILS_FIXTURE,
            ),
            Some(key),
            &CONFIG,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(manga::Madara::parse_chapters(
            &manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &url::join_url(CONFIG.base_url, &key),
                DETAILS_FIXTURE,
            ),
            &key,
            &CONFIG,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".into());
        Ok(manga::Madara::parse_pages(
            &manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &url::join_url(CONFIG.base_url, &key),
                PAGES_FIXTURE,
            ),
            &CONFIG,
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(CONFIG.base_url, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(CONFIG.base_url, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(CONFIG.base_url) {
            return Ok(Some(UrlResolveResult {
                item: Some(manga::Madara::parse_details(
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: manga::Madara::parse_listing(body, &CONFIG),
        has_next_page: manga::Madara::has_next_page(body, &CONFIG),
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><a href="https://pornhwa18.com/series/sample/"><img src="/cover.jpg" alt="Sample Pornhwa18"></a><h3><a href="https://pornhwa18.com/series/sample/">Sample Pornhwa18</a></h3></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample Pornhwa18</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<div class="summary__content"><p>Sample synopsis.</p></div>
<div class="post-content_item"><div class="summary-heading">Author</div><div class="summary-content"><a>Writer</a></div></div>
<div class="post-content_item"><div class="summary-heading">Genres</div><div class="summary-content"><a>Adult</a></div></div>
<ul><li class="wp-manga-chapter"><a href="https://pornhwa18.com/series/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">2024-01-01</span></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="reading-content"><img class="wp-manga-chapter-img" data-src="https://pornhwa18.com/page1.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(
            parse_listing(LIST_FIXTURE).entries[0].title,
            "Sample Pornhwa18"
        );
        assert_eq!(
            manga::Madara::parse_chapters(DETAILS_FIXTURE, "/series/sample", &CONFIG).len(),
            1
        );
        assert_eq!(manga::Madara::parse_pages(PAGES_FIXTURE, &CONFIG).len(), 1);
    }
}
