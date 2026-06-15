use manatan_extension::export_manga_source;
use manatan_shared::{impl_gallery_adults_source, manga};
use serde_json::Value;

const SOURCE: NHentaiXxx = NHentaiXxx;

const CONFIGS: [manga::GalleryAdultsConfig; 4] = [
    config("nhentaixxx-en", "en", "english"),
    config("nhentaixxx-ja", "ja", "japanese"),
    config("nhentaixxx-zh", "zh", "chinese"),
    config("nhentaixxx-all", "all", ""),
];

const fn config(
    source_id: &'static str,
    lang: &'static str,
    manga_lang: &'static str,
) -> manga::GalleryAdultsConfig {
    manga::GalleryAdultsConfig {
        base_url: "https://nhentai.xxx",
        source_id,
        lang,
        manga_lang,
        content_rating: "adult",
        id_prefix_uri: "g",
        page_uri: "g",
        list_selector_marker: "gallery_item",
        image_container_marker: "gallery_item",
    }
}

struct NHentaiXxx;

impl manga::GalleryAdultsSource for NHentaiXxx {
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

impl_gallery_adults_source!(NHentaiXxx);

fn config_for(request: &Value) -> &'static manga::GalleryAdultsConfig {
    let source_id = manga::GalleryAdults::source_id(request, "nhentaixxx-all");
    CONFIGS
        .iter()
        .find(|config| config.source_id == source_id)
        .unwrap_or(&CONFIGS[3])
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="gallery_item"><a href="https://nhentai.xxx/g/123/"><img class="gthumb" src="https://img.example/123t.jpg" alt="Fixture Gallery"></a><div class="gallery_title">Fixture Gallery</div><a><span class="thumb_flag"></span></a></div>
<ul class="pagination"><li class="active"></li><li></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Fixture Gallery</h1>
<div class="left_cover"><img src="https://img.example/cover.jpg"></div>
<li><span class="tags_text">Tags</span><a class="tag"><span class="item_name">Action</span></a><a class="tag"><span class="item_name">Drama</span></a></li>
<li><span class="tags_text">Artists</span><a class="tag"><span class="item_name">Artist One</span></a></li>
<li><span class="tags_text">Groups</span><a class="tag"><span class="item_name">Group One</span></a></li>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="gthumb"><img src="https://img.example/1.jpg"></div>
<div class="gthumb"><img src="https://img.example/2.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gallery_adults_source() {
        let config = &CONFIGS[0];
        assert_eq!(
            manga::GalleryAdults::parse_listing(LIST_FIXTURE, config).len(),
            1
        );
        assert_eq!(
            manga::GalleryAdults::parse_chapters(DETAILS_FIXTURE, "/g/123", config).len(),
            1
        );
        assert_eq!(
            manga::GalleryAdults::parse_pages(PAGES_FIXTURE, config).len(),
            2
        );
    }
}
