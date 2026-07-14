use madara_manga::MadaraMangaConfig;
#[cfg(target_arch = "wasm32")]
use madara_manga::MadaraMangaSource;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct PawManga;

impl MadaraMangaConfig for PawManga {
    const BASE_URL: &'static str = "https://pawmanga.com";
    const USE_NEW_CHAPTER_ENDPOINT: bool = true;
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().manga("pawmanga", MadaraMangaSource::<PawManga>::default())
);

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    const MANIFEST: &str = include_str!("../manifest.json");
    const ICON: &[u8] = include_bytes!("../assets/icon.png");
    const ICON_SHA256: &str = "4c859be0f73827e700d29433524225c792b8534a74d9dc4db25f69b6517cf8c8";

    #[test]
    fn metadata_matches_expected_configuration() {
        let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest parses");

        assert_eq!(manifest["id"], "pawmanga");
        assert_eq!(manifest["contentType"], "manga");
        assert_eq!(manifest["license"], "Apache-2.0");
        assert_eq!(
            manifest["permissions"]["network"]["allow"],
            json!(["pawmanga.com", "image.pawmanga.com"])
        );
        assert_eq!(
            manifest["sources"],
            json!([{
                "id": "pawmanga",
                "name": "Paw Manga",
                "lang": "en",
                "contentType": "manga",
                "baseUrl": "https://pawmanga.com",
                "contentRating": "adult",
                "capabilities": { "search": true, "latest": true, "filters": true },
                "listings": [
                    { "id": "popular", "name": "Popular" },
                    { "id": "latest", "name": "Latest" }
                ],
                "urlPatterns": [{ "pattern": "https://pawmanga.com/manga/*", "kind": "manga" }],
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
