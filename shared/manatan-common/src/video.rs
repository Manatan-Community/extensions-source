//! Media checks shared by sources that select between independent video mirrors.
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use manatan_sdk::{
    browser::{
        self, WebViewExtractRequest, WebViewExtractResponse, WebViewRequestCapture, WebViewSession,
        WebViewSessionPersistence, WebViewWaitUntil,
    },
    client::{Client, Response},
    Error, Result, VideoStream,
};
use regex::Regex;
use serde_json::Value;
use url::Url;

/// Open Byse's current frame with the same context its parent page supplies.
/// A generic extractor cannot operate the cross-origin frame from its parent.
pub fn byse_response(
    embed_url: &str,
    referer: &str,
    user_agent: &str,
) -> Result<WebViewExtractResponse> {
    let embed = Url::parse(embed_url).map_err(|e| Error::new(e.to_string()))?;
    if embed.scheme() != "https"
        || !matches!(embed.host_str(), Some("bysezoxexe.com" | "byselapuix.com"))
    {
        return Err(Error::new("unexpected Byse embed URL"));
    }
    let code = embed
        .path()
        .strip_prefix("/e/")
        .filter(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric()))
        .ok_or_else(|| Error::new("invalid Byse video code"))?;
    let details_url = embed
        .join(&format!("/api/videos/{code}/embed/details"))
        .map_err(|e| Error::new(e.to_string()))?;
    let details: Value = Client::browser()
        .get(details_url.to_string())
        .header("Referer", embed_url)
        .header("User-Agent", user_agent)
        .timeout_ms(15_000)
        .max_body_bytes(512 * 1024)
        .send()?
        .error_for_status()?
        .json()?;
    let frame = details
        .get("embed_frame_url")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new("Byse did not publish its player frame"))?;
    let frame_url = Url::parse(frame).map_err(|e| Error::new(e.to_string()))?;
    if frame_url.scheme() != "https"
        || frame_url.host_str() != Some("n1mwq.org")
        || !frame_url.path().ends_with(&format!("/{code}"))
    {
        return Err(Error::new("Byse returned an unexpected player frame"));
    }
    let context = serde_json::json!({
        "type": "moonwalk-embed-context",
        "host": Url::parse(referer).ok().and_then(|u| u.host_str().map(ToString::to_string)).unwrap_or_default(),
        "referrer": referer, "parentFrame": embed_url,
        "details": {
            "id": details["id"], "code": details["code"], "title": details["title"],
            "posterUrl": details["poster_url"], "description": details["description"],
            "createdAt": details["created_at"], "ownerPrivate": details["owner_private"],
        }
    });
    let wait = format!(
        r#"(() => {{
        const media = /\.(m3u8|mp4)(?:\?|$)/i;
        if (!document.querySelector('.captcha-gate, .jwplayer')) window.postMessage({context}, '*');
        document.querySelector('.captcha-gate__play')?.click();
        document.querySelector('.jw-icon-playback, .jw-display-icon-container')?.click();
        const video = document.querySelector('video');
        if (video?.paused) video.play().catch(() => {{}});
        return performance.getEntriesByType('resource').some(e => media.test(e.name));
    }})()"#
    );
    browser::extract(&WebViewExtractRequest {
        url: frame.to_string(), headers: vec![("Referer".into(), embed_url.into())],
        user_agent: Some(user_agent.into()), cookies: true, headless: Some(true),
        session: Some(WebViewSession {id: "byse-player".into(), persistence: WebViewSessionPersistence::Persistent, ..WebViewSession::default()}),
        wait_until: Some(WebViewWaitUntil::DomReady), wait_for_script: Some(wait), timeout_ms: Some(45_000),
        script: "(() => { const urls = performance.getEntriesByType('resource').map(e=>e.name).filter(u=>/\\.(m3u8|mp4)(?:\\?|$)/i.test(u)); document.querySelectorAll('video').forEach(video => { video.pause(); video.muted = true; }); return {urls}; })()".into(),
        capture_requests: [".m3u8", ".mp4"].into_iter().map(|value| WebViewRequestCapture {url_contains: Some(value.into()), limit: Some(8), ..WebViewRequestCapture::default()}).collect(),
        method: Default::default(),
        body: None,
        cookie_url: None,
        wait_for_selector: None,
        wait_for_event: None,
        preload_scripts: Vec::new(),
        capture_events: Vec::new(),
    })
}

/// Verify the manifest and its first media resource with the same headers that
/// playback uses. A parsed URL alone does not prove that a mirror is playable.
pub fn verify_stream(stream: &VideoStream) -> Result<()> {
    verify_stream_with(stream, |url, range| {
        let mut request = Client::browser()
            .get(url)
            .timeout_ms(12_000)
            .max_body_bytes(if range { 64 * 1024 } else { 2 * 1024 * 1024 });
        for (key, value) in &stream.headers {
            request = request.header(key, value);
        }
        if range {
            request = request.header("Range", "bytes=0-2047");
        }
        request.send()?.error_for_status()
    })
}

fn verify_stream_with(
    stream: &VideoStream,
    mut fetch: impl FnMut(&str, bool) -> Result<Response>,
) -> Result<()> {
    if !stream.is_hls {
        return check_media(fetch(&stream.url, true)?.bytes());
    }
    let mut url = stream.url.clone();
    for _ in 0..4 {
        let response = fetch(&url, false)?;
        let playlist = response.text()?;
        if !playlist
            .trim_start_matches('\u{feff}')
            .starts_with("#EXTM3U")
        {
            return Err(Error::new("mirror did not return an HLS playlist"));
        }
        let base = Url::parse(response.final_url()).map_err(|e| Error::new(e.to_string()))?;
        let first = playlist
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with('#'))
            .ok_or_else(|| Error::new("mirror returned an empty HLS playlist"))?;
        url = base
            .join(first)
            .map_err(|e| Error::new(e.to_string()))?
            .to_string();
        if playlist.contains("#EXT-X-STREAM-INF:") {
            continue;
        }
        // An unavailable host sometimes returns a short placeholder as valid HLS.
        let duration: f64 = playlist
            .lines()
            .filter_map(|line| line.strip_prefix("#EXTINF:"))
            .filter_map(|line| line.split(',').next()?.parse::<f64>().ok())
            .sum();
        if playlist.contains("#EXT-X-ENDLIST") && duration < 30.0 {
            return Err(Error::new("mirror returned a short placeholder playlist"));
        }
        for line in playlist
            .lines()
            .filter(|line| line.starts_with("#EXT-X-KEY:") || line.starts_with("#EXT-X-MAP:"))
        {
            if let Some(resource) = line
                .split("URI=\"")
                .nth(1)
                .and_then(|s| s.split('"').next())
            {
                let resource = base.join(resource).map_err(|e| Error::new(e.to_string()))?;
                let bytes = fetch(resource.as_str(), true)?;
                if line.starts_with("#EXT-X-KEY:") && bytes.bytes().len() != 16 {
                    return Err(Error::new("mirror returned an invalid HLS key"));
                }
            }
        }
        return check_media(fetch(&url, true)?.bytes());
    }
    Err(Error::new("mirror exceeded HLS playlist nesting limit"))
}

fn check_media(bytes: &[u8]) -> Result<()> {
    let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(256)]).to_ascii_lowercase();
    if bytes.len() < 64
        || prefix.trim_start().starts_with('<')
        || prefix.trim_start().starts_with('{')
    {
        return Err(Error::new("mirror returned an error page instead of media"));
    }
    Ok(())
}

pub fn voe_stream(embed_url: &str, referer: &str, user_agent: &str) -> Result<VideoStream> {
    let get = |url: &str, referer: &str| -> Result<String> {
        Ok(Client::browser()
            .get(url)
            .header("Referer", referer)
            .header("User-Agent", user_agent)
            .timeout_ms(15_000)
            .max_body_bytes(4 * 1024 * 1024)
            .send()?
            .error_for_status()?
            .text()?
            .to_string())
    };
    let page = get(embed_url, referer)?;
    let redirect = parse_voe_redirect_url(&page)?;
    let url = Url::parse(&redirect).map_err(|e| Error::new(e.to_string()))?;
    if url.scheme() != "https"
        || url.host_str() != Some("derricknationtop.com")
        || !url.path().starts_with("/e/")
    {
        return Err(Error::new("Voe returned an unexpected player redirect"));
    }
    let media = parse_voe_media_url(&get(&redirect, referer)?)?;
    let url = Url::parse(&media).map_err(|e| Error::new(e.to_string()))?;
    if url.scheme() != "https" || url.port_or_known_default() != Some(443) {
        return Err(Error::new("Voe returned an unexpected media URL"));
    }
    let is_hls = media.contains(".m3u8");
    Ok(VideoStream {
        url: media,
        format: Some(if is_hls { "hls" } else { "mp4" }.into()),
        is_hls,
        requires_proxy: true,
        initialized: true,
        headers: [
            ("Referer".into(), redirect),
            ("User-Agent".into(), user_agent.into()),
        ]
        .into_iter()
        .collect(),
        ..VideoStream::default()
    })
}

pub fn parse_voe_redirect_url(html: &str) -> Result<String> {
    Regex::new(r#"window\.location\.href\s*=\s*'([^']+)'"#)
        .unwrap()
        .captures(html)
        .and_then(|capture| capture.get(1))
        .map(|value| value.as_str().to_string())
        .ok_or_else(|| Error::new("Voe did not publish its player redirect"))
}

pub fn parse_voe_media_url(html: &str) -> Result<String> {
    let regex = Regex::new(
        r#"<script[^>]+type=[\"']application/json[\"'][^>]*>\s*\[\"([^\"]+)\"\]\s*</script>"#,
    )
    .unwrap();
    let encoded = regex
        .captures(html)
        .and_then(|c| c.get(1))
        .ok_or_else(|| Error::new("Voe did not publish its encoded playback data"))?;
    let mut normalized: String = encoded
        .as_str()
        .chars()
        .map(|c| match c {
            'A'..='Z' => (((c as u8 - b'A' + 13) % 26) + b'A') as char,
            'a'..='z' => (((c as u8 - b'a' + 13) % 26) + b'a') as char,
            _ => c,
        })
        .collect();
    for pattern in ["@$", "^^", "~@", "%?", "*~", "!!", "#&"] {
        normalized = normalized.replace(pattern, "_");
    }
    normalized.retain(|c| c != '_');
    let shifted = BASE64
        .decode(normalized)
        .map_err(|e| Error::new(e.to_string()))?;
    let reversed: Vec<u8> = shifted
        .into_iter()
        .map(|byte| byte.wrapping_sub(3))
        .rev()
        .collect();
    let json = BASE64
        .decode(reversed)
        .map_err(|e| Error::new(e.to_string()))?;
    let value: Value = serde_json::from_slice(&json).map_err(|e| Error::new(e.to_string()))?;
    value
        .get("source")
        .or_else(|| value.get("direct_access_url"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Error::new("Voe playback payload did not contain media"))
}

/// Decode the data dictionary used by packed players, without executing script.
pub fn unpack_player(html: &str) -> Result<String> {
    let regex = Regex::new(
        r#"(?s)\}\('((?:\\.|[^'\\])*)',\s*(\d+),\s*(\d+),\s*'((?:\\.|[^'\\])*)'\.split\('\|'\)"#,
    )
    .unwrap();
    let capture = regex
        .captures(html)
        .ok_or_else(|| Error::new("packed player data was not found"))?;
    let radix: u32 = capture[2]
        .parse()
        .map_err(|_| Error::new("invalid player radix"))?;
    if !(2..=36).contains(&radix) {
        return Err(Error::new("unsupported player radix"));
    }
    let dictionary: Vec<_> = capture[4].split('|').collect();
    let payload = capture[1].replace("\\'", "'").replace("\\\\", "\\");
    let word = Regex::new(r"\b[0-9a-z]+\b").unwrap();
    Ok(word
        .replace_all(&payload, |caps: &regex::Captures| {
            usize::from_str_radix(&caps[0], radix)
                .ok()
                .and_then(|index| dictionary.get(index))
                .filter(|value| !value.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| caps[0].to_string())
        })
        .into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use manatan_sdk::net;

    fn response(url: &str, body: &[u8]) -> Response {
        Response(net::Response {
            status: 200,
            headers: vec![],
            final_url: url.into(),
            body: body.into(),
        })
    }

    #[test]
    fn follows_extensionless_hls_and_rejects_failed_segment() {
        let stream = VideoStream {
            url: "https://media.example/master.txt".into(),
            is_hls: true,
            ..VideoStream::default()
        };
        let mut requests = Vec::new();
        let result = verify_stream_with(&stream, |url, range| {
            requests.push((url.to_string(), range));
            Ok(response(
                url,
                match requests.len() {
                    1 => b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nquality.txt?token=test\n",
                    2 => b"#EXTM3U\n#EXTINF:60,\nsegment.bin?token=test\n#EXT-X-ENDLIST\n",
                    _ => b"<html>Access denied</html>",
                },
            ))
        });
        assert!(result.is_err());
        assert_eq!(
            requests[2],
            ("https://media.example/segment.bin?token=test".into(), true)
        );
    }

    #[test]
    fn accepts_media_and_rejects_placeholder_hls() {
        let mut stream = VideoStream {
            url: "https://media.example/video.mp4".into(),
            ..VideoStream::default()
        };
        assert!(verify_stream_with(&stream, |url, _| Ok(response(url, &[0x47; 188]))).is_ok());
        stream.is_hls = true;
        assert!(verify_stream_with(&stream, |url, _| Ok(response(
            url,
            b"#EXTM3U\n#EXTINF:5,\na.ts\n#EXT-X-ENDLIST\n"
        )))
        .is_err());
    }

    #[test]
    fn unpacks_player_as_data_without_executing_script() {
        let packed = "eval(function(p,a,c,k,e,d){return p}('0({1:[{2:\"https://media.example/master.txt\"}]});',3,3,'setup|sources|file'.split('|')))";
        assert_eq!(
            unpack_player(packed).unwrap(),
            "setup({sources:[{file:\"https://media.example/master.txt\"}]});"
        );
    }
}
