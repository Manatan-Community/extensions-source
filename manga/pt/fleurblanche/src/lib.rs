use manatan_extension::export_manga_source;
use manatan_shared::{
    impl_madara_source,
    manga::{MadaraConfig, MadaraSource},
};
use serde_json::Value;

const SOURCE: FleurBlanche = FleurBlanche;

struct FleurBlanche;

impl MadaraSource for FleurBlanche {
    fn madara_config(&self, _request: &Value) -> MadaraConfig {
        MadaraConfig {
            base_url: "https://fbsquadx.com",
            lang: "pt-BR",
            content_rating: "adult",
            manga_path: "manga",
            popular_url_marker: "post-title",
            use_load_more: true,
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

impl_madara_source!(FleurBlanche);
export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample Fleur</a></h3><img src="/cover.jpg"></div>
<div class="navigation-ajax"></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="post-title">Sample Fleur</h1><div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary">Sample description.</div>
<div class="post-content_item"><div>Status</div><div class="summary-content">Em andamento</div></div>
<ul><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Capitulo 1</a><span class="chapter-release-date">01/01/2024</span></li></ul>
"#;

const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page-1.jpg"></div>"#;
