use manatan_extension::export_manga_source;
use manatan_shared::{
    impl_madara_source,
    manga::{MadaraConfig, MadaraSource},
};
use serde_json::Value;

const SOURCE: ArthurScan = ArthurScan;

struct ArthurScan;

impl MadaraSource for ArthurScan {
    fn madara_config(&self, _request: &Value) -> MadaraConfig {
        config()
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

impl_madara_source!(ArthurScan);

fn config() -> MadaraConfig {
    MadaraConfig {
        base_url: "https://arthurscan.xyz",
        lang: "pt-BR",
        content_rating: "safe",
        manga_path: "manga",
        popular_url_marker: "post-title",
        use_load_more: false,
        latest_enabled: true,
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample Arthur</a></h3><img src="/cover.jpg"></div>
<div class="nav-previous"></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample Arthur</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary">Sample description.</div>
<ul class="main version-chap"><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Capitulo 1</a><span class="chapter-release-date">janeiro 01, 2024</span></li></ul>
"#;

const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use manatan_extension::source::MangaSource;
    use serde_json::json;

    #[test]
    fn parses_madara_fixture() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Arthur"
        );
        assert_eq!(
            SOURCE
                .chapters(json!({"manga": "/manga/sample"}))
                .unwrap()
                .len(),
            1
        );
    }
}
