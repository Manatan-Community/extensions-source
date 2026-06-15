use manatan_extension::export_manga_source;
use manatan_shared::{
    impl_manga_world_source,
    manga::{MangaWorldConfig, MangaWorldSource},
};

const SOURCE: MangaworldAdult = MangaworldAdult;

struct MangaworldAdult;

impl MangaWorldSource for MangaworldAdult {
    fn manga_world_config(&self) -> MangaWorldConfig {
        MangaWorldConfig {
            base_url: "https://www.mangaworldadult.net",
            name: "MangaworldAdult",
            lang: "it",
            content_rating: "adult",
        }
    }

    fn manga_world_list_fixture(&self) -> &'static str {
        LIST_FIXTURE
    }

    fn manga_world_details_fixture(&self) -> &'static str {
        DETAILS_FIXTURE
    }

    fn manga_world_pages_fixture(&self) -> &'static str {
        PAGES_FIXTURE
    }
}

impl_manga_world_source!(MangaworldAdult);
export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="comics-grid"><div class="entry"><a href="/manga/sample" title="Sample Manga"><img src="/cover.jpg"></a></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<section class="comic-info"><div class="thumb"><img src="/cover.jpg"></div><div class="meta-data"><a class="badge" href="/archive?genre=hentai">Hentai</a><a href="/archive?status=completed">Finito</a><a href="/archive?author=author">Author</a><a href="/archive?artist=artist">Artist</a></div></section>
<h1>Sample Manga</h1><div id="noidungm">Description</div>
<div class="chapters-wrapper"><div class="chapter"><a class="chap" href="/read/sample/chapter-1"><span class="d-inline-block">Capitolo 1</span></a><span class="chap-date">1 gennaio 2024</span></div></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="page"><img class="page-image" src="/page1.jpg"><img class="page-image" src="/page2.jpg"></div>"#;
