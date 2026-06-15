use manatan_extension::export_manga_source;
use manatan_shared::{impl_manga_catalog_source, manga};
use serde_json::Value;

const SOURCE: ReadBokuNoHero = ReadBokuNoHero;
const CONFIG: manga::MangaCatalogConfig = manga::MangaCatalogConfig {
    base_url: "https://ww10.readmha.com",
    name: "Read Boku no Hero Academia My Hero Academia Manga",
    lang: "en",
    content_rating: "safe",
};
const FIXTURE: &str = r#"
<html><body>
<div class="container"><h1>Read Boku no Hero Academia My Hero Academia Manga</h1></div>
<div class="px-6"><div class="flex-col">Description Hero academia chapters Ongoing</div></div>
<div class="w-full"><div class="bg-bg-secondary"><div class="grid">
<div class="col-span-4"><a href="/manga/my-hero-academia-chapter-1">Chapter 1</a><span class="text-xs">The Fall</span></div>
</div></div></div>
<img data-src="/covers/my-hero-academia.jpg">
</body></html>
"#;
const PAGE_FIXTURE: &str = r#"<img data-src="/pages/mha-1.jpg"><img data-src="/pages/mha-2.jpg">"#;

struct ReadBokuNoHero;

impl manga::MangaCatalogSource for ReadBokuNoHero {
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

impl_manga_catalog_source!(ReadBokuNoHero);

export_manga_source!(SOURCE);
