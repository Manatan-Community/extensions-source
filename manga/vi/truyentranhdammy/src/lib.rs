use manatan_extension::export_manga_source;
use manatan_shared::{
    impl_madara_source,
    manga::{MadaraConfig, MadaraSource},
};
use serde_json::Value;

const SOURCE: TruyenTranhDamMy = TruyenTranhDamMy;

struct TruyenTranhDamMy;

impl MadaraSource for TruyenTranhDamMy {
    fn madara_config(&self, _request: &Value) -> MadaraConfig {
        MadaraConfig {
            base_url: "https://truyentranhdammyy.site",
            lang: "vi",
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

impl_madara_source!(TruyenTranhDamMy);

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><div class="post-title"><a href="/manga/sample/">Sample</a></div><img src="/cover.jpg"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary">Summary</div><div class="genres-content"><a>Dam My</a></div>
<li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">2024-01-01</span></li>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use manatan_extension::source::MangaSource;
    use serde_json::json;

    #[test]
    fn parses_fixture() {
        let list = SOURCE.list(json!({})).unwrap();
        assert_eq!(list.entries[0].title, "Sample");

        let pages = SOURCE
            .pages(json!({"chapter": "/manga/sample/chapter-1"}))
            .unwrap();
        assert_eq!(pages.len(), 1);
    }
}
