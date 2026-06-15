use manatan_extension::export_manga_source;
use manatan_shared::{impl_madara_source, manga, manga::MadaraConfig};

const SOURCE: DoujinZa = DoujinZa;

struct DoujinZa;

impl manga::MadaraSource for DoujinZa {
    fn madara_config(&self, _request: &serde_json::Value) -> MadaraConfig {
        MadaraConfig {
            base_url: "https://doujinza.com",
            lang: "th",
            content_rating: "adult",
            manga_path: "doujin",
            popular_url_marker: "post-title",
            use_load_more: false,
            latest_enabled: false,
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

impl_madara_source!(DoujinZa);
export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><h3 class="post-title"><a href="/doujin/sample/">Sample Doujin</a></h3><img src="/cover.jpg"></div><div class="nav-previous"></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="post-title">Sample Doujin</h1><div class="summary_image"><img src="/cover.jpg"></div><ul><li class="wp-manga-chapter"><a href="/doujin/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">January 1, 2024</span></li></ul>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;
