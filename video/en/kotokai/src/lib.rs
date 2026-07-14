use anikoto_theme::AnikotoConfig;
#[cfg(target_arch = "wasm32")]
use anikoto_theme::AnikotoSource;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct KotoKai;

impl AnikotoConfig for KotoKai {
    const NAME: &'static str = "AnimeKai (Unoriginal)";
    const LANG: &'static str = "en";
    const BASE_URL: &'static str = "https://animekaitv.to";
    const DOMAINS: &'static [&'static str] =
        &["animekaitv.to", "anikaitv.to", "animekai.se", "anikai.se"];
    const HOSTERS: &'static [&'static str] = &[
        "HD-1",
        "Vidstream-2",
        "VidCloud-1",
        "Kiwi-Stream",
        "VidPlay-1",
    ];
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().video("kotokai", AnikotoSource::<KotoKai>::default())
);
