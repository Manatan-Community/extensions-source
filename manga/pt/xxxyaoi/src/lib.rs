use manatan_extension::{
    CatalogItem, Paged, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    manga,
    manga::{MadaraConfig, MadaraSource},
};
use serde_json::Value;

const SOURCE: XXXYaoi = XXXYaoi;

struct XXXYaoi;

impl manga::MadaraSource for XXXYaoi {
    fn madara_config(&self, _request: &Value) -> MadaraConfig {
        MadaraConfig {
            base_url: "https://3xyaoi.com",
            lang: "pt-BR",
            content_rating: "adult",
            manga_path: "bl",
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
}

impl MangaSource for XXXYaoi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        manga::MadaraSource::madara_list(self, request)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.madara_config(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(config.base_url) {
            return manga::MadaraSource::madara_search(self, request);
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let path = filters
            .get("status")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|status| status.to_string())
            .or_else(|| {
                filters
                    .get("genre")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(|genre| format!("genero/{genre}"))
            })
            .unwrap_or_default();
        let page_path = if page <= 1 {
            "page/1".to_string()
        } else {
            format!("page/{page}")
        };
        let target = if path.is_empty() {
            format!("{}/{page_path}/?s={}", config.base_url, manatan_shared::url::query_escape(query))
        } else {
            format!(
                "{}/{path}/{page_path}/?s={}",
                config.base_url,
                manatan_shared::url::query_escape(query)
            )
        };
        let body = manga::Madara::fetch_document_or_fixture(&config, &target, LIST_FIXTURE);
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &config),
            has_next_page: manga::Madara::has_next_page(&body, &config),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        manga::MadaraSource::madara_details(self, request)
    }
    fn chapters(&self, request: Value) -> ExtensionResult<Vec<manatan_extension::MangaChapter>> {
        manga::MadaraSource::madara_chapters(self, request)
    }
    fn pages(&self, request: Value) -> ExtensionResult<Vec<manatan_extension::MangaPage>> {
        manga::MadaraSource::madara_pages(self, request)
    }
    fn handle_url(
        &self,
        request: Value,
    ) -> ExtensionResult<Option<manatan_extension::UrlResolveResult>> {
        manga::MadaraSource::madara_handle_url(self, request)
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="page-item-detail manga"><h3 class="post-title"><a href="/bl/sample/">Sample</a></h3><img src="/cover.jpg"></div><div class="nav-previous"></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="post-title">Sample</h1><div class="summary_image"><img src="/cover.jpg"></div><ul><li class="wp-manga-chapter"><a href="/bl/sample/chapter-1/">Chapter 1</a></li></ul>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;
