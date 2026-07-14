use madara_manga::MadaraMangaConfig;
#[cfg(target_arch = "wasm32")]
use madara_manga::MadaraMangaSource;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct Cat300;

impl MadaraMangaConfig for Cat300 {
    const BASE_URL: &'static str = "https://cat-300.com";
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().manga("cat300", MadaraMangaSource::<Cat300>::default())
);

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    const MANIFEST: &str = include_str!("../manifest.json");
    const ICON: &[u8] = include_bytes!("../assets/icon.png");
    const ICON_SHA256: &str = "54599cd7486d4c979888c34520b65c9894dcce1c6fe054a35cd47dee962167dc";

    #[test]
    fn metadata_matches_expected_configuration() {
        let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest parses");

        assert_eq!(manifest["id"], "cat300");
        assert_eq!(manifest["contentType"], "manga");
        assert_eq!(manifest["license"], "Apache-2.0");
        assert_eq!(
            manifest["permissions"]["network"]["allow"],
            json!(["cat-300.com"])
        );
        assert_eq!(
            manifest["sources"],
            json!([{
                "id": "cat300",
                "name": "Cat300",
                "lang": "th",
                "contentType": "manga",
                "baseUrl": "https://cat-300.com",
                "contentRating": "adult",
                "capabilities": { "search": true, "latest": true, "filters": true },
                "listings": [
                    { "id": "popular", "name": "Popular" },
                    { "id": "latest", "name": "Latest" }
                ],
                "urlPatterns": [{ "pattern": "https://cat-300.com/manga/*", "kind": "manga" }],
                "tags": ["madara"]
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
