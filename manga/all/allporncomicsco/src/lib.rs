use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest};
use serde_json::Value;

const SOURCE: AllPornComicsCo = AllPornComicsCo;

const CONFIG: manga::MadaraConfig = manga::MadaraConfig {
    base_url: "https://allporncomics.co",
    lang: "all",
    content_rating: "adult",
    manga_path: "comic",
    popular_url_marker: "<a",
    use_load_more: false,
    latest_enabled: false,
};

struct AllPornComicsCo;

impl MangaSource for AllPornComicsCo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.list_url(page, "views"),
            LIST_FIXTURE,
        );
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
            .unwrap_or_default()
            .trim();
        if query.starts_with(CONFIG.base_url) && query.contains("/comic/") {
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
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_details(&body, Some(key), &CONFIG))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_chapters(&body, &key, &CONFIG))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/comic/sample/chapter-1".into());
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), PAGES_FIXTURE);
        Ok(manga::Madara::parse_pages(&body, &CONFIG))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(CONFIG.base_url) && input.contains("/comic/") {
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
                query: input.to_string(),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<html><body>
  <div class="page-item-detail manga">
    <h3><a href="https://allporncomics.co/comic/sample-comic/">Sample Comic</a></h3>
    <img data-src="https://cdn.example/sample.jpg" alt="Sample Comic">
  </div>
  <div class="nav-previous"></div>
</body></html>
"#;

const DETAILS_FIXTURE: &str = r#"
<html><body>
  <div class="post-title"><h1>Sample Comic</h1></div>
  <div class="summary_image"><img src="https://cdn.example/cover.jpg"></div>
  <div class="description-summary"><p>A parsed description.</p></div>
  <div class="post-content_item"><div class="summary-heading">Author</div><div class="summary-content"><a>Author One</a></div></div>
  <div class="post-content_item"><div class="summary-heading">Genres</div><div class="summary-content"><a>Action</a><a>Drama</a></div></div>
  <li class="wp-manga-chapter"><a href="https://allporncomics.co/comic/sample-comic/chapter-1/">Chapter 1</a><span class="chapter-release-date">01/02/2024</span></li>
</body></html>
"#;

const PAGES_FIXTURE: &str = r#"
<html><body>
  <div class="reading-content">
    <img class="wp-manga-chapter-img" data-src="https://cdn.example/page-1.jpg">
    <img class="wp-manga-chapter-img" src="https://cdn.example/page-2.jpg">
  </div>
</body></html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing() {
        let entries = manga::Madara::parse_listing(LIST_FIXTURE, &CONFIG);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "/comic/sample-comic");
    }

    #[test]
    fn parses_details_and_chapters() {
        let item = manga::Madara::parse_details(DETAILS_FIXTURE, None, &CONFIG);
        assert_eq!(item.title, "Sample Comic");
        assert_eq!(item.authors, vec!["Author One"]);
        let chapters = manga::Madara::parse_chapters(DETAILS_FIXTURE, &item.key, &CONFIG);
        assert_eq!(chapters.len(), 1);
    }

    #[test]
    fn parses_pages() {
        let pages = manga::Madara::parse_pages(PAGES_FIXTURE, &CONFIG);
        assert_eq!(pages.len(), 2);
    }
}
