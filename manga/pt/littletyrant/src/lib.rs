use manatan_extension::export_manga_source;
use manatan_shared::{
    impl_madara_source,
    manga::{MadaraConfig, MadaraSource},
};

const SOURCE: LittleTyrant = LittleTyrant;
const BASE_URL: &str = "https://tiraninha.world";

struct LittleTyrant;

impl MadaraSource for LittleTyrant {
    fn madara_config(&self, _: &serde_json::Value) -> MadaraConfig {
        MadaraConfig {
            base_url: BASE_URL,
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

impl_madara_source!(LittleTyrant);
export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail">
  <h3 class="post-title"><a href="/manga/sample/">Sample Little Tyrant</a></h3>
  <img src="/cover.jpg">
</div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="post-title">Sample Little Tyrant</h1>
<div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary">Sample description.</div>
<ul><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Capitulo 1</a><span class="chapter-release-date">01/01/2024</span></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="reading-content"><img class="wp-manga-chapter-img" src="/page-1.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use manatan_extension::source::MangaSource;
    use serde_json::json;

    #[test]
    fn parses_madara_fixtures() {
        assert_eq!(SOURCE.list(json!({"listingId":"popular"})).unwrap().entries.len(), 1);
        assert_eq!(SOURCE.chapters(json!({"manga":"/manga/sample"})).unwrap().len(), 1);
        assert_eq!(SOURCE.pages(json!({"chapter":"/manga/sample/chapter-1"})).unwrap().len(), 1);
    }
}
