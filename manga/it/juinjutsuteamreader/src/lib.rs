use manatan_extension::export_manga_source;
use manatan_shared::{
    impl_foolslide_source,
    manga::{FoolSlideConfig, FoolSlideSource},
};

const SOURCE: JuinJutsuTeamReader = JuinJutsuTeamReader;

struct JuinJutsuTeamReader;

impl FoolSlideSource for JuinJutsuTeamReader {
    fn foolslide_config(&self) -> FoolSlideConfig {
        FoolSlideConfig {
            base_url: "https://www.juinjutsureader.ovh",
            name: "Juin Jutsu Team Reader",
            lang: "it",
            content_rating: "safe",
            url_modifier: "",
            popular_uses_latest: true,
        }
    }

    fn foolslide_list_fixture(&self) -> &'static str {
        LIST_FIXTURE
    }

    fn foolslide_details_fixture(&self) -> &'static str {
        DETAILS_FIXTURE
    }

    fn foolslide_pages_fixture(&self) -> &'static str {
        PAGES_FIXTURE
    }
}

impl_foolslide_source!(JuinJutsuTeamReader);
export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="group"><a title="Sample Manga" href="/series/sample"><img src="/thumb_cover.jpg"></a></div><div class="next"><a></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample Manga</h1><div class="info"><b>Autore</b>: Author<br><b>Trama</b>: Description</div><div class="group_comic"><div class="element"><a title="Chapter 1" href="/read/sample/1/1/">Chapter 1</a><div class="meta_r">2024.01.01</div></div></div>"#;
const PAGES_FIXTURE: &str = r#"var pages = [{"url":"/page1.jpg"},{"url":"/page2.jpg"}];"#;
