use manatan_sdk::{client::Client, Error, PlayerArg, Result, VideoStream};
use serde_json::Value;
use url::Url;

use super::{media_headers, validate_player_url};

pub(super) fn resolve(player_url: &str, language: &str, name: &str) -> Result<VideoStream> {
    let metadata_url = metadata_url(player_url)?;
    let response = Client::browser()
        .get(metadata_url.as_str())
        .header("Accept", "application/json")
        .header("Referer", player_url)
        .timeout_ms(4_000)
        .max_body_bytes(64 * 1024)
        .send()?
        .error_for_status()?;
    let payload = response.json()?;
    let media = parse_metadata(&payload)?;
    let mut progress = media.url.clone();
    progress
        .path_segments_mut()
        .map_err(|_| Error::new("Invalid ReAnime download URL"))?
        .push("progress");
    let ready = Client::browser()
        .get(progress.as_str())
        .header("Accept", "text/event-stream")
        .header("Referer", "https://flixcloud.cc/")
        .timeout_ms(4_000)
        .max_body_bytes(16 * 1024)
        .send()
        .map_err(|_| Error::new("ReAnime's download server did not finish preparing the video"))?;
    if ready.status() != 200 || !download_ready(ready.text()?) {
        return Err(Error::new(
            "ReAnime's download server has not prepared the video",
        ));
    }

    let quality = media.quality.map(str::to_owned);
    let display_name = quality
        .as_ref()
        .map_or_else(|| name.to_string(), |q| format!("{name} · {q}"));
    Ok(VideoStream {
        url: media.url.into(),
        name: Some(display_name),
        quality,
        format: Some(media.format.into()),
        size_bytes: media.size,
        preferred: language == "sub",
        initialized: true,
        headers: media_headers(),
        mpv_args: vec![PlayerArg {
            name: "alang".into(),
            value: if language == "dub" {
                "eng"
            } else {
                "jpn,zho,kor"
            }
            .into(),
        }],
        extra: [("previewVtt".into(), Value::String(media.preview.into()))].into(),
        ..VideoStream::default()
    })
}

fn metadata_url(player_url: &str) -> Result<Url> {
    validate_player_url(player_url)?;
    let mut url = Url::parse(player_url).map_err(super::url_error)?;
    let id = url
        .path()
        .trim_end_matches('/')
        .strip_prefix("/e/")
        .unwrap_or_default();
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(Error::new("Invalid ReAnime player ID"));
    }
    let path = format!("/d/{id}/__data.json");
    url.set_path(&path);
    url.set_fragment(None);
    Ok(url)
}

struct Media<'a> {
    url: Url,
    preview: Url,
    format: &'static str,
    quality: Option<&'a str>,
    size: Option<u64>,
}

// SvelteKit's data response stores objects in a value table. The integers
// in the object fields are indices into that table, not the actual values.
fn parse_metadata(payload: &Value) -> Result<Media<'_>> {
    let invalid = || Error::new("ReAnime returned invalid download metadata");
    let table = payload["nodes"]
        .as_array()
        .and_then(|nodes| nodes.iter().rev().find_map(|node| node["data"].as_array()))
        .ok_or_else(invalid)?;
    let field = |entry: &Value| {
        entry
            .as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .and_then(|i| table.get(i))
    };
    let root = table.first().ok_or_else(invalid)?;
    let video = field(&root["video"]).ok_or_else(invalid)?;
    let download = field(&root["download"]).ok_or_else(invalid)?;
    let file = field(&video["fileId"])
        .and_then(Value::as_str)
        .filter(|id| {
            id.len() == 36
                && id.bytes().enumerate().all(|(i, b)| {
                    if [8, 13, 18, 23].contains(&i) {
                        b == b'-'
                    } else {
                        b.is_ascii_hexdigit()
                    }
                })
        })
        .ok_or_else(invalid)?;
    let token = field(&download["token"])
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(invalid)?;
    let base = field(&download["base"])
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    let mut url = Url::parse(base).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || url.path() != "/"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.host_str().is_some_and(|host| {
            host == "flixcloud.cc"
                || host
                    .strip_prefix("fetch")
                    .and_then(|host| host.strip_suffix(".flixcloud.cc"))
                    .is_some_and(|number| number.bytes().all(|b| b.is_ascii_digit()))
        })
    {
        return Err(invalid());
    }
    let format = match field(&video["filename"])
        .and_then(Value::as_str)
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("mkv") => "mkv",
        Some("mp4") => "mp4",
        _ => {
            return Err(Error::new(
                "ReAnime download has an unsupported video format",
            ))
        }
    };
    let preview = url
        .join(&format!("thumbnails_vtt/{file}"))
        .map_err(|_| invalid())?;
    url.set_path(&format!("/download/{file}"));
    url.query_pairs_mut().append_pair("token", token);

    Ok(Media {
        url,
        preview,
        format,
        quality: field(&video["resolution"])
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty()),
        size: field(&video["sizeBytes"]).and_then(Value::as_u64),
    })
}

fn download_ready(body: &str) -> bool {
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
        .any(|event| event["status"] == "ready")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_the_provider_download_and_preview_from_the_svelte_value_table() {
        let payload = json!({"type":"data","nodes":[null,{"type":"data","data":[
            {"video":1,"download":2},
            {"fileId":3,"filename":4,"resolution":5,"sizeBytes":6},
            {"base":7,"token":8},
            "19eff0b7-3592-4b90-8f4f-34f2e1fa6e2a","Episode.mkv","1080p",256000,
            "https://fetch8.flixcloud.cc","example-token"
        ]}]});
        let result = parse_metadata(&payload).unwrap();
        assert_eq!(result.url.host_str(), Some("fetch8.flixcloud.cc"));
        assert_eq!(result.url.query(), Some("token=example-token"));
        assert!(result.preview.path().starts_with("/thumbnails_vtt/"));
        assert_eq!(result.quality, Some("1080p"));
        assert_eq!(result.size, Some(256000));
        assert_eq!(result.format, "mkv");
        let mut untrusted = payload;
        untrusted["nodes"][1]["data"][7] = json!("https://fetch8.flixcloud.cc.example.org");
        assert!(parse_metadata(&untrusted).is_err());
    }

    #[test]
    fn keeps_the_server_variant_and_uses_only_ready_events() {
        let url = metadata_url("https://flixcloud.cc/e/sample?v=2").unwrap();
        assert_eq!(
            url.as_str(),
            "https://flixcloud.cc/d/sample/__data.json?v=2"
        );
        assert!(!download_ready("data: {\"status\":\"building\"}\n\n"));
        assert!(download_ready("data: {\"status\":\"ready\"}\n\n"));
    }
}
