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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_current_aniyomi_domains_and_hosters() {
        assert!(Anikoto::DOMAINS.contains(&"anikototv.to"));
        assert!(Anikoto::DOMAINS.contains(&"anikototv.se"));
        assert!(Anikoto::HOSTERS.contains(&"HD-1"));
        assert!(Anikoto::HOSTERS.contains(&"Vidstream-2"));
    }

    #[test]
    fn manifest_allows_current_player_resources() {
        let manifest = include_str!("../manifest.json");
        for origin in [
            "https://*.kryntal.top",
            "https://*.norami.top",
            "https://*.sugevideo.xyz",
        ] {
            assert!(
                manifest.contains(&format!("\"{origin}\"")),
                "missing network permission for {origin}"
            );
        }
    }
}
