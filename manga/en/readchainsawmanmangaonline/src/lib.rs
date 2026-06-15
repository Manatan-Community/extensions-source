use manatan_extension::export_manga_source;
use manatan_shared::{impl_manga_catalog_source, manga};
use serde_json::Value;

const SOURCE: ReadChainsawMan = ReadChainsawMan;
const CONFIG: manga::MangaCatalogConfig = manga::MangaCatalogConfig {
    base_url: "https://ww5.readchainsawman.com",
    name: "Read Chainsaw Man Manga Online",
    lang: "en",
    content_rating: "adult",
};
const FIXTURE: &str = r#"
<html><body>
<div class="container"><h1>Read Chainsaw Man Manga Online</h1></div>
<div class="px-6"><div class="flex-col">Description Chainsaw Man chapters Ongoing</div></div>
<div class="w-full"><div class="bg-bg-secondary"><div class="grid">
<div class="col-span-4"><a href="/manga/chainsaw-man-chapter-1">Chapter 1</a><span class="text-xs">The Fall</span></div>
</div></div></div>
<img data-src="/covers/chainsaw-man.jpg">
</body></html>
"#;
const PAGE_FIXTURE: &str =
    r#"<img data-src="/pages/chainsaw-man-1.jpg"><img data-src="/pages/chainsaw-man-2.jpg">"#;

struct ReadChainsawMan;

impl manga::MangaCatalogSource for ReadChainsawMan {
    fn manga_catalog_config(&self, _request: &Value) -> &manga::MangaCatalogConfig {
        &CONFIG
    }

    fn manga_catalog_details_fixture(&self) -> &'static str {
        FIXTURE
    }

    fn manga_catalog_pages_fixture(&self) -> &'static str {
        PAGE_FIXTURE
    }
}

impl_manga_catalog_source!(ReadChainsawMan);

export_manga_source!(SOURCE);
