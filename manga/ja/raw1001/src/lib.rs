use liliana_manga::LilianaConfig;
#[cfg(target_arch = "wasm32")]
use liliana_manga::LilianaMangaSource;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct Raw1001;

impl LilianaConfig for Raw1001 {
    const BASE_URL: &'static str = "https://raw1001.net";
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().manga("raw1001", LilianaMangaSource::<Raw1001>::default())
);

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    const MANIFEST: &str = include_str!("../manifest.json");
    const ICON: &[u8] = include_bytes!("../assets/icon.png");
    const ICON_SHA256: &str = "f40b1f906500efba773616e75e107b35f1acca60d068e85fdc0afea5a94a4415";

    #[test]
    fn metadata_matches_live_hosts_and_adult_rating() {
        let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest parses");

        assert_eq!(manifest["id"], "raw1001");
        assert_eq!(manifest["contentType"], "manga");
        assert_eq!(manifest["license"], "Apache-2.0");
        assert_eq!(
            manifest["permissions"]["network"]["allow"],
            json!(["raw1001.net", "sg.cdnkk.top", "mgraw1111.wordpress.com"])
        );
        assert_eq!(
            manifest["sources"],
            json!([{
                "id": "raw1001",
                "name": "Raw1001",
                "lang": "ja",
                "contentType": "manga",
                "baseUrl": "https://raw1001.net",
                "contentRating": "adult",
                "capabilities": { "search": true, "latest": true, "filters": true, "urlResolution": true },
                "listings": [
                    { "id": "popular", "name": "Popular" },
                    { "id": "latest", "name": "Latest" }
                ],
                "urlPatterns": [{ "pattern": "https://raw1001.net/manga/*", "kind": "item-or-chapter" }],
                "tags": ["liliana"]
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
