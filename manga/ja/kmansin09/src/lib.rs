use madara_manga::MadaraMangaConfig;
#[cfg(target_arch = "wasm32")]
use madara_manga::MadaraMangaSource;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct Kmansin09;

impl MadaraMangaConfig for Kmansin09 {
    const BASE_URL: &'static str = "https://kmansin09.top";
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().manga("kmansin09", MadaraMangaSource::<Kmansin09>::default())
);

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    const MANIFEST: &str = include_str!("../manifest.json");
    const ICON: &[u8] = include_bytes!("../assets/icon.png");
    const ICON_SHA256: &str = "b0cc2af071c7c0762edefe33ccd8dfb1553997fade1164186401dd6f7a194f0f";

    #[test]
    fn metadata_matches_expected_configuration() {
        let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest parses");

        assert_eq!(manifest["id"], "kmansin09");
        assert_eq!(manifest["contentType"], "manga");
        assert_eq!(manifest["license"], "Apache-2.0");
        assert_eq!(
            manifest["permissions"]["network"]["allow"],
            json!(["kmansin09.top", "post-phinf.pstatic.net"])
        );
        assert_eq!(
            manifest["sources"],
            json!([{
                "id": "kmansin09",
                "name": "Kmansin09",
                "lang": "ja",
                "contentType": "manga",
                "baseUrl": "https://kmansin09.top",
                "contentRating": "safe",
                "capabilities": { "search": true, "latest": true, "filters": true },
                "listings": [
                    { "id": "popular", "name": "Popular" },
                    { "id": "latest", "name": "Latest" }
                ],
                "urlPatterns": [{ "pattern": "https://kmansin09.top/manga/*", "kind": "manga" }],
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
