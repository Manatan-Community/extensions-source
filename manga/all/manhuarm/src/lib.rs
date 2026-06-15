use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::manga::{MadaraConfig, MadaraSource};
use serde_json::Value;

const SOURCE: Manhuarm = Manhuarm;

struct Manhuarm;

impl MadaraSource for Manhuarm {
    fn madara_config(&self, _request: &Value) -> MadaraConfig {
        MadaraConfig {
            base_url: "https://manhuarmtl.com",
            lang: "all",
            content_rating: "adult",
            manga_path: "manga",
            popular_url_marker: "post-title",
            use_load_more: false,
            latest_enabled: true,
        }
    }

    fn madara_list_fixture(&self) -> &'static str {
        LIST_FIXTURE
    }

    fn madara_details_fixture(&self) -> &'static str {
        DETAILS_FIXTURE
    }

    fn madara_pages_fixture(&self) -> &'static str {
        PAGES_FIXTURE
    }

    fn madara_listing_order(&self, request: &Value) -> &'static str {
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "trending"
        }
    }
}

impl MangaSource for Manhuarm {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        self.madara_list(request)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        self.madara_search(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        self.madara_details(request)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        self.madara_chapters(request)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        self.madara_pages(request)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        self.madara_handle_url(request)
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail post-title">
  <a href="https://manhuarmtl.com/manga/sample-series/">Sample Series</a>
  <img data-src="https://manhuarmtl.com/uploads/sample.jpg" />
</div>
<a class="nextpostslink" href="/manga/page/2/?m_orderby=trending">Next</a>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample Series</h1></div>
<div class="summary_image"><img data-src="https://manhuarmtl.com/uploads/sample.jpg" /></div>
<div class="description-summary"><p>A translated manhua sample.</p></div>
<div class="post-status"><div class="post-content_item"><div class="summary-heading">Status</div><div class="summary-content">OnGoing</div></div></div>
<div class="genres-content"><a>Action</a><a>Fantasy</a></div>
<ul class="main version-chap">
  <li class="wp-manga-chapter"><a href="https://manhuarmtl.com/manga/sample-series/chapter-1/">Chapter 1</a><span class="chapter-release-date">January 2, 2025</span></li>
</ul>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="reading-content">
  <img class="wp-manga-chapter-img" data-src="https://manhuarmtl.com/uploads/sample-page-1.jpg" />
  <img class="wp-manga-chapter-img" data-src="https://manhuarmtl.com/uploads/sample-page-2.jpg" />
</div>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fixture_listing_and_pages() {
        let list = SOURCE.list(json!({})).unwrap();
        assert_eq!(list.entries[0].title, "Sample Series");
        assert!(list.has_next_page);

        let pages = SOURCE.pages(json!({ "chapter": "/manga/sample-series/chapter-1" })).unwrap();
        assert_eq!(pages.len(), 2);
    }
}
