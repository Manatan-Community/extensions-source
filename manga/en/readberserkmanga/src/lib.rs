use manatan_extension::export_manga_source;
use manatan_shared::{impl_manga_catalog_source, manga};
use serde_json::Value;

const SOURCE: ReadBerserk = ReadBerserk;
const CONFIG: manga::MangaCatalogConfig = manga::MangaCatalogConfig {
    base_url: "https://readberserk.com",
    name: "Read Berserk Manga",
    lang: "en",
    content_rating: "safe",
};
const FIXTURE: &str = r#"
<html><body>
<div class="container"><h1>Read Berserk Manga</h1></div>
<div class="px-6"><div class="flex-col">Description Berserk chapters Ongoing</div></div>
<div class="w-full"><div class="bg-bg-secondary"><div class="grid">
<div class="col-span-4"><a href="/manga/berserk-chapter-1">Chapter 1</a><span class="text-xs">The Fall</span></div>
</div></div></div>
<img data-src="/covers/berserk.jpg">
</body></html>
"#;
const PAGE_FIXTURE: &str =
    r#"<img data-src="/pages/berserk-1.jpg"><img data-src="/pages/berserk-2.jpg">"#;

struct ReadBerserk;

impl manga::MangaCatalogSource for ReadBerserk {
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

impl_manga_catalog_source!(ReadBerserk);

export_manga_source!(SOURCE);
