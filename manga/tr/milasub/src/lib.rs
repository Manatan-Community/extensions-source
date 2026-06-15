use manatan_extension::export_manga_source;
use manatan_shared::{impl_madara_source, manga, manga::MadaraConfig};

const SOURCE: MilaSub = MilaSub;

struct MilaSub;

impl manga::MadaraSource for MilaSub {
    fn madara_config(&self, _request: &serde_json::Value) -> MadaraConfig {
        MadaraConfig {
            base_url: "https://www.millascan.com",
            lang: "tr",
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
}

impl_madara_source!(MilaSub);
export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div><div class="nav-previous"></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="post-title">Sample Manga</h1><div class="summary_image"><img src="/cover.jpg"></div><ul><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">1 Ocak 2024</span></li></ul>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;
