use manatan_extension::export_manga_source;
use manatan_shared::{
    impl_manga_world_source,
    manga::{MangaWorldConfig, MangaWorldSource},
};

const SOURCE: Mangaworld = Mangaworld;

struct Mangaworld;

impl MangaWorldSource for Mangaworld {
    fn manga_world_config(&self) -> MangaWorldConfig {
        MangaWorldConfig {
            base_url: "https://www.mangaworld.mx",
            name: "Mangaworld",
            lang: "it",
            content_rating: "safe",
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

impl_manga_world_source!(Mangaworld);
export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="comics-grid"><div class="entry"><a href="/manga/sample" title="Sample Manga"><img src="/cover.jpg"></a></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<section class="comic-info"><div class="thumb"><img src="/cover.jpg"></div><div class="meta-data"><a class="badge" href="/archive?genre=azione">Azione</a><a href="/archive?status=ongoing">In corso</a><a href="/archive?author=author">Author</a><a href="/archive?artist=artist">Artist</a></div></section>
<h1>Sample Manga</h1><div id="noidungm">Description</div>
<div class="chapters-wrapper"><div class="chapter"><a class="chap" href="/read/sample/chapter-1"><span class="d-inline-block">Capitolo 1</span></a><span class="chap-date">1 gennaio 2024</span></div></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="page"><img class="page-image" src="/page1.jpg"><img class="page-image" src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use manatan_extension::source::MangaSource;
    use serde_json::json;

    #[test]
    fn parses_mangaworld_fixture() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries[0].title, "Sample Manga");
        assert_eq!(SOURCE.chapters(json!({"manga":"/manga/sample"})).unwrap()[0].chapter_number, Some(1.0));
        assert_eq!(SOURCE.pages(json!({"chapter":"/read/sample/chapter-1"})).unwrap().len(), 2);
    }
}
