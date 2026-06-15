use manatan_extension::export_manga_source;
use manatan_shared::{
    impl_madara_source,
    manga::{MadaraConfig, MadaraSource},
};
use serde_json::Value;

const SOURCE: ArvenComics = ArvenComics;

struct ArvenComics;

impl MadaraSource for ArvenComics {
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

impl_madara_source!(ArvenComics);

fn config() -> MadaraConfig {
    MadaraConfig {
        base_url: "https://arvencomics.in",
        lang: "en",
        content_rating: "safe",
        manga_path: "comic",
        popular_url_marker: "post-title",
        use_load_more: false,
        latest_enabled: true,
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/comic/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div>
<div class="nav-previous"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample Manga</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<div class="post-status"><div class="post-content_item"><h5>Genres</h5><div class="summary-content"><a>Drama</a></div></div></div>
<ul class="main version-chap"><li class="wp-manga-chapter"><a href="/comic/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">01/01/2024</span></li></ul>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img src="/page1.jpg"><img src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_madara_source() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample Manga");

        let chapters = SOURCE.chapters(json!({"manga":"/comic/sample"})).unwrap();
        assert!(!chapters.is_empty());
    }
}
