use manatan_extension::export_manga_source;
use manatan_shared::{impl_gallery_adults_source, manga};
use serde_json::Value;

const SOURCE: HentaiZap = HentaiZap;

const CONFIGS: [manga::GalleryAdultsConfig; 8] = [
    config("hentaizap-en", "en", "english"),
    config("hentaizap-ja", "ja", "japanese"),
    config("hentaizap-es", "es", "spanish"),
    config("hentaizap-fr", "fr", "french"),
    config("hentaizap-ko", "ko", "korean"),
    config("hentaizap-de", "de", "german"),
    config("hentaizap-ru", "ru", "russian"),
    config("hentaizap-all", "all", ""),
];

const fn config(
    source_id: &'static str,
    lang: &'static str,
    manga_lang: &'static str,
) -> manga::GalleryAdultsConfig {
    manga::GalleryAdultsConfig {
        base_url: "https://hentaizap.com",
        source_id,
        lang,
        manga_lang,
        content_rating: "adult",
        id_prefix_uri: "gallery",
        page_uri: "gallery",
        list_selector_marker: "thumb",
        image_container_marker: "gp_th",
    }
}

struct HentaiZap;

impl manga::GalleryAdultsSource for HentaiZap {
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

impl_gallery_adults_source!(HentaiZap);

fn config_for(request: &Value) -> &'static manga::GalleryAdultsConfig {
    let source_id = manga::GalleryAdults::source_id(request, "hentaizap-all");
    CONFIGS
        .iter()
        .find(|config| config.source_id == source_id)
        .unwrap_or(&CONFIGS[7])
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="thumb"><a href="https://hentaizap.com/gallery/123/"><img class="gp_th" src="https://img.example/123t.jpg" alt="Fixture Gallery"></a><div class="caption">Fixture Gallery</div><a><span class="th_lg"></span></a></div>
<ul class="pagination"><li class="active"></li><li></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Fixture Gallery</h1>
<div class="gp_cover"><img src="https://img.example/cover.jpg"></div>
<span class="info_txt">Tags:</span><li><a class="gp_btn_tag">Action</a><a class="gp_btn_tag">Drama</a></li>
<span class="info_txt">Artists:</span><li><a class="gp_btn_tag">Artist One</a></li>
<span class="info_txt">Groups:</span><li><a class="gp_btn_tag">Group One</a></li>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="gp_th"><img src="https://img.example/1.jpg"></div>
<div class="gp_th"><img src="https://img.example/2.jpg"></div>
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
