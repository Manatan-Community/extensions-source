use manatan_sdk::{
    browser::{
        self, WebViewExtractRequest, WebViewExtractResponse, WebViewRequestCapture, WebViewSession,
        WebViewSessionPersistence, WebViewWaitUntil,
    },
    client::Client,
    runtime, CatalogItem, Error, ImageRequest, Paged, Result, VideoEpisode, VideoSource,
    VideoStream,
};
use serde_json::Value;
use url::Url;

const BASE_URL: &str = "https://shuttletv.su";
const TMDB_URL: &str = "https://api.themoviedb.org/3";
const IMAGE_URL: &str = "https://image.tmdb.org/t/p";
// ShuttleTV publishes this browser key in its own client bundle.
const TMDB_KEY: &str = "ea021b3b0775c8531592713ab727f254";

#[derive(Default)]
pub struct ShuttleTv;

impl ShuttleTv {
    fn tmdb(&self, route: &str, parameters: &[(&str, String)]) -> Result<Value> {
        let mut url = Url::parse(&format!("{TMDB_URL}{route}")).map_err(url_error)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("api_key", TMDB_KEY);
            query.append_pair("language", "en-US");
            for (key, value) in parameters {
                query.append_pair(key, value);
            }
        }
        Client::browser()
            .get(url.as_str())
            .timeout_ms(30_000)
            .max_body_bytes(8 * 1024 * 1024)
            .send()?
            .error_for_status()?
            .json()
    }

    fn page(&self, route: &str, page: u32) -> Result<Paged<CatalogItem>> {
        let value = self.tmdb(route, &[("page", page.max(1).to_string())])?;
        parse_tmdb_page(&value)
    }

    fn capture_stream(&self, embed_url: &str) -> Result<Vec<VideoStream>> {
        validate_cinesrc_url(embed_url)?;
        let navigation_url = cache_busted_url(embed_url, runtime::now_millis())?;
        let response: WebViewExtractResponse = browser::extract(&WebViewExtractRequest {
            url: navigation_url,
            method: Default::default(),
            body: None,
            cookie_url: None,
            session: Some(WebViewSession {
                id: "cinesrc-player".to_string(),
                persistence: WebViewSessionPersistence::Persistent,
                ..WebViewSession::default()
            }),
            headers: vec![
                ("Referer".to_string(), format!("{BASE_URL}/")),
                ("Origin".to_string(), BASE_URL.to_string()),
            ],
            user_agent: None,
            wait_until: Some(WebViewWaitUntil::DomReady),
            wait_for_selector: None,
            wait_for_event: None,
            // CineSrc delays its player bootstrap while its browser challenge runs.
            // Do not use an elapsed-time fallback here: challenge completion varies
            // substantially, and returning early produces an empty stream list.
            wait_for_script: Some(
                "Boolean((window.__cinesrcVideo || document.querySelector('video'))?.currentSrc) || performance.getEntriesByType('resource').some(e => e.name.includes('.m3u8') || e.name.includes('.mpd') || e.name.includes('/stream/mpd'))"
                    .to_string(),
            ),
            script: "(() => { const v = window.__cinesrcVideo || document.querySelector('video'); const h = window.__cinesrcHls; const resources = performance.getEntriesByType('resource').map(e => e.name); const levels = Array.from(h?.levels || []).flatMap(l => Array.isArray(l.url) ? l.url : [l.url]); return { url: v?.currentSrc || v?.src || '', urls: Array.from(new Set([...resources, ...levels].filter(u => typeof u === 'string' && (u.includes('.m3u8') || u.includes('.mpd') || u.includes('/stream/mpd'))))), textTracks: Array.from(v?.textTracks || []).map(t => ({ label: t.label, language: t.language })) }; })()".to_string(),
            timeout_ms: Some(120_000),
            capture_requests: vec![
                capture(".m3u8"),
                capture(".mpd"),
                capture("/stream/"),
            ],
            capture_events: Vec::new(),
            cookies: false,
            headless: Some(true),
            preload_scripts: Vec::new(),
        })?;
        streams_from_capture(&response, embed_url)
    }
}

impl VideoSource for ShuttleTv {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.page("/trending/all/week", page)
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.page("/trending/all/day", page)
    }

    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        let value = self.tmdb(
            "/search/multi",
            &[
                ("query", query.trim().to_string()),
                ("page", page.max(1).to_string()),
                ("include_adult", "false".to_string()),
            ],
        )?;
        parse_tmdb_page(&value)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let (kind, id) = parse_key(&item.key)?;
        let value = self.tmdb(
            &format!("/{kind}/{id}"),
            &[(
                "append_to_response",
                "images,recommendations,videos,content_ratings,release_dates".to_string(),
            )],
        )?;
        parse_tmdb_item(&value, Some(kind))
    }

    fn episodes(&mut self, item: CatalogItem) -> Result<Vec<VideoEpisode>> {
        let (kind, id) = parse_key(&item.key)?;
        if kind == "movie" {
            return Ok(vec![VideoEpisode {
                key: item.key,
                title: Some("Movie".to_string()),
                episode_number: Some(1.0),
                url: Some(format!("{BASE_URL}/watch/{id}")),
                ..VideoEpisode::default()
            }]);
        }
        let details = self.tmdb(&format!("/tv/{id}"), &[])?;
        let seasons = details
            .get("seasons")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::new("ShuttleTV returned no seasons"))?;
        let mut episodes = Vec::new();
        for season in seasons {
            let number = season
                .get("season_number")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if number == 0 {
                continue;
            }
            let value = self.tmdb(&format!("/tv/{id}/season/{number}"), &[])?;
            for episode in value
                .get("episodes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let episode_number = episode
                    .get("episode_number")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                if episode_number == 0 {
                    continue;
                }
                episodes.push(VideoEpisode {
                    key: format!("tv:{id}:{number}:{episode_number}"),
                    title: string(episode, "name"),
                    description: string(episode, "overview"),
                    episode_number: Some(episode_number as f32),
                    season_number: Some(number as f32),
                    thumbnail: image(episode, "still_path", "w500"),
                    url: Some(format!(
                        "{BASE_URL}/watch/{id}?s={number}&e={episode_number}"
                    )),
                    ..VideoEpisode::default()
                });
            }
        }
        Ok(episodes)
    }

    fn streams(&mut self, item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoStream>> {
        let (kind, id) = parse_key(&item.key)?;
        let embed = if kind == "movie" {
            format!("https://cinesrc.st/embed/movie/{id}")
        } else {
            let (_, episode_id, season, number) = parse_episode_key(&episode.key)?;
            if episode_id != id {
                return Err(Error::new("episode does not belong to this ShuttleTV item"));
            }
            format!("https://cinesrc.st/embed/tv/{id}?s={season}&e={number}")
        };
        self.capture_stream(&embed)
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        let (_, id) = parse_key(&item.key)?;
        Ok(Some(format!("{BASE_URL}/title/{id}")))
    }

    fn episode_url(
        &mut self,
        _item: &CatalogItem,
        episode: &VideoEpisode,
    ) -> Result<Option<String>> {
        Ok(episode.url.clone())
    }
}

fn parse_tmdb_page(value: &Value) -> Result<Paged<CatalogItem>> {
    let mut entries = Vec::new();
    for raw in value
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::new("TMDB response did not contain results"))?
    {
        let kind = string(raw, "media_type")
            .or_else(|| {
                if raw.get("first_air_date").is_some() {
                    Some("tv".to_string())
                } else {
                    Some("movie".to_string())
                }
            })
            .unwrap_or_default();
        if !matches!(kind.as_str(), "movie" | "tv") {
            continue;
        }
        entries.push(parse_tmdb_item(raw, Some(&kind))?);
    }
    let page = value.get("page").and_then(Value::as_u64).unwrap_or(1);
    let total = value
        .get("total_pages")
        .and_then(Value::as_u64)
        .unwrap_or(page);
    Ok(Paged::new(entries, page < total))
}

fn parse_tmdb_item(raw: &Value, forced_kind: Option<&str>) -> Result<CatalogItem> {
    let id = raw
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::new("TMDB item is missing id"))?;
    let kind = forced_kind
        .or_else(|| raw.get("media_type").and_then(Value::as_str))
        .unwrap_or("movie");
    let title = string(raw, "title")
        .or_else(|| string(raw, "name"))
        .ok_or_else(|| Error::new("TMDB item is missing title"))?;
    let tags = raw
        .get("genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|genre| string(genre, "name"))
        .collect();
    Ok(CatalogItem {
        key: format!("{kind}:{id}"),
        title,
        url: Some(format!("{BASE_URL}/title/{id}")),
        cover: image(raw, "poster_path", "w500"),
        banner: image(raw, "backdrop_path", "w1280"),
        description: string(raw, "overview"),
        tags,
        initialized: raw.get("genres").is_some(),
        language: string(raw, "original_language"),
        rating: raw
            .get("vote_average")
            .and_then(Value::as_f64)
            .map(|rating| rating as f32),
        content_rating: Some("suggestive".to_string()),
        ..CatalogItem::default()
    })
}

fn image(raw: &Value, key: &str, size: &str) -> Option<ImageRequest> {
    raw.get(key)
        .and_then(Value::as_str)
        .filter(|path| path.starts_with('/'))
        .map(|path| ImageRequest::get(format!("{IMAGE_URL}/{size}{path}")))
}

fn string(raw: &Value, key: &str) -> Option<String> {
    raw.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn capture(needle: &str) -> WebViewRequestCapture {
    WebViewRequestCapture {
        url_contains: Some(needle.to_string()),
        limit: Some(8),
        ..WebViewRequestCapture::default()
    }
}

fn streams_from_capture(
    response: &WebViewExtractResponse,
    referer: &str,
) -> Result<Vec<VideoStream>> {
    let script_url = response
        .value
        .as_ref()
        .and_then(|value| value.get("url"))
        .and_then(Value::as_str);
    let mut urls: Vec<String> = response
        .captured_requests
        .iter()
        .map(|request| request.url.clone())
        .filter(|url| media_url(url))
        .collect();
    if let Some(url) = script_url.filter(|url| media_url(url)) {
        urls.push(url.to_string());
    }
    urls.extend(
        response
            .value
            .as_ref()
            .and_then(|value| value.get("urls"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|url| media_url(url))
            .map(ToString::to_string),
    );
    urls.sort();
    urls.dedup();
    if urls.is_empty() {
        return Err(Error::new(
            "ShuttleTV player did not expose a playable stream",
        ));
    }
    Ok(urls
        .into_iter()
        .map(|url| {
            let lower = url.to_ascii_lowercase();
            let is_hls = lower.contains(".m3u8");
            let is_dash = lower.contains(".mpd") || lower.contains("/stream/mpd");
            VideoStream {
                url,
                name: Some("CineSrc".to_string()),
                format: Some(if is_dash { "dash" } else { "hls" }.to_string()),
                is_hls,
                is_dash,
                requires_proxy: true,
                initialized: true,
                headers: [
                    ("Referer".to_string(), referer.to_string()),
                    ("Origin".to_string(), "https://cinesrc.st".to_string()),
                ]
                .into_iter()
                .collect(),
                ..VideoStream::default()
            }
        })
        .collect())
}

fn media_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains(".m3u8") || lower.contains(".mpd") || lower.contains("/stream/mpd")
}

fn parse_key(key: &str) -> Result<(&str, u64)> {
    let (kind, id) = key
        .split_once(':')
        .ok_or_else(|| Error::new("invalid ShuttleTV key"))?;
    if !matches!(kind, "movie" | "tv") {
        return Err(Error::new("invalid ShuttleTV media type"));
    }
    let id = id
        .parse()
        .map_err(|_| Error::new("invalid ShuttleTV TMDB id"))?;
    Ok((kind, id))
}

fn parse_episode_key(key: &str) -> Result<(&str, u64, u64, u64)> {
    let mut parts = key.split(':');
    let kind = parts.next().unwrap_or_default();
    let id = parts.next().and_then(|v| v.parse().ok());
    let season = parts.next().and_then(|v| v.parse().ok());
    let episode = parts.next().and_then(|v| v.parse().ok());
    if kind != "tv" || parts.next().is_some() {
        return Err(Error::new("invalid ShuttleTV episode key"));
    }
    Ok((
        kind,
        id.ok_or_else(|| Error::new("missing TMDB id"))?,
        season.ok_or_else(|| Error::new("missing season"))?,
        episode.ok_or_else(|| Error::new("missing episode"))?,
    ))
}

fn validate_cinesrc_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    if url.scheme() != "https" || url.host_str() != Some("cinesrc.st") {
        return Err(Error::new("unexpected ShuttleTV player URL"));
    }
    Ok(())
}

fn cache_busted_url(value: &str, timestamp: i64) -> Result<String> {
    let mut url = Url::parse(value).map_err(url_error)?;
    url.query_pairs_mut()
        .append_pair("_manatan", &timestamp.to_string());
    Ok(url.into())
}

fn url_error(error: url::ParseError) -> Error {
    Error::new(format!("invalid URL: {error}"))
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(manatan_sdk::Extension::new().video("shuttletv", ShuttleTv));

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_movie_and_tv_results() {
        let page = parse_tmdb_page(&json!({
            "page": 1,
            "total_pages": 2,
            "results": [
                {"id": 550, "media_type": "movie", "title": "Fight Club", "poster_path": "/a.jpg"},
                {"id": 1396, "media_type": "tv", "name": "Breaking Bad", "poster_path": "/b.jpg"},
                {"id": 1, "media_type": "person", "name": "Ignored"}
            ]
        }))
        .unwrap();
        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.entries[0].key, "movie:550");
        assert_eq!(page.entries[1].key, "tv:1396");
        assert!(page.has_next_page);
    }

    #[test]
    fn accepts_only_owned_episode_keys_and_player_urls() {
        assert_eq!(parse_episode_key("tv:1396:2:3").unwrap().2, 2);
        assert!(parse_episode_key("movie:550:1:1").is_err());
        assert!(validate_cinesrc_url("https://cinesrc.st/embed/movie/550").is_ok());
        assert!(validate_cinesrc_url("https://example.com/embed/movie/550").is_err());
        assert_eq!(
            cache_busted_url("https://cinesrc.st/embed/tv/1396?s=2&e=3", 42).unwrap(),
            "https://cinesrc.st/embed/tv/1396?s=2&e=3&_manatan=42"
        );
    }

    #[test]
    fn reads_streams_from_webkit_resource_timing() {
        let response = WebViewExtractResponse {
            value: Some(json!({
                "url": "blob:https://cinesrc.st/player",
                "urls": [
                    "https://media.example/master.m3u8",
                    "https://media.example/poster.jpg"
                ]
            })),
            ..WebViewExtractResponse::default()
        };
        let streams = streams_from_capture(&response, "https://cinesrc.st/embed/movie/550")
            .expect("resource timing should expose the native-WebKit HLS URL");
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].url, "https://media.example/master.m3u8");
        assert!(streams[0].is_hls);
        assert!(streams[0].requires_proxy);
    }
}
