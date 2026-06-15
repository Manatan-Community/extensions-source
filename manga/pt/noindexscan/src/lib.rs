use manatan_extension::export_manga_source;
use manatan_shared::{impl_madara_source, manga, manga::MadaraConfig};
use serde_json::Value;

const SOURCE: NoIndexScan = NoIndexScan;

struct NoIndexScan;

impl manga::MadaraSource for NoIndexScan {
    fn madara_config(&self, _request: &Value) -> MadaraConfig {
        MadaraConfig {
            base_url: "https://hanamiheaven.org",
            lang: "pt-BR",
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

impl_madara_source!(NoIndexScan);
export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail manga"><div class="item-thumb"><a href="/manga/sample/"><img src="/cover.jpg" alt="Sample"></a></div><div class="post-title"><h3><a href="/manga/sample/">Sample</a></h3></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary">Sample description.</div>
<li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">2024-01-01</span></li>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;
