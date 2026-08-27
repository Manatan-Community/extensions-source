use anikoto_theme::AnikotoConfig;
#[cfg(target_arch = "wasm32")]
use anikoto_theme::AnikotoSource;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct AniWave;

impl AnikotoConfig for AniWave {
    const NAME: &'static str = "AniWave (Unoriginal)";
    const LANG: &'static str = "en";
    const BASE_URL: &'static str = "https://animewave.to";
    const DOMAINS: &'static [&'static str] = &[
        "animewave.to",
        "aniwave.id",
        "aniwave.best",
        "aniwave.ro",
        "aniwave.cz",
    ];
    const HOSTERS: &'static [&'static str] = &[
        "HD-1",
        "Vidstream-2",
        "VidCloud-1",
        "Kiwi-Stream",
        "VidPlay-1",
    ];
    const MAPPER_URL: &'static str = "https://mapper.mewcdn.online/api";
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().video("aniwave", AnikotoSource::<AniWave>::default())
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_current_aniyomi_domains_and_mapper() {
        assert!(AniWave::DOMAINS.contains(&"aniwave.id"));
        assert!(AniWave::DOMAINS.contains(&"aniwave.best"));
        assert!(AniWave::DOMAINS.contains(&"aniwave.ro"));
        assert_eq!(AniWave::MAPPER_URL, "https://mapper.mewcdn.online/api");
    }

    #[test]
    fn manifest_allows_current_player_resources() {
        let manifest = include_str!("../manifest.json");
        for origin in [
            "https://*.kotocdn.site",
            "https://*.lostproject.club",
            "https://mapper.mewcdn.online",
            "https://*.kryntal.top",
            "https://*.norami.top",
            "https://*.sugevideo.xyz",
            "https://*.livedns.my",
        ] {
            assert!(
                manifest.contains(&format!("\"{origin}\"")),
                "missing network permission for {origin}"
            );
        }
    }
}
