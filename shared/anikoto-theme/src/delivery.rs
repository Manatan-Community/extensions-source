//! Native catalog selection, bounded catalog caching, and player metadata.
use std::collections::BTreeMap;

use manatan_sdk::{
    html, runtime, storage, CatalogItem, Error, ImageRequest, Paged, Result, SeekPreviewTile,
    SeekPreviewTrack,
};
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

fn timestamp(value: &str) -> Option<u32> {
    let mut result = 0u64;
    let pieces = value.trim().split(':').collect::<Vec<_>>();
    if !(2..=3).contains(&pieces.len()) {
        return None;
    }
    for (index, part) in pieces.iter().enumerate() {
        if index + 1 == pieces.len() {
            let (seconds, fraction) = part.split_once('.').unwrap_or((part, ""));
            let seconds = seconds.parse::<u64>().ok()?;
            if seconds >= 60 || fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            let millis = if fraction.is_empty() {
                0
            } else {
                fraction.parse::<u64>().ok()? * 10u64.pow(3 - fraction.len() as u32)
            };
            result = result
                .checked_mul(60_000)?
                .checked_add(seconds * 1000 + millis)?;
        } else {
            let number = part.parse::<u64>().ok()?;
            if index > 0 && number >= 60 {
                return None;
            }
            result = result.checked_mul(60)?.checked_add(number)?;
        }
    }
    u32::try_from(result).ok()
}

pub(crate) fn preview_track(
    body: &str,
    base: &str,
    headers: &BTreeMap<String, String>,
) -> Result<SeekPreviewTrack> {
    if !body
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("WEBVTT")
    {
        return Err(Error::new("thumbnail track is not WebVTT"));
    }
    let mut track = SeekPreviewTrack {
        sprites: Vec::new(),
        tiles: Vec::new(),
    };
    let mut position = None;
    for line in body.lines().map(str::trim) {
        if let Some((start, end)) = line.split_once("-->") {
            let end = end.split_whitespace().next().and_then(timestamp);
            position = timestamp(start).filter(|start| end.is_some_and(|end| end > *start));
            continue;
        }
        let Some(time) = position.take() else {
            continue;
        };
        let Some((reference, rectangle)) = line.split_once("#xywh=") else {
            continue;
        };
        let values = rectangle
            .split(',')
            .map(str::parse::<u32>)
            .collect::<std::result::Result<Vec<_>, _>>();
        let Ok(values) = values else { continue };
        if values.len() != 4 || values[2] == 0 || values[3] == 0 {
            continue;
        }
        let url = super::absolute_url(base, reference)?;
        let index = match track.sprites.iter().position(|sprite| sprite.url == url) {
            Some(index) => index,
            None => {
                if track.sprites.len() >= 128 {
                    return Err(Error::new("thumbnail track exceeds sprite limit"));
                }
                let mut sprite = ImageRequest::get(url);
                sprite.headers = headers.clone();
                track.sprites.push(sprite);
                track.sprites.len() - 1
            }
        };
        track.tiles.push(SeekPreviewTile {
            position_ms: time,
            sprite_index: index,
            x: values[0],
            y: values[1],
            width: values[2],
            height: values[3],
        });
        if track.tiles.len() >= 16_384 {
            return Err(Error::new("thumbnail track exceeds cue limit"));
        }
    }
    track.tiles.sort_by_key(|tile| tile.position_ms);
    Ok(track)
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

    #[test]
    fn parses_thumbnail_cues_with_ids_relative_urls_and_multiple_sprites() {
        let body = "WEBVTT\n\ncue-1\n00:00.000 --> 00:05.000\na.jpg#xywh=0,0,160,90\n\n00:05.000 --> 00:10.000\na.jpg#xywh=160,0,160,90\n\n00:10.000 --> 00:15.000\nb.jpg#xywh=0,0,160,90\n";
        let headers = BTreeMap::from([("Referer".into(), "https://player.example/".into())]);
        let track =
            preview_track(body, "https://cdn.example/preview/thumbs.vtt", &headers).unwrap();
        assert_eq!(track.sprites.len(), 2);
        assert_eq!(track.sprites[0].url, "https://cdn.example/preview/a.jpg");
        assert_eq!(track.sprites[0].headers, headers);
        assert_eq!(
            track
                .tiles
                .iter()
                .map(|tile| tile.position_ms)
                .collect::<Vec<_>>(),
            [0, 5000, 10000]
        );
        assert_eq!(track.tiles[1].x, 160);
        assert_eq!(track.tiles[2].sprite_index, 1);
        assert_eq!(timestamp("01:02:03.45"), Some(3_723_450));
    }

    #[test]
    fn rejects_invalid_thumbnail_cues_and_non_vtt_responses() {
        assert!(preview_track(
            "<html>denied</html>",
            "https://cdn.example",
            &BTreeMap::new()
        )
        .is_err());
        let body = "WEBVTT\n\n00:61.000 --> 01:05.000\na.jpg#xywh=0,0,1,1\n\n00:00.000 --> 00:01.000\na.jpg#xywh=0,0,0,90\n";
        assert!(preview_track(body, "https://cdn.example", &BTreeMap::new())
            .unwrap()
            .tiles
            .is_empty());
    }
}
