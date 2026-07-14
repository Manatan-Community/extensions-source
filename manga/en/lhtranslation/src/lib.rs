use madara_manga::MadaraMangaConfig;
#[cfg(target_arch = "wasm32")]
use madara_manga::MadaraMangaSource;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct LHTranslation;

impl MadaraMangaConfig for LHTranslation {
    const BASE_URL: &'static str = "https://lhtranslation.net";
    const USE_NEW_CHAPTER_ENDPOINT: bool = true;
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(manatan_sdk::Extension::new().manga(
    "lhtranslation",
    MadaraMangaSource::<LHTranslation>::default()
));
