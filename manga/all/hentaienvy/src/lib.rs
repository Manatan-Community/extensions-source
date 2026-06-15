use manatan_extension::export_manga_source;
use manatan_shared::{impl_gallery_adults_source, manga};
use serde_json::Value;

const SOURCE: HentaiEnvy = HentaiEnvy;

const CONFIGS: [manga::GalleryAdultsConfig; 8] = [
    config("hentaienvy-en", "en", "english"),
    config("hentaienvy-ja", "ja", "japanese"),
    config("hentaienvy-es", "es", "spanish"),
    config("hentaienvy-fr", "fr", "french"),
    config("hentaienvy-ko", "ko", "korean"),
    config("hentaienvy-de", "de", "german"),
    config("hentaienvy-ru", "ru", "russian"),
    config("hentaienvy-all", "all", ""),
];

const fn config(
    source_id: &'static str,
    lang: &'static str,
    manga_lang: &'static str,
) -> manga::GalleryAdultsConfig {
    manga::GalleryAdultsConfig {
        base_url: "https://hentaienvy.com",
        source_id,
        lang,
        manga_lang,
        content_rating: "adult",
        id_prefix_uri: "gallery",
        page_uri: "gallery",
        list_selector_marker: "thumb",
        image_container_marker: "th_gp",
    }
}

struct HentaiEnvy;

impl manga::GalleryAdultsSource for HentaiEnvy {
    fn gallery_config(&self, request: &Value) -> &manga::GalleryAdultsConfig {
        config_for(request)
    }

    fn gallery_list_fixture(&self) -> &'static str {
        LIST_FIXTURE
    }

    fn gallery_details_fixture(&self) -> &'static str {
        DETAILS_FIXTURE
    }

    fn gallery_pages_fixture(&self) -> &'static str {
        PAGES_FIXTURE
    }
}

impl_gallery_adults_source!(HentaiEnvy);

fn config_for(request: &Value) -> &'static manga::GalleryAdultsConfig {
    let source_id = manga::GalleryAdults::source_id(request, "hentaienvy-all");
    CONFIGS
        .iter()
        .find(|config| config.source_id == source_id)
        .unwrap_or(&CONFIGS[7])
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="thumb"><a href="https://hentaienvy.com/gallery/123/"><img class="th_img" src="https://img.example/123t.jpg" alt="Fixture Gallery"></a><div class="caption">Fixture Gallery</div><div class="flag"><a href="/language/english/">English</a></div></div>
<ul class="pagination"><li class="active"></li><li></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Fixture Gallery</h1>
<div class="gt_left"><img src="https://img.example/cover.jpg"></div>
<ul><li><span class="tag_title">Tags:</span><a class="gp_tag">Action</a><a class="gp_tag">Drama</a></li></ul>
<ul><li><span class="tag_title">Artists:</span><a class="gp_tag">Artist One</a></li></ul>
<ul><li><span class="tag_title">Groups:</span><a class="gp_tag">Group One</a></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="th_gp"><img src="https://img.example/1.jpg"></div>
<div class="th_gp"><img src="https://img.example/2.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gallery_adults_source() {
        let config = &CONFIGS[0];
        assert_eq!(manga::GalleryAdults::parse_listing(LIST_FIXTURE, config).len(), 1);
        assert_eq!(
            manga::GalleryAdults::parse_chapters(DETAILS_FIXTURE, "/gallery/123", config).len(),
            1
        );
        assert_eq!(manga::GalleryAdults::parse_pages(PAGES_FIXTURE, config).len(), 2);
    }
}
