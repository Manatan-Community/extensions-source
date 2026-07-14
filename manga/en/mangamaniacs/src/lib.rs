use madara_manga::MadaraMangaConfig;
#[cfg(target_arch = "wasm32")]
use madara_manga::MadaraMangaSource;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct MangaManiacs;

impl MadaraMangaConfig for MangaManiacs {
    const BASE_URL: &'static str = "https://mangamaniacs.org";
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(manatan_sdk::Extension::new()
    .manga("mangamaniacs", MadaraMangaSource::<MangaManiacs>::default()));

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    const MANIFEST: &str = include_str!("../manifest.json");
    const ICON: &[u8] = include_bytes!("../assets/icon.png");
    const ICON_SHA256: &str = "512b9b45cc3bff98a718b7d793d4669a0cc5a7cac3ce47c7030230c2d7736403";

    #[test]
    fn metadata_matches_expected_configuration() {
        let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest parses");

        assert_eq!(manifest["id"], "mangamaniacs");
        assert_eq!(manifest["contentType"], "manga");
        assert_eq!(manifest["license"], "Apache-2.0");
        assert_eq!(
            manifest["permissions"]["network"]["allow"],
            json!(["mangamaniacs.org", "images.mangamaniacs.org"])
        );
        assert_eq!(
            manifest["sources"],
            json!([{
                "id": "mangamaniacs",
                "name": "MangaManiacs",
                "lang": "en",
                "contentType": "manga",
                "baseUrl": "https://mangamaniacs.org",
                "contentRating": "adult",
                "capabilities": { "search": true, "latest": true, "filters": true },
                "listings": [
                    { "id": "popular", "name": "Popular" },
                    { "id": "latest", "name": "Latest" }
                ],
                "urlPatterns": [{ "pattern": "https://mangamaniacs.org/manga/*", "kind": "manga" }],
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
