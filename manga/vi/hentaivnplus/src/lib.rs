use manatan_extension::export_manga_source;
use manatan_shared::{
    impl_madara_source,
    manga::{MadaraConfig, MadaraSource},
};
use serde_json::Value;

const SOURCE: HentaiVNPlus = HentaiVNPlus;
const DEFAULT_BASE_URL: &str = "https://hentaivn.show";

struct HentaiVNPlus;

impl MadaraSource for HentaiVNPlus {
    fn madara_config(&self, request: &Value) -> MadaraConfig {
        MadaraConfig {
            base_url: base_url(request),
            lang: "vi",
            content_rating: "adult",
            manga_path: "truyen-hentai",
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
        match request
            .get("filters")
            .and_then(|filters| filters.get("sort"))
            .and_then(Value::as_str)
        {
            Some("latest") => "latest",
            Some("new-manga") => "new-manga",
            Some("alphabet") => "alphabet",
            Some("rating") => "rating",
            Some("trending") => "trending",
            _ if request.get("listingId").and_then(Value::as_str) == Some("latest") => "latest",
            _ => "views",
        }
    }
}

impl_madara_source!(HentaiVNPlus);

fn base_url(request: &Value) -> &'static str {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("overrideBaseUrl"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .map(|value| {
            Box::leak(value.trim_end_matches('/').to_string().into_boxed_str()) as &'static str
        })
        .unwrap_or(DEFAULT_BASE_URL)
}

const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><div class="post-title"><a href="/truyen-hentai/sample/">Sample</a></div><img src="/cover.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="post-title"><h1>Sample</h1></div><div class="summary_image"><img src="/cover.jpg"></div><div class="description-summary">Summary</div><li class="wp-manga-chapter"><a href="/truyen-hentai/sample/chapter-1/">Chapter 1</a></li>"#;
const PAGES_FIXTURE: &str = r#"<div class="reading-content"><img src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
