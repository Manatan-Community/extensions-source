use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
    VideoEpisode, VideoHoster, VideoStream, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{dates, html, url, video};
use serde_json::Value;

const BASE_URL: &str = "https://video.example";
const SOURCE: ExampleVideo = ExampleVideo;

struct ExampleVideo;

impl VideoSource for ExampleVideo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let fixture = if listing == "latest" {
            LATEST_FIXTURE
        } else {
            POPULAR_FIXTURE
        };
        Ok(Paged {
            entries: parse_listing(fixture),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![parse_details(DETAILS_FIXTURE)],
                has_next_page: false,
            });
        }
        let mut entries = parse_listing(POPULAR_FIXTURE);
        if !query.is_empty() {
            entries.retain(|item| item.title.to_lowercase().contains(&query.to_lowercase()));
        }
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(parse_details(DETAILS_FIXTURE))
    }

    fn episodes(&self, _request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        Ok(parse_episodes(EPISODES_FIXTURE))
    }

    fn streams(&self, _request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut streams = parse_playlist(PLAYLIST_FIXTURE);
        streams.push(video::torrent_stream("magnet:?xt=urn:btih:example-video-fixture"));
        video::sort_streams(&mut streams);
        Ok(streams)
    }

    fn hosters(&self, _request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        Ok(parse_hosters(HOSTERS_FIXTURE))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let token = request
            .get("token")
            .and_then(Value::as_str)
            .unwrap_or_else(|| extract_webview_signature(WEBVIEW_FIXTURE).unwrap_or("fixture"));
        Ok(vec![video::hls_stream(
            &format!("https://media.example/hls/master.m3u8?sig={token}"),
            "1080p",
            BASE_URL,
        )])
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: parse_listing(POPULAR_FIXTURE),
            has_more: true,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.contains("/show/") {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(DETAILS_FIXTURE)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(None)
    }
}

fn parse_listing(input: &str) -> Vec<CatalogItem> {
    input
        .split("<article")
        .skip(1)
        .filter_map(|chunk| {
            let key = html::attr(chunk, "data-key")?;
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| key.clone());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|path| url::join_url(BASE_URL, &path)),
                url: Some(url::join_url(BASE_URL, &format!("/show/{key}"))),
                tags: vec!["adventure".to_string(), "subbed".to_string()],
                language: Some("en".to_string()),
                status: ItemStatus::Ongoing,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(input: &str) -> CatalogItem {
    CatalogItem {
        key: "signal-tower".to_string(),
        title: html::text_between(input, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_else(|| "Signal Tower".to_string()),
        cover: html::attr_after(input, "<img", "src").map(|path| url::join_url(BASE_URL, &path)),
        url: Some(url::join_url(BASE_URL, "/show/signal-tower")),
        description: html::text_between(input, "<p", "</p>").map(|value| html::strip_tags(&value)),
        tags: vec!["adventure".to_string()],
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        latest_update: dates::parse_fixture_date("2024-02-01"),
        status: ItemStatus::Ongoing,
        ..CatalogItem::default()
    }
}

fn parse_episodes(input: &str) -> Vec<VideoEpisode> {
    input
        .split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let key = html::attr(chunk, "data-key")?;
            Some(VideoEpisode {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)),
                episode_number: key.rsplit('-').next().and_then(|value| value.parse().ok()),
                season_number: Some(1.0),
                date_uploaded: dates::parse_fixture_date("2024-02-01"),
                thumbnail: Some(url::join_url(BASE_URL, &format!("/thumbs/{key}.jpg"))),
                url: Some(url::join_url(BASE_URL, &format!("/show/signal-tower/{key}"))),
                duration_seconds: Some(1420.0),
                language: Some("en".to_string()),
                labels: vec!["subbed".to_string()],
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_hosters(input: &str) -> Vec<VideoHoster> {
    input
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let key = html::attr(chunk, "data-hoster")?;
            let href = html::attr(chunk, "href")?;
            Some(video::hoster(&key, &html::strip_tags(chunk), &href))
        })
        .collect()
}

fn parse_playlist(input: &str) -> Vec<VideoStream> {
    input
        .lines()
        .filter(|line| line.ends_with(".m3u8"))
        .enumerate()
        .map(|(index, line)| {
            let quality = if index == 0 { "1080p" } else { "720p" };
            video::hls_stream(line, quality, BASE_URL)
        })
        .collect()
}

fn extract_webview_signature(input: &str) -> Option<&str> {
    input.split("signature:").nth(1)?.split('"').nth(1)
}

const POPULAR_FIXTURE: &str = r#"
<article data-key="signal-tower"><img src="/covers/signal-tower.jpg"><h3>Signal Tower</h3></article>
<article data-key="night-market"><img src="/covers/night-market.jpg"><h3>Night Market</h3></article>
"#;

const LATEST_FIXTURE: &str = r#"
<article data-key="rain-room"><img src="/covers/rain-room.jpg"><h3>Rain Room</h3></article>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Signal Tower</h1>
<img src="/covers/signal-tower.jpg">
<p>A fixture video source used to demonstrate details parsing.</p>
"#;

const EPISODES_FIXTURE: &str = r#"
<li data-key="episode-1"><a href="/show/signal-tower/episode-1">Arrival</a></li>
<li data-key="episode-2"><a href="/show/signal-tower/episode-2">Lantern Street</a></li>
"#;

const HOSTERS_FIXTURE: &str = r#"
<a data-hoster="fixture-hls" href="https://hoster.example/embed/abc">Fixture HLS</a>
"#;

const PLAYLIST_FIXTURE: &str = r#"
#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080
https://media.example/hls/1080p.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1280x720
https://media.example/hls/720p.m3u8
"#;

const WEBVIEW_FIXTURE: &str = r#"
<script>window.player = { signature:"fixture-signature" };</script>
"#;

export_video_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_episode_fixture() {
        let episodes = parse_episodes(EPISODES_FIXTURE);
        assert_eq!(episodes.len(), 2);
        assert_eq!(episodes[0].episode_number, Some(1.0));
    }

    #[test]
    fn parses_hoster_fixture() {
        let hosters = parse_hosters(HOSTERS_FIXTURE);
        assert_eq!(hosters[0].key, "fixture-hls");
    }

    #[test]
    fn parses_playlist_fixture() {
        let streams = parse_playlist(PLAYLIST_FIXTURE);
        assert_eq!(streams.len(), 2);
        assert!(streams[0].is_hls);
        assert_eq!(extract_webview_signature(WEBVIEW_FIXTURE), Some("fixture-signature"));
    }
}
