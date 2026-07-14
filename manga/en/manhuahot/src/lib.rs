use madara_manga::MadaraMangaConfig;
#[cfg(target_arch = "wasm32")]
use madara_manga::MadaraMangaSource;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct ManhuaHot;

impl MadaraMangaConfig for ManhuaHot {
    const BASE_URL: &'static str = "https://manhuahot.com";
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().manga("manhuahot", MadaraMangaSource::<ManhuaHot>::default())
);

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    const MANIFEST: &str = include_str!("../manifest.json");
    const ICON: &[u8] = include_bytes!("../assets/icon.png");
    const ICON_SHA256: &str = "c26907a7500d99917658c61ba0e10a954c80a9e6dcfbb9eb7947de3daa095fc4";

    #[test]
    fn metadata_matches_expected_configuration() {
        let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest parses");

        assert_eq!(manifest["id"], "manhuahot");
        assert_eq!(manifest["contentType"], "manga");
        assert_eq!(manifest["license"], "Apache-2.0");
        assert_eq!(
            manifest["permissions"]["network"]["allow"],
            json!(["manhuahot.com", "cdn.manhuahot.com"])
        );
        assert_eq!(
            manifest["sources"],
            json!([{
                "id": "manhuahot",
                "name": "ManhuaHot",
                "lang": "en",
                "contentType": "manga",
                "baseUrl": "https://manhuahot.com",
                "contentRating": "safe",
                "capabilities": { "search": true, "latest": true, "filters": true },
                "listings": [
                    { "id": "popular", "name": "Popular" },
                    { "id": "latest", "name": "Latest" }
                ],
                "urlPatterns": [{ "pattern": "https://manhuahot.com/manga/*", "kind": "manga" }],
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
