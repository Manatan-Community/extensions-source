use manatan_extension::export_manga_source;
use manatan_shared::{impl_gallery_adults_source, manga};
use serde_json::Value;

const SOURCE: AsmHentai = AsmHentai;

const CONFIGS: [manga::GalleryAdultsConfig; 4] = [
    manga::GalleryAdultsConfig {
        base_url: "https://asmhentai.com",
        source_id: "asmhentai-en",
        lang: "en",
        manga_lang: "english",
        content_rating: "adult",
        id_prefix_uri: "g",
        page_uri: "gallery",
        list_selector_marker: "preview_item",
        image_container_marker: "preview_thumb",
    },
    manga::GalleryAdultsConfig {
        base_url: "https://asmhentai.com",
        source_id: "asmhentai-ja",
        lang: "ja",
        manga_lang: "japanese",
        content_rating: "adult",
        id_prefix_uri: "g",
        page_uri: "gallery",
        list_selector_marker: "preview_item",
        image_container_marker: "preview_thumb",
    },
    manga::GalleryAdultsConfig {
        base_url: "https://asmhentai.com",
        source_id: "asmhentai-zh",
        lang: "zh",
        manga_lang: "chinese",
        content_rating: "adult",
        id_prefix_uri: "g",
        page_uri: "gallery",
        list_selector_marker: "preview_item",
        image_container_marker: "preview_thumb",
    },
    manga::GalleryAdultsConfig {
        base_url: "https://asmhentai.com",
        source_id: "asmhentai-all",
        lang: "all",
        manga_lang: "",
        content_rating: "adult",
        id_prefix_uri: "g",
        page_uri: "gallery",
        list_selector_marker: "preview_item",
        image_container_marker: "preview_thumb",
    },
];

struct AsmHentai;

impl manga::GalleryAdultsSource for AsmHentai {
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

impl_gallery_adults_source!(AsmHentai);

fn config_for(request: &Value) -> &'static manga::GalleryAdultsConfig {
    let source_id = manga::GalleryAdults::source_id(request, "asmhentai-all");
    CONFIGS
        .iter()
        .find(|config| config.source_id == source_id)
        .unwrap_or(&CONFIGS[3])
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<html><body>
  <div class="preview_item">
    <div class="image"><a href="https://asmhentai.com/g/123/"><img data-src="https://img.example/123t.jpg" alt="Fixture Gallery"></a></div>
    <div class="caption">Fixture Gallery</div>
    <a class="flag" href="https://asmhentai.com/language/english/">English</a>
  </div>
  <ul class="pagination"><li class="active"></li><li></li></ul>
</body></html>
"#;

const DETAILS_FIXTURE: &str = r#"
<html><body>
  <div class="book_page">
    <h1>Fixture Gallery</h1>
    <div class="cover"><img src="https://img.example/cover.jpg"></div>
    <div class="tags">Tags:<span class="tag_list"><a><span class="tag">Action</span></a><a><span class="tag">Drama</span></a></span></div>
    <div class="tags">Artists:<span class="tag_list"><a><span class="tag">Artist One</span></a></span></div>
    <div class="tags">Groups:<span class="tag_list"><a><span class="tag">Group One</span></a></span></div>
  </div>
</body></html>
"#;

const PAGES_FIXTURE: &str = r#"
<html><body>
  <input id="load_id" value="123">
  <input id="load_dir" value="gallery/123">
  <div class="preview_thumb"><a><img src="https://img.example/1t.jpg"></a></div>
  <div class="preview_thumb"><a><img src="https://img.example/2t.jpg"></a></div>
</body></html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_source_id() {
        let config = config_for(&serde_json::json!({"sourceId": "asmhentai-en"}));
        assert_eq!(config.lang, "en");
    }

    #[test]
    fn parses_listing() {
        let entries = manga::GalleryAdults::parse_listing(LIST_FIXTURE, &CONFIGS[0]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "/g/123");
    }

    #[test]
    fn parses_details_and_pages() {
        let item = manga::GalleryAdults::parse_details(DETAILS_FIXTURE, Some("/g/123".into()), &CONFIGS[0]);
        assert_eq!(item.title, "Fixture Gallery");
        let pages = manga::GalleryAdults::parse_pages(PAGES_FIXTURE, &CONFIGS[0]);
        assert_eq!(pages.len(), 2);
    }
}
