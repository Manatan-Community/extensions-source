use manatan_extension::export_manga_source;
use manatan_shared::{impl_gallery_adults_source, manga};
use serde_json::Value;

const SOURCE: HentaiFox = HentaiFox;

const CONFIGS: [manga::GalleryAdultsConfig; 5] = [
    config("hentaifox-en", "en", "english"),
    config("hentaifox-ja", "ja", "japanese"),
    config("hentaifox-zh", "zh", "chinese"),
    config("hentaifox-ko", "ko", "korean"),
    config("hentaifox-all", "all", ""),
];

const fn config(
    source_id: &'static str,
    lang: &'static str,
    manga_lang: &'static str,
) -> manga::GalleryAdultsConfig {
    manga::GalleryAdultsConfig {
        base_url: "https://hentaifox.com",
        source_id,
        lang,
        manga_lang,
        content_rating: "adult",
        id_prefix_uri: "gallery",
        page_uri: "gallery",
        list_selector_marker: "thumb",
        image_container_marker: "thumb",
    }
}

struct HentaiFox;

impl manga::GalleryAdultsSource for HentaiFox {
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

impl_gallery_adults_source!(HentaiFox);

fn config_for(request: &Value) -> &'static manga::GalleryAdultsConfig {
    let source_id = manga::GalleryAdults::source_id(request, "hentaifox-all");
    CONFIGS
        .iter()
        .find(|config| config.source_id == source_id)
        .unwrap_or(&CONFIGS[4])
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="thumb" data-languages="1"><div class="inner_thumb"><a href="https://hentaifox.com/gallery/123/"><img src="https://img.example/123t.jpg" alt="Fixture Gallery"></a></div><div class="caption">Fixture Gallery</div></div>
<ul class="pagination"><li class="active"></li><li></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Fixture Gallery</h1>
<div class="cover"><img src="https://img.example/cover.jpg"></div>
<ul class="tags"><li><a>Action</a><a>Drama</a></li></ul>
<ul class="artists"><li><a>Artist One</a></li></ul>
<ul class="groups"><li><a>Group One</a></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="thumb"><img src="https://img.example/1.jpg"></div>
<div class="thumb"><img src="https://img.example/2.jpg"></div>
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
