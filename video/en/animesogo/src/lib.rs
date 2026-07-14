use anikoto_theme::AnikotoConfig;
#[cfg(target_arch = "wasm32")]
use anikoto_theme::AnikotoSource;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct AnimeSogo;

impl AnikotoConfig for AnimeSogo {
    const NAME: &'static str = "AnimeSogo";
    const LANG: &'static str = "en";
    const BASE_URL: &'static str = "https://animesogo.to";
    const DOMAINS: &'static [&'static str] = &["animesogo.to"];
    const HOSTERS: &'static [&'static str] = &["HD-1", "HD-2", "HD-3", "VidPlay-1", "Kiwi-Stream"];

    fn listing_thumbnail_selector() -> &'static str {
        "a.poster img"
    }

    fn detail_thumbnail_selector() -> &'static str {
        "section#w-info div.poster img"
    }

    fn synopsis_content_selector() -> &'static str {
        "div.synopsis > div.content"
    }

    fn score_label() -> &'static str {
        "Scores"
    }

    fn episode_list_selector() -> &'static str {
        "ul.episodes > li > a"
    }

    fn server_group_selector() -> &'static str {
        "div.type"
    }

    fn server_item_selector() -> &'static str {
        "a.server"
    }

    fn server_name_selector() -> Option<&'static str> {
        Some("span")
    }

    fn canonical_server_name(raw: &str) -> String {
        if raw.to_ascii_lowercase().starts_with("server") {
            let suffix = raw.get("Server".len()..).unwrap_or_default().trim();
            return if suffix.is_empty() {
                "Kiwi-Stream".to_owned()
            } else {
                format!("Kiwi-Stream {suffix}")
            };
        }
        raw.trim_end_matches(['-', ' ']).to_owned()
    }

    fn server_matches(configured: &str, actual: &str) -> bool {
        if configured.eq_ignore_ascii_case(actual) {
            return true;
        }
        let configured = configured.to_ascii_lowercase();
        let actual = actual.to_ascii_lowercase();
        actual.strip_prefix(&configured).is_some_and(|suffix| {
            let suffix = suffix.trim();
            !suffix.is_empty() && suffix.chars().all(|value| value.is_ascii_digit())
        })
    }

    fn map_filter_value(key: &str, value: &str) -> String {
        if key == "rating" {
            value.to_ascii_lowercase().replace('-', "_")
        } else {
            value.to_owned()
        }
    }

    fn should_generate_search_vrf(query: &str) -> bool {
        !query.trim().is_empty()
    }
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().video("animesogo", AnikotoSource::<AnimeSogo>::default())
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_upstream_layout_and_server_behavior() {
        assert_eq!(AnimeSogo::listing_thumbnail_selector(), "a.poster img");
        assert_eq!(
            AnimeSogo::canonical_server_name("Server 2"),
            "Kiwi-Stream 2"
        );
        assert!(AnimeSogo::server_matches("Kiwi-Stream", "Kiwi-Stream 2"));
        assert_eq!(AnimeSogo::map_filter_value("rating", "PG-13"), "pg_13");
    }
}
