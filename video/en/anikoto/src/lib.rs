use anikoto_theme::AnikotoConfig;
#[cfg(target_arch = "wasm32")]
use anikoto_theme::AnikotoSource;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct Anikoto;

impl AnikotoConfig for Anikoto {
    const NAME: &'static str = "Anikoto";
    const LANG: &'static str = "en";
    const BASE_URL: &'static str = "https://anikototv.to";
    const DOMAINS: &'static [&'static str] = &[
        "anikototv.to",
        "anikoto.bz",
        "anikoto.cz",
        "anikoto.me",
        "anikoto.net",
        "anikototv.se",
    ];
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
    manatan_sdk::Extension::new().video("anikoto", AnikotoSource::<Anikoto>::default())
);
