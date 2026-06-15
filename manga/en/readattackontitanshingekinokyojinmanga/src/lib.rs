use manatan_extension::export_manga_source;
use manatan_shared::{impl_manga_catalog_source, manga};
use serde_json::Value;

const SOURCE: ReadAttackOnTitan = ReadAttackOnTitan;
const CONFIG: manga::MangaCatalogConfig = manga::MangaCatalogConfig {
    base_url: "https://ww11.readsnk.com",
    name: "Read Attack on Titan Shingeki no Kyojin Manga",
    lang: "en",
    content_rating: "safe",
};
const FIXTURE: &str = r#"
<html><body>
<div class="container"><h1>Read Attack on Titan Shingeki no Kyojin Manga</h1></div>
<div class="px-6"><div class="flex-col">Description Survey Corps chapters Completed</div></div>
<div class="w-full"><div class="bg-bg-secondary"><div class="grid">
<div class="col-span-4"><a href="/manga/attack-on-titan-chapter-1">Chapter 1</a><span class="text-xs">The Fall</span></div>
</div></div></div>
<img data-src="/covers/attack-on-titan.jpg">
</body></html>
"#;
const PAGE_FIXTURE: &str = r#"<img data-src="/pages/aot-1.jpg"><img data-src="/pages/aot-2.jpg">"#;

struct ReadAttackOnTitan;

impl manga::MangaCatalogSource for ReadAttackOnTitan {
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

impl_manga_catalog_source!(ReadAttackOnTitan);

export_manga_source!(SOURCE);
