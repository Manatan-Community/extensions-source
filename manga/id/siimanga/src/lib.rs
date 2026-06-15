use manatan_extension::export_manga_source;
use manatan_shared::{
    impl_madara_source,
    manga::{MadaraConfig, MadaraSource},
};
use serde_json::Value;

const SOURCE: Siimanga = Siimanga;

struct Siimanga;

impl MadaraSource for Siimanga {
    fn madara_config(&self, _request: &Value) -> MadaraConfig {
        MadaraConfig {
            base_url: "https://siikomik.net",
            lang: "id",
            content_rating: "adult",
            manga_path: "komik",
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

impl_madara_source!(Siimanga);
export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/komik/sample/">Sample Siikomik</a></h3><img src="/cover.jpg"></div>
<div class="nav-previous"></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample Siikomik</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary">Sample description.</div>
<ul class="main version-chap"><li class="wp-manga-chapter"><a href="/komik/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">01/01/2024</span></li></ul>
"#;

const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;
