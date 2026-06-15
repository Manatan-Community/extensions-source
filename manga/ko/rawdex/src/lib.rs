use manatan_extension::export_manga_source;
use manatan_shared::{
    impl_madara_source,
    manga::{MadaraConfig, MadaraSource},
};
use serde_json::Value;

const SOURCE: RawDex = RawDex;

struct RawDex;

impl MadaraSource for RawDex {
    fn madara_config(&self, _request: &Value) -> MadaraConfig {
        MadaraConfig {
            base_url: "https://rawdex.net",
            lang: "ko",
            content_rating: "adult",
            manga_path: "manga",
            popular_url_marker: "<a",
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

impl_madara_source!(RawDex);
export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail manga"><a href="https://rawdex.net/manga/sample"><img src="/cover.jpg" alt="Sample RawDEX"></a><h3><a href="https://rawdex.net/manga/sample">Sample RawDEX</a></h3></div>
<div class="nav-previous"><a href="/manga/page/2/">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample RawDEX</h1></div>
<div class="summary_image"><img src="/cover.jpg"></div>
<div class="summary__content">Sample description</div>
<div class="summary-heading"><h5>Status</h5></div><div>Ongoing</div>
<ul><li class="wp-manga-chapter"><a href="https://rawdex.net/manga/sample/chapter-1">Chapter 1</a></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>
"#;
