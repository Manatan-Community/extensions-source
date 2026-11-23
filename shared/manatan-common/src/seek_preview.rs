use manatan_sdk::{Error, ImageRequest, Result, SeekPreviewTile, SeekPreviewTrack};
use std::collections::BTreeMap;

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

pub fn preview_track(
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
