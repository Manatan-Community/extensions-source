//! Native catalog selection, bounded catalog caching, and player metadata.
use std::collections::BTreeMap;

use manatan_sdk::{html, runtime, storage, CatalogItem, Error, Paged, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

const CACHE_TTL_MS: i64 = 300_000;
const CACHE_LIMIT: usize = 16;
const CACHE_NAMESPACE: &str = "anikoto.catalog.v1";

#[derive(Serialize, Deserialize)]
struct Cached<T> {
    written: i64,
    value: T,
}

fn cache_key(url: &str) -> String {
    format!("{:x}", Sha256::digest(url.as_bytes()))
}

fn fresh(written: i64, now: i64) -> bool {
    now.checked_sub(written)
        .is_some_and(|age| (0..CACHE_TTL_MS).contains(&age))
}

pub(crate) fn cached<T: DeserializeOwned>(url: &str) -> Option<T> {
    let entry = storage::get::<Cached<T>>(CACHE_NAMESPACE, &cache_key(url)).ok()??;
    fresh(entry.written, runtime::now_millis()).then_some(entry.value)
}

pub(crate) fn remember<T: Serialize>(url: &str, value: T) {
    let now = runtime::now_millis();
    let key = cache_key(url);
    let mut index = storage::get::<BTreeMap<String, i64>>(CACHE_NAMESPACE, "index")
        .ok()
        .flatten()
        .unwrap_or_default();
    let expired = index
        .iter()
        .filter(|(_, time)| !fresh(**time, now))
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in expired {
        index.remove(&key);
        let _ = storage::delete(CACHE_NAMESPACE, &key);
    }
    index.insert(key.clone(), now);
    while index.len() > CACHE_LIMIT {
        let oldest = index
            .iter()
            .min_by_key(|(_, time)| **time)
            .map(|(key, _)| key.clone())
            .unwrap();
        index.remove(&oldest);
        let _ = storage::delete(CACHE_NAMESPACE, &oldest);
    }
    let _ = storage::set(
        CACHE_NAMESPACE,
        &key,
        &Cached {
            written: now,
            value,
        },
    );
    let _ = storage::set(CACHE_NAMESPACE, "index", &index);
}

/// Older hosts retain the portable parser. Supported hosts parse the complete
/// DOM once in native code and return only bounded listing fields to the guest.
pub(crate) fn native_listing<C: super::AnikotoConfig>(
    body: &str,
    base: &str,
) -> Option<Result<Paged<CatalogItem>>> {
    if !cfg!(target_arch = "wasm32") || !html::native_selection_available() {
        return None;
    }
    use html::{SelectField as Field, SelectQuery as Query, SelectRequest, SelectValue as Value};
    let request = SelectRequest {
        queries: vec![
            Query::new("items", "div.ani.items > div.item")
                .limit(100)
                .field(
                    Field::new(
                        "href",
                        Value::Attribute {
                            name: "href".into(),
                        },
                    )
                    .within("a.name"),
                )
                .field(Field::new("title", Value::Text).within("a.name"))
                .field(
                    Field::new(
                        "lazy",
                        Value::Attribute {
                            name: "data-src".into(),
                        },
                    )
                    .within(C::listing_thumbnail_selector()),
                )
                .field(
                    Field::new("src", Value::Attribute { name: "src".into() })
                        .within(C::listing_thumbnail_selector()),
                ),
            Query::new("next", "nav > ul.pagination > li.active ~ li").limit(1),
        ],
        ..SelectRequest::default()
    };
    Some(
        html::select(&request, body.as_bytes()).and_then(|selected| {
            listing_fields(
                selected
                    .results
                    .get("items")
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                selected
                    .results
                    .get("next")
                    .is_some_and(|rows| !rows.is_empty()),
                base,
            )
        }),
    )
}

fn listing_fields(
    rows: &[BTreeMap<String, Option<String>>],
    next: bool,
    base: &str,
) -> Result<Paged<CatalogItem>> {
    let mut items = Vec::new();
    for row in rows {
        let get = |key: &str| {
            row.get(key)
                .and_then(Option::as_deref)
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let (Some(href), Some(title)) = (get("href"), get("title")) else {
            continue;
        };
        let href = super::absolute_url(base, &super::strip_episode_suffix(href))?;
        let mut item = CatalogItem::new(
            Url::parse(&href).map_err(super::url_error)?.path(),
            super::normalize_space(title),
        );
        item.url = Some(href);
        item.cover = get("lazy")
            .or_else(|| get("src"))
            .and_then(|url| super::absolute_url(base, url).ok())
            .map(Into::into);
        item.content_rating = Some("suggestive".into());
        items.push(item);
    }
    Ok(Paged::new(items, next))
}

/// Parse HLS rendition declarations without downloading any video segments.
pub(crate) fn renditions(body: &str, base: &str) -> Result<Vec<(String, String)>> {
    if !body.trim_start().starts_with("#EXTM3U") {
        return Err(Error::new("player returned a non-HLS playlist"));
    }
    let mut pending = None;
    let mut result = Vec::new();
    for line in body.lines().take(16_384).map(str::trim) {
        if let Some(attributes) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending = attributes.split(',').find_map(|part| {
                let resolution = part.trim().strip_prefix("RESOLUTION=")?;
                let (_, height) = resolution.split_once('x')?;
                let height = height.parse::<u32>().ok()?;
                (height > 0).then(|| format!("{height}p"))
            });
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some(quality) = pending.take() {
                result.push((super::absolute_url(base, line)?, quality));
                if result.len() >= 32 {
                    break;
                }
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_expires_and_rejects_clock_reversal() {
        assert!(fresh(1000, 1000));
        assert!(fresh(1000, 300_999));
        assert!(!fresh(1000, 301_000));
        assert!(!fresh(1000, 999));
        assert_ne!(
            cache_key("https://one/filter?q=a"),
            cache_key("https://one/filter?q=b")
        );
    }

    #[test]
    fn native_listing_preserves_fields_and_skips_invalid_cards() {
        let row = BTreeMap::from([
            ("title".into(), Some(" Example  Anime ".into())),
            ("href".into(), Some("/watch/example/ep-1".into())),
            ("lazy".into(), Some("/images/cover.jpg".into())),
        ]);
        let result =
            listing_fields(&[BTreeMap::new(), row], true, "https://example.org/filter").unwrap();
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].title, "Example Anime");
        assert_eq!(
            result.entries[0].url.as_deref(),
            Some("https://example.org/watch/example")
        );
        assert!(result.has_next_page);
    }

    #[test]
    fn resolves_hls_renditions_against_final_playlist_url() {
        let body = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION=1920x1080\n1080/index.m3u8?token=one\n#EXT-X-STREAM-INF:RESOLUTION=1280x720\n//cdn.example/720.m3u8\n";
        let rows = renditions(body, "https://media.example/master.m3u8").unwrap();
        assert_eq!(
            rows,
            [
                (
                    "https://media.example/1080/index.m3u8?token=one".into(),
                    "1080p".into()
                ),
                ("https://cdn.example/720.m3u8".into(), "720p".into())
            ]
        );
        assert!(renditions(
            "#EXTM3U\n#EXTINF:2\nsegment.ts",
            "https://media.example/index.m3u8"
        )
        .unwrap()
        .is_empty());
        assert!(renditions("<html>error</html>", "https://media.example").is_err());
    }
}
