#[cfg(target_arch = "wasm32")]
use manatan_sdk::Extension;
use natsuid_manga::{NatsuIdMangaConfig, NatsuIdMangaSource};

#[derive(Default)]
pub struct RawkumaConfig;

impl NatsuIdMangaConfig for RawkumaConfig {
    const NAME: &'static str = "Rawkuma";
    const BASE_URL: &'static str = "https://rawkuma.net";
    const LANG: &'static str = "ja";
    const CONTENT_RATING: Option<&'static str> = Some("adult");
}

pub type RawkumaSource = NatsuIdMangaSource<RawkumaConfig>;

#[cfg(target_arch = "wasm32")]
fn extension() -> Extension {
    Extension::new().manga("rawkuma", RawkumaSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    const MANIFEST: &str = include_str!("../manifest.json");
    const ICON: &[u8] = include_bytes!("../assets/icon.png");
    const ICON_SHA256: &str = "241f75509486150d6df210dde1b7bb58ebd90cdd30da05aadef7f1953645744a";

    #[test]
    fn preserves_upstream_source_metadata() {
        assert_eq!(RawkumaConfig::NAME, "Rawkuma");
        assert_eq!(RawkumaConfig::BASE_URL, "https://rawkuma.net");
        assert_eq!(RawkumaConfig::LANG, "ja");
    }

    #[test]
    fn metadata_matches_expected_configuration() {
        let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest parses");

        assert_eq!(manifest["id"], "rawkuma");
        assert_eq!(manifest["contentType"], "manga");
        assert_eq!(manifest["license"], "Apache-2.0");
        assert_eq!(
            manifest["permissions"]["network"]["allow"],
            json!([
                "https://rawkuma.net",
                "https://cdn.kumacdn.club",
                "https://rcdn.kyut.dev"
            ])
        );
        assert_eq!(manifest["permissions"]["cookies"], true);
        assert_eq!(manifest["permissions"]["webview"], true);
        assert_eq!(
            manifest["sources"],
            json!([{
                "id": "rawkuma",
                "name": "Rawkuma",
                "lang": "ja",
                "contentType": "manga",
                "baseUrl": "https://rawkuma.net",
                "contentRating": "adult",
                "capabilities": {
                    "search": true,
                    "latest": true,
                    "filters": true,
                    "urlResolution": true
                },
                "listings": [
                    { "id": "popular", "name": "Popular" },
                    { "id": "latest", "name": "Latest" }
                ],
                "urlPatterns": [
                    { "pattern": "https://rawkuma.net/manga/*", "kind": "item-or-chapter" }
                ],
                "tags": ["natsuid"]
            }])
        );
    }

    #[test]
    fn icon_digest_matches_manifest() {
        let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest parses");

        assert_eq!(manifest["assets"][0]["sha256"], ICON_SHA256);
        assert_eq!(format!("{:x}", Sha256::digest(ICON)), ICON_SHA256);
    }
}
