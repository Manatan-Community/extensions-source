const SOURCE: ThaiMadaraSource = ThaiMadaraSource;
const CONFIG: ThaiMadaraConfig = ThaiMadaraConfig {
    base_url: "https://www.doodmanga.com",
    name: "Doodmanga",
    lang: "th",
    content_rating: "safe",
    manga_path: "manga",
    latest_enabled: true,
};

struct ThaiMadaraSource;

const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div><div class="nav-previous"></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="post-title">Sample Manga</h1><div class="summary_image"><img src="/cover.jpg"></div><div class="description-summary">Sample description.</div><ul><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">01/01/2024</span></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<div class="text-center"><p><img src="/page1.jpg"></p></div>"#;

include!("thai_madara_impl.rs");
