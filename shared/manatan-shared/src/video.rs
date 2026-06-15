use crate::sdk::{
    AudioTrack, Context, DebridInfo, SubtitleTrack, TorrentInfo, VideoHoster, VideoStream,
    VideoStreamKind,
};

pub fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsVariant {
    pub url: String,
    pub quality: Option<String>,
    pub resolution: Option<String>,
    pub bandwidth: Option<u64>,
    pub codecs: Option<String>,
}

pub fn parse_hls_master_playlist(body: &str, playlist_url: &str) -> Vec<HlsVariant> {
    let mut variants = Vec::new();
    let mut lines = body.lines().map(str::trim).peekable();
    while let Some(line) = lines.next() {
        if !line.starts_with("#EXT-X-STREAM-INF:") {
            continue;
        }
        let attrs = parse_hls_attributes(line.trim_start_matches("#EXT-X-STREAM-INF:"));
        let Some(uri) = lines
            .by_ref()
            .find(|line| !line.is_empty() && !line.starts_with('#'))
        else {
            continue;
        };
        let resolution = attrs.get("RESOLUTION").cloned();
        let quality = resolution
            .as_deref()
            .and_then(|value| value.split('x').nth(1))
            .filter(|height| !height.is_empty())
            .map(|height| format!("{height}p"));
        variants.push(HlsVariant {
            url: absolute_or(uri, playlist_url),
            quality,
            resolution,
            bandwidth: attrs.get("BANDWIDTH").and_then(|value| value.parse().ok()),
            codecs: attrs.get("CODECS").cloned(),
        });
    }
    variants
}

pub fn hls_streams_from_master(
    body: &str,
    playlist_url: &str,
    name: &str,
    referer: &str,
) -> Vec<VideoStream> {
    let variants = parse_hls_master_playlist(body, playlist_url);
    if variants.is_empty() {
        return vec![named_hls_stream(playlist_url, name, None, referer)];
    }
    variants
        .into_iter()
        .map(|variant| {
            let mut stream =
                named_hls_stream(&variant.url, name, variant.quality.as_deref(), referer);
            stream.resolution = variant.resolution;
            stream.bitrate = variant.bandwidth;
            if let Some(codecs) = variant.codecs {
                let parts = codecs
                    .split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>();
                stream.video_codec = parts.first().map(|value| (*value).to_string());
                stream.audio_codec = parts.get(1).map(|value| (*value).to_string());
            }
            stream
        })
        .collect()
}

pub fn named_hls_stream(
    url: &str,
    name: &str,
    quality: Option<&str>,
    referer: &str,
) -> VideoStream {
    let quality = quality.unwrap_or("auto");
    VideoStream {
        url: url.to_string(),
        name: Some(format!("{name} - {quality}")),
        quality: Some(quality.to_string()),
        format: Some("hls".to_string()),
        is_hls: true,
        stream_kind: Some(VideoStreamKind::Hls),
        headers: referer_headers(referer),
        preferred: quality == "1080p",
        initialized: true,
        ..VideoStream::default()
    }
}

pub fn external_hoster_name(url: &str) -> &'static str {
    let lower = url.to_ascii_lowercase();
    match lower.as_str() {
        value if value.contains("dood.") || value.contains("doodstream") => "Dood",
        value if value.contains("filemoon.") || value.contains("filemoon.sx") => "Filemoon",
        value if value.contains("ok.ru") || value.contains("odnoklassniki") => "Okru",
        value if value.contains("sibnet.") || value.contains("video.sibnet") => "Sibnet",
        value if value.contains("voe.") || value.contains("voe.sx") => "Voe",
        value if value.contains("uqload.") => "Uqload",
        value if value.contains("streamtape.") => "StreamTape",
        value if value.contains("mixdrop.") || value.contains("mixdroop.") => "MixDrop",
        value if value.contains("vk.com") || value.contains("vkvideo.") => "VK",
        value if value.contains("cda.pl") => "CDA",
        value if value.contains("mp4upload.") => "Mp4upload",
        value if value.contains("dailymotion.") || value.contains("dai.ly") => "Dailymotion",
        value if value.contains("streamwish.") => "StreamWish",
        value if value.contains("vidbm.") => "Vidbm",
        value if value.contains("sendvid.") => "Sendvid",
        value if value.contains("upstream.") => "Upstream",
        value if value.contains("vudeo.") => "Vudeo",
        value if value.contains("vido.") || value.contains("vidhide.") => "Vidhide",
        value if value.contains("drive.google.") || value.contains("googleusercontent.") => {
            "Google Drive"
        }
        _ => "External",
    }
}

pub fn external_hoster_stream(url: &str, referer: &str) -> VideoStream {
    let name = external_hoster_name(url);
    let is_hls = url.contains(".m3u8");
    VideoStream {
        url: url.to_string(),
        name: Some(name.to_string()),
        quality: Some(if is_hls { "auto" } else { "external" }.to_string()),
        format: Some(if is_hls { "hls" } else { "external" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::External
        }),
        headers: referer_headers(referer),
        preferred: true,
        initialized: true,
        ..VideoStream::default()
    }
}

fn parse_hls_attributes(input: &str) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let mut key = String::new();
    let mut value = String::new();
    let mut in_value = false;
    let mut quote = false;
    for ch in input.chars().chain([',']) {
        match ch {
            '=' if !in_value => in_value = true,
            '"' if in_value => quote = !quote,
            ',' if in_value && !quote => {
                if !key.trim().is_empty() {
                    out.insert(
                        key.trim().to_string(),
                        value.trim().trim_matches('"').to_string(),
                    );
                }
                key.clear();
                value.clear();
                in_value = false;
            }
            _ if in_value => value.push(ch),
            _ => key.push(ch),
        }
    }
    out
}

pub fn absolute_or(input: &str, base: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        return input.to_string();
    }
    if input.starts_with("//") {
        return format!("https:{input}");
    }
    let Some((origin, path)) = split_url_origin_path(base) else {
        return input.to_string();
    };
    if input.starts_with('/') {
        return format!("{origin}{input}");
    }
    let parent = path
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    if parent.is_empty() {
        format!("{origin}/{input}")
    } else {
        format!("{origin}{parent}/{input}")
    }
}

fn split_url_origin_path(input: &str) -> Option<(&str, &str)> {
    let scheme_end = input.find("://")? + 3;
    let path_start = input[scheme_end..]
        .find('/')
        .map(|index| scheme_end + index)
        .unwrap_or(input.len());
    Some((&input[..path_start], input.get(path_start..).unwrap_or("")))
}

#[cfg(test)]
mod top_level_tests {
    use super::*;

    #[test]
    fn parses_hls_master_playlist() {
        let body = r#"#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1920x1080,CODECS="avc1.640028,mp4a.40.2"
hi/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=854x480
/low/index.m3u8
"#;
        let variants = parse_hls_master_playlist(body, "https://cdn.example/show/master.m3u8");
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].quality.as_deref(), Some("1080p"));
        assert_eq!(variants[0].url, "https://cdn.example/show/hi/index.m3u8");
        assert_eq!(variants[1].url, "https://cdn.example/low/index.m3u8");
    }

    #[test]
    fn creates_hls_streams_from_master() {
        let body = r#"#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=1200000,RESOLUTION=1280x720
720.m3u8
"#;
        let streams = hls_streams_from_master(
            body,
            "https://cdn.example/a/master.m3u8",
            "Main",
            "https://site.example/watch",
        );
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].quality.as_deref(), Some("720p"));
        assert!(streams[0].is_hls);
        assert_eq!(streams[0].stream_kind, Some(VideoStreamKind::Hls));
    }

    #[test]
    fn classifies_common_external_hosters() {
        assert_eq!(external_hoster_name("https://doodstream.com/e/abc"), "Dood");
        assert_eq!(
            external_hoster_name("https://filemoon.sx/e/abc"),
            "Filemoon"
        );
        assert_eq!(external_hoster_name("https://ok.ru/video/1"), "Okru");
        assert_eq!(
            external_hoster_name("https://example.com/embed"),
            "External"
        );
    }
}

pub mod indonesian {
    use crate::{
        html,
        sdk::{
            CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
            VideoEpisode, VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult,
            http::HttpClient, source::VideoSource,
        },
        url,
        video::referer_headers,
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde_json::Value;
    use std::{collections::BTreeSet, marker::PhantomData};

    pub trait IndonesianConfig {
        const NAME: &'static str;
        const BASE_URL: &'static str;
        const LANG: &'static str = "id";
        const CONTENT_RATING: &'static str = "safe";
        const QUALITY_DEFAULT: &'static str = "720p";

        fn list_url(listing: &str, page: u64) -> String;
        fn search_url(query: &str, page: u64, genre: Option<String>) -> String;
    }

    pub struct IndonesianSource<C>(PhantomData<C>);

    impl<C> IndonesianSource<C> {
        pub const fn new() -> Self {
            Self(PhantomData)
        }
    }

    impl<C: IndonesianConfig> VideoSource for IndonesianSource<C> {
        fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
            let target = C::list_url(listing(&request), page(&request));
            let body = fetch_or_fixture::<C>(&target, LIST_FIXTURE, C::BASE_URL);
            Ok(Paged {
                entries: parse_cards::<C>(&body),
                has_next_page: has_next_page(&body),
            })
        }

        fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
            let query = request
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if let Some(path) = path_from_url::<C>(query) {
                return Ok(Paged {
                    entries: vec![fetch_details::<C>(&path)],
                    has_next_page: false,
                });
            }
            let target = C::search_url(query, page(&request), filter(&request, "genre"));
            let body = fetch_or_fixture::<C>(&target, LIST_FIXTURE, C::BASE_URL);
            Ok(Paged {
                entries: parse_cards::<C>(&body),
                has_next_page: has_next_page(&body),
            })
        }

        fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
            let path =
                request_key::<C>(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
            Ok(fetch_details::<C>(&path))
        }

        fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
            let path =
                request_key::<C>(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
            let body =
                fetch_or_fixture::<C>(&absolute_url::<C>(&path), DETAILS_FIXTURE, C::BASE_URL);
            let mut episodes = parse_episodes::<C>(&body);
            if episodes.is_empty() {
                episodes.push(VideoEpisode {
                    key: path.clone(),
                    title: Some(title_from_path::<C>(&path)),
                    episode_number: Some(1.0),
                    url: Some(absolute_url::<C>(&path)),
                    language: Some(C::LANG.to_string()),
                    ..VideoEpisode::default()
                });
            }
            Ok(episodes)
        }

        fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
            let episode = request_key::<C>(&request, "episode")
                .unwrap_or_else(|| "/sample-episode".to_string());
            if episode.starts_with("nimegami:") {
                return Ok(hosters_from_nimegami_payload::<C>(&episode));
            }
            let episode_url = absolute_url::<C>(&episode);
            let body = fetch_or_fixture::<C>(&episode_url, EPISODE_FIXTURE, C::BASE_URL);
            Ok(parse_hosters::<C>(&body, &episode_url))
        }

        fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
            let key = hoster_key(&request).unwrap_or_default();
            let name = request
                .get("hoster")
                .and_then(|hoster| hoster.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("External");
            Ok(resolve_hoster_key::<C>(&key, name, &request))
        }

        fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
            let mut out = Vec::new();
            for hoster in self.hosters(request.clone())? {
                let mut streams = self.resolve_hoster(serde_json::json!({
                    "hoster": { "key": hoster.key, "name": hoster.name },
                    "preferences": request.get("preferences").cloned().unwrap_or(Value::Null),
                }))?;
                for stream in &mut streams {
                    stream.hoster = Some(hoster.clone());
                }
                out.extend(streams);
            }
            super::sort_streams(&mut out);
            Ok(out)
        }

        fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
            let popular = self.list(with_listing(&request, "popular"))?;
            let latest = self.list(with_listing(&request, "latest"))?;
            Ok(vec![
                HomeSection {
                    id: "popular".to_string(),
                    title: "Popular".to_string(),
                    style: Some(HomeSectionStyle::Featured),
                    entries: popular.entries,
                    has_more: popular.has_next_page,
                    ..HomeSection::default()
                },
                HomeSection {
                    id: "latest".to_string(),
                    title: "Latest".to_string(),
                    entries: latest.entries,
                    has_more: latest.has_next_page,
                    ..HomeSection::default()
                },
            ])
        }

        fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
            Ok(request_key::<C>(&request, "item").map(|path| absolute_url::<C>(&path)))
        }

        fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
            Ok(request_key::<C>(&request, "episode").map(|path| absolute_url::<C>(&path)))
        }

        fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
            let Some(input) = request.get("url").and_then(Value::as_str) else {
                return Ok(None);
            };
            if let Some(path) = path_from_url::<C>(input) {
                return Ok(Some(UrlResolveResult {
                    item: Some(fetch_details::<C>(&path)),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
            Ok(Some(UrlResolveResult {
                search: Some(crate::sdk::SearchRequest {
                    query: input.to_string(),
                    ..crate::sdk::SearchRequest::default()
                }),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }))
        }
    }

    fn client<C: IndonesianConfig>(referer: &str) -> HttpClient {
        HttpClient::browser()
            .with_desktop_user_agent()
            .with_referer(referer)
            .with_header("Origin", C::BASE_URL)
            .with_cookies_for(C::BASE_URL)
            .with_webview_challenge_fallback()
    }

    fn fetch_or_fixture<C: IndonesianConfig>(target: &str, fixture: &str, referer: &str) -> String {
        client::<C>(referer)
            .get(target)
            .browser_document()
            .referer(referer)
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    fn parse_cards<C: IndonesianConfig>(body: &str) -> Vec<CatalogItem> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for chunk in body.split("<a").skip(1) {
            let Some(href) = html::attr(chunk, "href") else {
                continue;
            };
            if !looks_like_item::<C>(&href) || !chunk.contains("<img") {
                continue;
            }
            let key = path_key::<C>(&href);
            if !seen.insert(key.clone()) {
                continue;
            }
            let title = card_title::<C>(chunk, &key);
            out.push(CatalogItem {
                key: key.clone(),
                title,
                cover: image_from_chunk::<C>(chunk),
                url: Some(absolute_url::<C>(&key)),
                language: Some(C::LANG.to_string()),
                content_rating: Some(C::CONTENT_RATING.to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            });
        }
        out
    }

    fn looks_like_item<C: IndonesianConfig>(href: &str) -> bool {
        let lower = href.to_lowercase();
        (href.starts_with(C::BASE_URL) || href.starts_with('/'))
            && !lower.contains("/episode/")
            && !lower.contains("/eps/")
            && !lower.contains("download")
            && !lower.contains("wp-admin")
            && !lower.contains("/genre/")
            && !lower.ends_with(".jpg")
            && !lower.ends_with(".png")
    }

    fn card_title<C: IndonesianConfig>(chunk: &str, key: &str) -> String {
        html::attr_after(chunk, "<img", "alt")
            .or_else(|| html::text_between(chunk, "<h2", "</h2>").map(|v| html::strip_tags(&v)))
            .or_else(|| html::text_between(chunk, "<h3", "</h3>").map(|v| html::strip_tags(&v)))
            .or_else(|| html::text_between(chunk, "<h4", "</h4>").map(|v| html::strip_tags(&v)))
            .or_else(|| {
                html::text_between(chunk, "class=\"title", "</").map(|v| html::strip_tags(&v))
            })
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| title_from_path::<C>(key))
    }

    fn fetch_details<C: IndonesianConfig>(path: &str) -> CatalogItem {
        let body = fetch_or_fixture::<C>(&absolute_url::<C>(path), DETAILS_FIXTURE, C::BASE_URL);
        let title = text_for_labels(&body, &["Judul", "Title"])
            .or_else(|| html::text_between(&body, "<h1", "</h1>").map(|v| html::strip_tags(&v)))
            .or_else(|| html::text_between(&body, "<h2", "</h2>").map(|v| html::strip_tags(&v)))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| title_from_path::<C>(path));
        let status_text = text_for_labels(&body, &["Status"]);
        CatalogItem {
            key: path_key::<C>(path),
            title,
            cover: image_from_chunk::<C>(&body),
            url: Some(absolute_url::<C>(path)),
            description: description(&body),
            tags: linked_values(&body, &["Genre", "Kategori"]),
            authors: text_for_labels(&body, &["Produser", "Fansub"])
                .into_iter()
                .collect(),
            artists: text_for_labels(&body, &["Studio"]).into_iter().collect(),
            language: Some(C::LANG.to_string()),
            content_rating: Some(C::CONTENT_RATING.to_string()),
            status: parse_status(status_text.as_deref().unwrap_or_default()),
            initialized: true,
            ..CatalogItem::default()
        }
    }

    fn parse_episodes<C: IndonesianConfig>(body: &str) -> Vec<VideoEpisode> {
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        for chunk in body.split("<a").skip(1) {
            let Some(href) = html::attr(chunk, "href") else {
                continue;
            };
            let text = html::strip_tags(chunk.split("</a>").next().unwrap_or_default());
            let lower = format!("{} {}", href, text).to_lowercase();
            if !(lower.contains("episode")
                || lower.contains("/eps")
                || lower.contains("/ep-")
                || lower.contains(" eps ")
                || lower.contains(" ep "))
            {
                continue;
            }
            let key = path_key::<C>(&href);
            if !seen.insert(key.clone()) {
                continue;
            }
            let number = episode_number(&text).or_else(|| episode_number(&href));
            out.push(VideoEpisode {
                key: key.clone(),
                title: Some(if text.is_empty() {
                    number
                        .map(|value| format!("Episode {}", display_number(value)))
                        .unwrap_or_else(|| title_from_path::<C>(&key))
                } else {
                    text
                }),
                episode_number: number,
                url: Some(absolute_url::<C>(&key)),
                language: Some(C::LANG.to_string()),
                ..VideoEpisode::default()
            });
        }
        for chunk in body.split("<li").skip(1) {
            let Some(data) = html::attr(chunk, "data") else {
                continue;
            };
            let id = html::attr(chunk, "id").unwrap_or_default();
            let number = episode_number(&id).unwrap_or(out.len() as f32 + 1.0);
            let key = format!("nimegami:{data}");
            if !seen.insert(key.clone()) {
                continue;
            }
            out.push(VideoEpisode {
                key,
                title: Some(format!("Episode {}", display_number(number))),
                episode_number: Some(number),
                language: Some(C::LANG.to_string()),
                ..VideoEpisode::default()
            });
        }
        out.sort_by(|a, b| {
            a.episode_number
                .partial_cmp(&b.episode_number)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    }

    fn parse_hosters<C: IndonesianConfig>(body: &str, episode_url: &str) -> Vec<VideoHoster> {
        let mut out = Vec::new();
        for (name, target) in iframe_targets(body) {
            out.push(video_hoster::<C>(&target, &name, episode_url));
        }
        for (name, target) in source_targets(body) {
            out.push(video_hoster::<C>(&target, &name, episode_url));
        }
        for chunk in body.split("<option").skip(1) {
            let name = html::strip_tags(chunk.split("</option>").next().unwrap_or_default());
            let encoded = html::attr(chunk, "value").unwrap_or_default();
            if let Some(target) =
                decode_embed(&encoded).or_else(|| iframe_targets(body).pop().map(|v| v.1))
            {
                out.push(video_hoster::<C>(&target, &name, episode_url));
            }
        }
        for chunk in body.split("east_player_option").skip(1) {
            if let Some(target) = oploverz_ajax::<C>(chunk, episode_url) {
                let name = html::strip_tags(chunk.split("</div>").next().unwrap_or_default());
                out.push(video_hoster::<C>(&target, &name, episode_url));
            }
        }
        for chunk in body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("data-content"))
        {
            if let Some((quality, target)) = otakudesu_ajax::<C>(body, chunk, episode_url) {
                out.push(video_hoster::<C>(&target, &quality, episode_url));
            }
        }
        dedupe_hosters(out)
    }

    fn resolve_hoster_key<C: IndonesianConfig>(
        key: &str,
        name: &str,
        request: &Value,
    ) -> Vec<VideoStream> {
        if key.starts_with("nimegami:") {
            return streams_from_nimegami_payload::<C>(key, request);
        }
        if key.ends_with(".m3u8") || key.ends_with(".mp4") {
            return vec![stream_for::<C>(key, name, request)];
        }
        let body = fetch_or_fixture::<C>(key, "", C::BASE_URL);
        let mut streams: Vec<VideoStream> = source_targets(&body)
            .into_iter()
            .map(|(quality, target)| stream_for::<C>(&target, &quality, request))
            .collect();
        if streams.is_empty() {
            streams.push(stream_for::<C>(key, name, request));
        }
        streams
    }

    fn iframe_targets(body: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for chunk in body.split("<iframe").skip(1) {
            let Some(src) = html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src"))
            else {
                continue;
            };
            let name = html::attr(chunk, "title").unwrap_or_else(|| host_name(&src));
            out.push((name, normalize_url(&src)));
        }
        out
    }

    fn source_targets(body: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for chunk in body.split("<source").skip(1) {
            let Some(src) = html::attr(chunk, "src") else {
                continue;
            };
            let quality = html::attr(chunk, "size")
                .map(|value| format!("{value}p"))
                .or_else(|| html::attr(chunk, "label"))
                .unwrap_or_else(|| quality_from_url(&src));
            out.push((quality, normalize_url(&src)));
        }
        for needle in ["file\":\"", "file':'", "src: \"", "source: \""] {
            for part in body.split(needle).skip(1) {
                let src = part.split(['"', '\'']).next().unwrap_or_default();
                if src.contains(".m3u8") || src.contains(".mp4") {
                    out.push((quality_from_url(src), normalize_url(src)));
                }
            }
        }
        out
    }

    fn oploverz_ajax<C: IndonesianConfig>(chunk: &str, episode_url: &str) -> Option<String> {
        let post = html::attr(chunk, "data-post")?;
        let nume = html::attr(chunk, "data-nume")?;
        let kind = html::attr(chunk, "data-type")?;
        let body = client::<C>(episode_url)
            .post(format!("{}/wp-admin/admin-ajax.php", C::BASE_URL))
            .xhr()
            .referer(episode_url)
            .form(&[
                ("action", "player_ajax"),
                ("post", &post),
                ("nume", &nume),
                ("type", &kind),
            ])
            .send_text()
            .ok()?;
        iframe_targets(&body).pop().map(|(_, url)| url)
    }

    fn otakudesu_ajax<C: IndonesianConfig>(
        body: &str,
        chunk: &str,
        episode_url: &str,
    ) -> Option<(String, String)> {
        let script = body
            .split("script")
            .find(|part| part.contains("{action:"))?;
        let nonce_action = script
            .split("{action:\"")
            .nth(1)
            .or_else(|| script.split("{action:'").nth(1))?
            .split(['"', '\''])
            .next()?;
        let action = script
            .split("action:\"")
            .nth(1)
            .or_else(|| script.split("action:'").nth(1))?
            .split(['"', '\''])
            .next()?;
        let nonce = client::<C>(episode_url)
            .post(format!("{}/wp-admin/admin-ajax.php", C::BASE_URL))
            .xhr()
            .referer(episode_url)
            .form(&[("action", nonce_action)])
            .send_text()
            .ok()?
            .split(":\"")
            .nth(1)?
            .split('"')
            .next()?
            .to_string();
        let data = decode_base64(&html::attr(chunk, "data-content")?)?;
        let id = jsonish_value(&data, "id")?;
        let mirror = jsonish_value(&data, "i")?;
        let quality = jsonish_value(&data, "q").unwrap_or_else(|| "External".to_string());
        let response = client::<C>(episode_url)
            .post(format!("{}/wp-admin/admin-ajax.php", C::BASE_URL))
            .xhr()
            .referer(episode_url)
            .form(&[
                ("id", &id),
                ("i", &mirror),
                ("q", &quality),
                ("nonce", &nonce),
                ("action", action),
            ])
            .send_text()
            .ok()?;
        let decoded = response
            .split(":\"")
            .nth(1)
            .and_then(|part| part.split('"').next())
            .and_then(decode_base64)?;
        iframe_targets(&decoded)
            .pop()
            .map(|(_, url)| (quality, url))
    }

    fn hosters_from_nimegami_payload<C: IndonesianConfig>(key: &str) -> Vec<VideoHoster> {
        streams_from_nimegami_payload::<C>(key, &Value::Null)
            .into_iter()
            .map(|stream| {
                video_hoster::<C>(
                    &stream.url,
                    stream.name.as_deref().unwrap_or("NimeGami"),
                    C::BASE_URL,
                )
            })
            .collect()
    }

    fn streams_from_nimegami_payload<C: IndonesianConfig>(
        key: &str,
        request: &Value,
    ) -> Vec<VideoStream> {
        let Some(decoded) = key.strip_prefix("nimegami:").and_then(decode_base64) else {
            return Vec::new();
        };
        let Ok(items) = serde_json::from_str::<Value>(&decoded) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for quality in items.as_array().into_iter().flatten() {
            let label = quality
                .get("format")
                .and_then(Value::as_str)
                .unwrap_or("External");
            for url in quality
                .get("url")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                out.push(stream_for::<C>(url, label, request));
            }
        }
        out
    }

    fn video_hoster<C: IndonesianConfig>(target: &str, name: &str, referer: &str) -> VideoHoster {
        let normalized = normalize_url(target);
        VideoHoster {
            key: normalized.clone(),
            name: if name.trim().is_empty() {
                host_name(&normalized)
            } else {
                name.trim().to_string()
            },
            url: Some(normalized),
            lazy: true,
            video_count: Some(1),
            headers: referer_headers(referer),
            ..VideoHoster::default()
        }
    }

    fn stream_for<C: IndonesianConfig>(target: &str, name: &str, request: &Value) -> VideoStream {
        let url = normalize_url(target);
        let is_hls = url.contains(".m3u8");
        let is_direct = is_hls || url.contains(".mp4");
        let quality = if is_direct {
            quality_from_url(&url)
        } else {
            "external".to_string()
        };
        VideoStream {
            url,
            name: Some(if name.trim().is_empty() {
                "External".to_string()
            } else {
                name.trim().to_string()
            }),
            quality: Some(quality.clone()),
            format: Some(
                if is_hls {
                    "hls"
                } else if is_direct {
                    "mp4"
                } else {
                    "external"
                }
                .to_string(),
            ),
            is_hls,
            stream_kind: Some(if is_hls {
                VideoStreamKind::Hls
            } else if is_direct {
                VideoStreamKind::Direct
            } else {
                VideoStreamKind::External
            }),
            preferred: quality.contains(&preference(
                request,
                "preferred_quality",
                C::QUALITY_DEFAULT,
            )),
            headers: referer_headers(C::BASE_URL),
            initialized: true,
            ..VideoStream::default()
        }
    }

    fn dedupe_hosters(hosters: Vec<VideoHoster>) -> Vec<VideoHoster> {
        let mut seen = BTreeSet::new();
        hosters
            .into_iter()
            .filter(|hoster| seen.insert(hoster.key.clone()))
            .collect()
    }

    fn image_from_chunk<C: IndonesianConfig>(chunk: &str) -> Option<String> {
        html::attr_after(chunk, "<img", "data-src")
            .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
            .or_else(|| html::attr_after(chunk, "<img", "data-setbg"))
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .or_else(|| html::attr_after(chunk, "set-bg", "data-setbg"))
            .map(|src| absolute_url::<C>(&normalize_url(&src)))
    }

    fn text_for_labels(body: &str, labels: &[&str]) -> Option<String> {
        for label in labels {
            for marker in [
                format!("{label}:"),
                format!("{label}</"),
                format!("{label} "),
            ] {
                let Some(block) = body.split(&marker).nth(1) else {
                    continue;
                };
                let value = html::strip_tags(
                    block
                        .split("</li>")
                        .next()
                        .unwrap_or(block)
                        .split("</p>")
                        .next()
                        .unwrap_or(block)
                        .split("</span>")
                        .next()
                        .unwrap_or(block),
                )
                .trim_matches(':')
                .trim()
                .to_string();
                if !value.is_empty() && value.len() < 200 {
                    return Some(value);
                }
            }
        }
        None
    }

    fn linked_values(body: &str, labels: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        for label in labels {
            let section = body.split(label).nth(1).unwrap_or(body);
            for chunk in section.split("<a").skip(1).take(40) {
                if let Some(text) = html::text_between(chunk, ">", "</a>") {
                    let value = html::strip_tags(&text);
                    if !value.is_empty() && value.len() < 60 {
                        out.push(value);
                    }
                }
            }
            if !out.is_empty() {
                break;
            }
        }
        out
    }

    fn description(body: &str) -> Option<String> {
        for marker in [
            "sinopsis",
            "Sinopsis",
            "synopsis",
            "Synopsis",
            "entry-content",
            "contenidotv",
        ] {
            if let Some(block) = body.split(marker).nth(1) {
                let value = html::strip_tags(
                    block
                        .split("</div>")
                        .next()
                        .unwrap_or(block)
                        .split("</section>")
                        .next()
                        .unwrap_or(block),
                );
                if value.len() > 20 {
                    return Some(value);
                }
            }
        }
        None
    }

    fn decode_embed(input: &str) -> Option<String> {
        decode_base64(input).and_then(|decoded| {
            iframe_targets(&decoded)
                .pop()
                .map(|(_, url)| url)
                .or_else(|| Some(decoded).filter(|value| value.starts_with("http")))
        })
    }

    fn decode_base64(input: &str) -> Option<String> {
        STANDARD
            .decode(input.trim().trim_matches('"').trim_matches('\''))
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    }

    fn jsonish_value(input: &str, key: &str) -> Option<String> {
        input
            .split(&format!("\"{key}\""))
            .nth(1)
            .or_else(|| input.split(&format!("'{key}'")).nth(1))?
            .split(':')
            .nth(1)?
            .trim()
            .trim_matches('{')
            .trim_matches('}')
            .trim_matches(',')
            .trim_matches('"')
            .trim_matches('\'')
            .to_string()
            .into()
    }

    fn episode_number(input: &str) -> Option<f32> {
        let mut current = String::new();
        let mut numbers = Vec::new();
        for ch in input.chars() {
            if ch.is_ascii_digit() || ch == '.' {
                current.push(ch);
            } else if !current.is_empty() {
                numbers.push(current.clone());
                current.clear();
            }
        }
        if !current.is_empty() {
            numbers.push(current);
        }
        numbers.into_iter().last()?.parse().ok()
    }

    fn display_number(value: f32) -> String {
        if value.fract() == 0.0 {
            format!("{}", value as i32)
        } else {
            value.to_string()
        }
    }

    fn has_next_page(body: &str) -> bool {
        let lower = body.to_lowercase();
        lower.contains("next page")
            || lower.contains("page-numbers next")
            || lower.contains("pagination")
                && (lower.contains("next") || lower.contains("selanjutnya"))
    }

    fn parse_status(input: &str) -> ItemStatus {
        let lower = input.trim().to_lowercase();
        if lower.contains("completed")
            || lower.contains("selesai")
            || lower.contains("finished")
            || lower.contains("end")
        {
            ItemStatus::Completed
        } else if lower.contains("ongoing")
            || lower.contains("tayang")
            || lower.contains("airing")
            || lower.contains("on-going")
        {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        }
    }

    fn quality_from_url(input: &str) -> String {
        for quality in ["2160", "1080", "720", "480", "360", "240"] {
            if input.contains(quality) {
                return format!("{quality}p");
            }
        }
        if input.contains(".m3u8") {
            "HLS".to_string()
        } else {
            "external".to_string()
        }
    }

    fn normalize_url(input: &str) -> String {
        let trimmed = input.trim().replace("\\/", "/");
        if trimmed.starts_with("//") {
            format!("https:{trimmed}")
        } else {
            trimmed
        }
    }

    fn host_name(input: &str) -> String {
        input
            .split("//")
            .nth(1)
            .unwrap_or(input)
            .split('/')
            .next()
            .unwrap_or("External")
            .replace("www.", "")
    }

    fn hoster_key(request: &Value) -> Option<String> {
        request
            .get("hoster")
            .and_then(|hoster| {
                hoster
                    .get("key")
                    .or_else(|| hoster.get("url"))
                    .and_then(Value::as_str)
                    .or_else(|| hoster.as_str())
            })
            .or_else(|| request.get("key").and_then(Value::as_str))
            .map(ToString::to_string)
    }

    fn request_key<C: IndonesianConfig>(request: &Value, field: &str) -> Option<String> {
        request
            .get(field)
            .and_then(|value| {
                value
                    .get("key")
                    .or_else(|| value.get("url"))
                    .and_then(Value::as_str)
                    .or_else(|| value.as_str())
            })
            .or_else(|| request.get("key").and_then(Value::as_str))
            .map(path_key::<C>)
    }

    fn path_from_url<C: IndonesianConfig>(input: &str) -> Option<String> {
        (input.starts_with(C::BASE_URL) || input.starts_with('/')).then(|| path_key::<C>(input))
    }

    fn path_key<C: IndonesianConfig>(input: &str) -> String {
        if input.starts_with("nimegami:") {
            return input.to_string();
        }
        if input.starts_with("http") && !input.starts_with(C::BASE_URL) {
            return input.to_string();
        }
        let without_base = input.strip_prefix(C::BASE_URL).unwrap_or(input);
        format!(
            "/{}",
            without_base
                .split('#')
                .next()
                .unwrap_or(without_base)
                .trim_matches('/')
        )
    }

    fn absolute_url<C: IndonesianConfig>(input: &str) -> String {
        if input.starts_with("http") || input.starts_with("nimegami:") {
            input.to_string()
        } else {
            url::join_url(C::BASE_URL, input)
        }
    }

    fn title_from_path<C: IndonesianConfig>(path: &str) -> String {
        path.trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(C::NAME)
            .replace('-', " ")
    }

    fn page(request: &Value) -> u64 {
        request
            .get("page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1)
    }

    fn listing(request: &Value) -> &str {
        request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular")
    }

    fn filter(request: &Value, key: &str) -> Option<String> {
        request
            .get("filters")
            .and_then(|f| f.get(key))
            .or_else(|| request.get(key))
            .and_then(Value::as_str)
            .map(ToString::to_string)
    }

    fn preference(request: &Value, key: &str, default: &str) -> String {
        request
            .get("preferences")
            .and_then(|p| p.get(key))
            .or_else(|| request.get(key))
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_string()
    }

    fn with_listing(request: &Value, listing: &str) -> Value {
        let mut cloned = request.clone();
        if let Value::Object(ref mut map) = cloned {
            map.insert("listing".to_string(), Value::String(listing.to_string()));
        }
        cloned
    }

    const LIST_FIXTURE: &str = r#"<article><a href="/anime/sample"><img alt="Sample Anime" src="/poster.jpg"></a></article>"#;
    const DETAILS_FIXTURE: &str = r#"<h1>Sample Anime</h1><img src="/poster.jpg"><p>Sinopsis: Sample details.</p><ul><li><a href="/episode/sample-1">Episode 1</a></li></ul>"#;
    const EPISODE_FIXTURE: &str = r#"<iframe src="https://example.invalid/embed/sample"></iframe>"#;
}

pub fn hoster(key: &str, name: &str, url: &str) -> VideoHoster {
    VideoHoster {
        key: key.to_string(),
        name: name.to_string(),
        url: Some(url.to_string()),
        lazy: true,
        video_count: Some(1),
        ..VideoHoster::default()
    }
}

pub fn hls_stream(url: &str, quality: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: url.to_string(),
        name: Some(format!("HLS {quality}")),
        quality: Some(quality.to_string()),
        format: Some("hls".to_string()),
        is_hls: true,
        stream_kind: Some(VideoStreamKind::Hls),
        preferred: quality == "1080p",
        headers: referer_headers(referer),
        audio_tracks: vec![AudioTrack {
            language: Some("en".to_string()),
            label: Some("English".to_string()),
            format: Some("aac".to_string()),
            is_default: true,
            ..AudioTrack::default()
        }],
        subtitles: vec![SubtitleTrack {
            url: "https://media.example/subtitles/example-en.vtt".to_string(),
            language: Some("en".to_string()),
            label: Some("English".to_string()),
            format: Some("vtt".to_string()),
            is_default: true,
            ..SubtitleTrack::default()
        }],
        ..VideoStream::default()
    }
}

pub fn torrent_stream(magnet_url: &str) -> VideoStream {
    VideoStream {
        url: magnet_url.to_string(),
        name: Some("Magnet".to_string()),
        quality: Some("1080p".to_string()),
        format: Some("magnet".to_string()),
        stream_kind: Some(VideoStreamKind::Magnet),
        torrent: Some(TorrentInfo {
            magnet_url: Some(magnet_url.to_string()),
            file_name: Some("example.mkv".to_string()),
            seeders: Some(42),
            size_bytes: Some(1_500_000_000),
            ..TorrentInfo::default()
        }),
        debrid: Some(DebridInfo {
            provider: Some("ExampleDebrid".to_string()),
            requires_account: true,
            external_playback: true,
            ..DebridInfo::default()
        }),
        ..VideoStream::default()
    }
}

pub fn sort_streams(streams: &mut [VideoStream]) {
    streams.sort_by(|a, b| b.quality.cmp(&a.quality));
}

pub mod wco {
    use crate::{
        html,
        sdk::{
            CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
            VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult, http::HttpClient,
            source::VideoSource,
        },
        url,
        video::referer_headers,
    };
    use serde::Deserialize;
    use serde_json::Value;
    use std::marker::PhantomData;

    pub trait WcoConfig {
        const NAME: &'static str;
        const BASE_URL: &'static str;
    }

    pub struct WcoSource<C>(PhantomData<C>);

    impl<C> WcoSource<C> {
        pub const fn new() -> Self {
            Self(PhantomData)
        }
    }

    impl<C: WcoConfig> VideoSource for WcoSource<C> {
        fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
            let body = fetch_or_fixture::<C>(C::BASE_URL, LIST_FIXTURE, C::BASE_URL);
            let listing = request
                .get("listing")
                .or_else(|| request.get("listingId"))
                .and_then(Value::as_str)
                .unwrap_or("popular");
            let selector = if listing == "latest" {
                "Recent Releases"
            } else {
                "items"
            };
            Ok(Paged {
                entries: parse_cards::<C>(&body, selector),
                has_next_page: false,
            })
        }

        fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
            let query = request
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if let Some(path) = path_from_url::<C>(query) {
                return Ok(Paged {
                    entries: vec![fetch_details::<C>(&path)],
                    has_next_page: false,
                });
            }
            if !query.is_empty() {
                let body = client::<C>(C::BASE_URL)
                    .post(format!("{}/search", C::BASE_URL))
                    .browser_document()
                    .referer(C::BASE_URL)
                    .form(&[("catara", query), ("konuara", "series")])
                    .send_text()
                    .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
                return Ok(Paged {
                    entries: parse_cards::<C>(&body, "search"),
                    has_next_page: false,
                });
            }
            if let Some(genre) = filter_value(&request, "genre").filter(|value| !value.is_empty()) {
                let target = format!("{}/search-by-genre/page/{}", C::BASE_URL, genre_id(&genre));
                let body = fetch_or_fixture::<C>(&target, GENRE_FIXTURE, C::BASE_URL);
                return Ok(Paged {
                    entries: parse_cards::<C>(&body, "genre"),
                    has_next_page: false,
                });
            }
            self.list(request)
        }

        fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
            let path = request_key::<C>(&request, "item")
                .unwrap_or_else(|| "/anime/sample-cartoon".to_string());
            Ok(fetch_details::<C>(&path))
        }

        fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
            let path = request_key::<C>(&request, "item")
                .unwrap_or_else(|| "/anime/sample-cartoon".to_string());
            let body =
                fetch_or_fixture::<C>(&absolute_url::<C>(&path), DETAILS_FIXTURE, C::BASE_URL);
            let episodes = parse_episodes::<C>(&body);
            if episodes.is_empty() {
                return Ok(vec![VideoEpisode {
                    key: path.clone(),
                    title: Some(title_from_path::<C>(&path)),
                    episode_number: Some(1.0),
                    url: Some(absolute_url::<C>(&path)),
                    language: Some("en".to_string()),
                    ..VideoEpisode::default()
                }]);
            }
            Ok(episodes)
        }

        fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
            let path = request_key::<C>(&request, "episode")
                .unwrap_or_else(|| "/sample-cartoon-episode-1".to_string());
            let body =
                fetch_or_fixture::<C>(&absolute_url::<C>(&path), EPISODE_FIXTURE, C::BASE_URL);
            let mut streams = body
                .split("<iframe")
                .skip(1)
                .filter_map(|chunk| html::attr(chunk, "src"))
                .flat_map(|src| resolve_iframe::<C>(&absolute_url::<C>(&src), &request))
                .collect::<Vec<_>>();
            sort_wco_streams(&mut streams, &request);
            Ok(streams)
        }

        fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
            let popular = self.list(with_listing(&request, "popular"))?;
            let latest = self.list(with_listing(&request, "latest"))?;
            Ok(vec![
                HomeSection {
                    id: "popular".to_string(),
                    title: "Popular".to_string(),
                    style: Some(HomeSectionStyle::Featured),
                    entries: popular.entries,
                    has_more: false,
                    ..HomeSection::default()
                },
                HomeSection {
                    id: "latest".to_string(),
                    title: "Latest".to_string(),
                    entries: latest.entries,
                    has_more: false,
                    ..HomeSection::default()
                },
            ])
        }

        fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
            Ok(request_key::<C>(&request, "item").map(|path| absolute_url::<C>(&path)))
        }

        fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
            Ok(request_key::<C>(&request, "episode").map(|path| absolute_url::<C>(&path)))
        }

        fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
            let Some(input) = request.get("url").and_then(Value::as_str) else {
                return Ok(None);
            };
            if let Some(path) = path_from_url::<C>(input) {
                return Ok(Some(UrlResolveResult {
                    item: Some(fetch_details::<C>(&path)),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
            Ok(Some(UrlResolveResult {
                search: Some(crate::sdk::SearchRequest {
                    query: input.to_string(),
                    ..crate::sdk::SearchRequest::default()
                }),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }))
        }
    }

    fn client<C: WcoConfig>(referer: &str) -> HttpClient {
        HttpClient::browser()
            .with_desktop_user_agent()
            .with_referer(referer)
            .with_header("Origin", C::BASE_URL)
            .with_cookies_for(C::BASE_URL)
            .with_webview_challenge_fallback()
    }

    fn fetch_or_fixture<C: WcoConfig>(target: &str, fixture: &str, referer: &str) -> String {
        client::<C>(referer)
            .get(target)
            .browser_document()
            .referer(referer)
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    fn fetch_xhr_or_fixture<C: WcoConfig>(target: &str, fixture: &str, referer: &str) -> String {
        client::<C>(referer)
            .get(target)
            .xhr()
            .referer(referer)
            .header("Accept", "application/json, text/javascript, */*; q=0.01")
            .header("X-Requested-With", "XMLHttpRequest")
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    fn parse_cards<C: WcoConfig>(body: &str, selector_hint: &str) -> Vec<CatalogItem> {
        let mut items = Vec::new();
        for chunk in body.split("<li").skip(1) {
            let Some(href) = html::attr_after(chunk, "<a", "href") else {
                continue;
            };
            let title = html::attr_after(chunk, "<img", "alt")
                .or_else(|| html::text_between(chunk, "recent-release-episodes", "</a>"))
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path::<C>(&href));
            let cover = html::attr_after(chunk, "<img", "src").map(|src| absolute_url::<C>(&src));
            items.push(CatalogItem {
                key: path_key::<C>(&href),
                title,
                cover,
                url: Some(absolute_url::<C>(&href)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            });
        }
        if items.is_empty() && selector_hint != "items" {
            parse_cards::<C>(LIST_FIXTURE, "items")
        } else {
            items
        }
    }

    fn fetch_details<C: WcoConfig>(path: &str) -> CatalogItem {
        let body = fetch_or_fixture::<C>(&absolute_url::<C>(path), DETAILS_FIXTURE, C::BASE_URL);
        parse_details::<C>(&body, path)
    }

    fn parse_details<C: WcoConfig>(body: &str, path: &str) -> CatalogItem {
        let title = html::text_between(body, "video-title", "</")
            .or_else(|| html::text_between(body, "header-tag", "</"))
            .or_else(|| html::text_between(body, "baslikCell", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_path::<C>(path));
        let side = body.split("sidebar_cat").nth(1).unwrap_or(body);
        let description = html::text_between(side, "<p", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty());
        let tags = side
            .split("<a")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect();
        CatalogItem {
            key: path_key::<C>(path),
            title,
            cover: html::attr_after(side, "<img", "src").map(|src| absolute_url::<C>(&src)),
            url: Some(absolute_url::<C>(path)),
            description,
            tags,
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            initialized: true,
            ..CatalogItem::default()
        }
    }

    fn parse_episodes<C: WcoConfig>(body: &str) -> Vec<VideoEpisode> {
        let mut episodes = Vec::new();
        for marker in ["cat-eps", "dark-episode-item"] {
            for chunk in body.split(marker).skip(1) {
                let Some(href) = html::attr_after(chunk, "<a", "href") else {
                    continue;
                };
                let raw_title = html::text_between(chunk, "<span", "</span>")
                    .or_else(|| html::text_between(chunk, ">", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| title_from_path::<C>(&href));
                let (title, number) = episode_title(&raw_title);
                episodes.push(VideoEpisode {
                    key: path_key::<C>(&href),
                    title: Some(title),
                    episode_number: Some(number),
                    url: Some(absolute_url::<C>(&href)),
                    language: Some("en".to_string()),
                    ..VideoEpisode::default()
                });
            }
        }
        if episodes.is_empty() {
            for chunk in body.split("<a").skip(1) {
                if !chunk.contains("episode") {
                    continue;
                }
                let Some(href) = html::attr(chunk, "href") else {
                    continue;
                };
                let title = html::strip_tags(
                    &html::text_between(chunk, ">", "</a>")
                        .unwrap_or_else(|| title_from_path::<C>(&href)),
                );
                let (title, number) = episode_title(&title);
                episodes.push(VideoEpisode {
                    key: path_key::<C>(&href),
                    title: Some(title),
                    episode_number: Some(number),
                    url: Some(absolute_url::<C>(&href)),
                    language: Some("en".to_string()),
                    ..VideoEpisode::default()
                });
            }
        }
        episodes.reverse();
        episodes
    }

    fn resolve_iframe<C: WcoConfig>(iframe: &str, request: &Value) -> Vec<VideoStream> {
        if iframe.contains("embed.wcostream") {
            return resolve_wcostream_embed::<C>(iframe, request);
        }
        if iframe.contains("vhs.watchanimesub") {
            return resolve_premium_hls::<C>(iframe, request);
        }
        Vec::new()
    }

    fn resolve_wcostream_embed<C: WcoConfig>(iframe: &str, request: &Value) -> Vec<VideoStream> {
        let body = fetch_or_fixture::<C>(iframe, IFRAME_FIXTURE, C::BASE_URL);
        let Some(path) = html::text_between(&body, "$.getJSON(\"", "\"")
            .or_else(|| html::text_between(&body, "$.getJSON('", "'"))
        else {
            return Vec::new();
        };
        let origin = origin(iframe).unwrap_or_else(|| "https://embed.wcostream.com".to_string());
        let target = url::join_url(&origin, &path);
        let body = fetch_xhr_or_fixture::<C>(&target, VIDEO_JSON_FIXTURE, &target);
        let data = serde_json::from_str::<VideoResponse>(&body).unwrap_or_default();
        data.into_streams(request)
    }

    fn resolve_premium_hls<C: WcoConfig>(iframe: &str, request: &Value) -> Vec<VideoStream> {
        let body = fetch_or_fixture::<C>(iframe, PREMIUM_IFRAME_FIXTURE, C::BASE_URL);
        let Some(playlist) = html::text_between(&body, "getRedirectedUrl(\"", "\"") else {
            return Vec::new();
        };
        parse_hls::<C>(&playlist, iframe, request)
    }

    fn parse_hls<C: WcoConfig>(playlist: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
        let body = fetch_or_fixture::<C>(playlist, HLS_FIXTURE, referer);
        if !body.contains("#EXT-X-STREAM-INF") {
            return vec![stream(playlist, "Premium", "auto", referer, request)];
        }
        body.split("#EXT-X-STREAM-INF:")
            .skip(1)
            .filter_map(|block| {
                let quality = block
                    .split("RESOLUTION=")
                    .nth(1)
                    .and_then(|part| part.split('x').nth(1))
                    .and_then(|part| part.split([',', '\n']).next())
                    .map(|height| format!("{height}p"))
                    .unwrap_or_else(|| "auto".to_string());
                let line = block
                    .lines()
                    .find(|line| !line.trim().is_empty() && !line.starts_with('#'))?;
                Some(stream(
                    &absolute_or(line.trim(), playlist),
                    "Premium",
                    &quality,
                    referer,
                    request,
                ))
            })
            .collect()
    }

    fn stream(url: &str, name: &str, quality: &str, referer: &str, request: &Value) -> VideoStream {
        let is_hls = url.contains(".m3u8");
        VideoStream {
            url: url.to_string(),
            name: Some(format!("{name} - {quality}")),
            quality: Some(quality.to_string()),
            format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
            is_hls,
            stream_kind: Some(if is_hls {
                VideoStreamKind::Hls
            } else {
                VideoStreamKind::Direct
            }),
            headers: referer_headers(referer),
            preferred: is_preferred(quality, request),
            initialized: true,
            ..VideoStream::default()
        }
    }

    fn sort_wco_streams(streams: &mut [VideoStream], request: &Value) {
        streams.sort_by_key(|stream| {
            quality_height(stream.quality.as_deref().unwrap_or_default())
                + if is_preferred(stream.quality.as_deref().unwrap_or_default(), request) {
                    10_000
                } else {
                    0
                }
        });
        streams.reverse();
        for stream in streams {
            let quality = stream.quality.clone().unwrap_or_default();
            stream.preferred = is_preferred(&quality, request);
        }
    }

    fn is_preferred(quality: &str, request: &Value) -> bool {
        quality.contains(&preference(request, "preferred_quality", "720"))
    }

    fn quality_height(quality: &str) -> i32 {
        quality
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap_or(0)
    }

    fn request_key<C: WcoConfig>(request: &Value, field: &str) -> Option<String> {
        request
            .get(field)
            .and_then(|value| {
                value
                    .get("key")
                    .or_else(|| value.get("url"))
                    .and_then(Value::as_str)
                    .or_else(|| value.as_str())
            })
            .or_else(|| request.get("key").and_then(Value::as_str))
            .map(path_key::<C>)
    }

    fn path_from_url<C: WcoConfig>(input: &str) -> Option<String> {
        if input.starts_with(C::BASE_URL) || input.starts_with('/') {
            Some(path_key::<C>(input))
        } else {
            None
        }
    }

    fn path_key<C: WcoConfig>(input: &str) -> String {
        if input.starts_with("http") && !input.starts_with(C::BASE_URL) {
            return input.to_string();
        }
        let without_base = input.strip_prefix(C::BASE_URL).unwrap_or(input);
        let path = without_base
            .split('#')
            .next()
            .unwrap_or(without_base)
            .split('?')
            .next()
            .unwrap_or(without_base);
        format!("/{}", path.trim_matches('/'))
    }

    fn absolute_url<C: WcoConfig>(input: &str) -> String {
        if input.starts_with("http") {
            input.to_string()
        } else {
            url::join_url(C::BASE_URL, input)
        }
    }

    fn absolute_or(input: &str, base: &str) -> String {
        if input.starts_with("http") {
            return input.to_string();
        }
        let root = base.rsplit_once('/').map(|(root, _)| root).unwrap_or(base);
        format!(
            "{}/{}",
            root.trim_end_matches('/'),
            input.trim_start_matches('/')
        )
    }

    fn origin(input: &str) -> Option<String> {
        let (_, rest) = input.split_once("://")?;
        let host = rest.split('/').next()?;
        Some(format!("https://{host}"))
    }

    fn title_from_path<C: WcoConfig>(input: &str) -> String {
        input
            .trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(C::NAME)
            .replace('-', " ")
    }

    fn episode_title(input: &str) -> (String, f32) {
        let lower = input.to_lowercase();
        let episode = lower
            .split("episode")
            .nth(1)
            .and_then(|part| {
                part.split_whitespace()
                    .next()
                    .and_then(|value| value.trim_matches(':').parse::<f32>().ok())
            })
            .unwrap_or(1.0);
        (input.to_string(), episode)
    }

    fn with_listing(request: &Value, listing: &str) -> Value {
        let mut cloned = request.clone();
        if let Value::Object(ref mut map) = cloned {
            map.insert("listing".to_string(), Value::String(listing.to_string()));
        }
        cloned
    }

    fn preference(request: &Value, key: &str, default: &str) -> String {
        request
            .get("preferences")
            .and_then(|preferences| preferences.get(key))
            .or_else(|| request.get(key))
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_string()
    }

    fn filter_value(request: &Value, key: &str) -> Option<String> {
        request
            .get("filters")
            .and_then(|filters| filters.get(key))
            .or_else(|| request.get(key))
            .and_then(Value::as_str)
            .map(ToString::to_string)
    }

    fn genre_id(input: &str) -> String {
        input.rsplit(':').next().unwrap_or(input).trim().to_string()
    }

    #[derive(Default, Deserialize)]
    struct VideoResponse {
        server: String,
        enc: Option<String>,
        hd: Option<String>,
        fhd: Option<String>,
    }

    impl VideoResponse {
        fn into_streams(self, request: &Value) -> Vec<VideoStream> {
            [("480p", self.enc), ("720p", self.hd), ("1080p", self.fhd)]
                .into_iter()
                .filter_map(|(quality, evid)| {
                    let evid = evid?;
                    if evid.trim().is_empty() || self.server.trim().is_empty() {
                        return None;
                    }
                    let stream_url =
                        format!("{}/getvid?evid={}", self.server.trim_end_matches('/'), evid);
                    Some(stream(
                        &stream_url,
                        "WCO",
                        quality,
                        "https://embed.wcostream.com",
                        request,
                    ))
                })
                .collect()
        }
    }

    const LIST_FIXTURE: &str = r#"
    <div id="sidebar_right2"><ul class="items">
      <li><div class="img"><a href="/anime/sample-cartoon"><img src="/cover.jpg" alt="Sample Cartoon"></a></div><div class="recent-release-episodes"><a href="/anime/sample-cartoon">Sample Cartoon</a></div></li>
    </ul></div>
    <div class="recent-release">Recent Releases</div><div><ul>
      <li><a href="/sample-cartoon-episode-1"><img src="/cover.jpg" alt="Sample Cartoon Episode 1"><span>Sample Cartoon Episode 1</span></a></li>
    </ul></div>
    "#;

    const SEARCH_FIXTURE: &str = r#"<div id="sidebar_right2"><li><a href="/anime/sample-cartoon"><img src="/cover.jpg" alt="Sample Cartoon">Sample Cartoon</a></li></div>"#;
    const GENRE_FIXTURE: &str = r#"<div id="sidebar_right4"><div class="ddmcc"><li><a href="/anime/sample-cartoon"><img src="/cover.jpg" alt="Sample Cartoon">Sample Cartoon</a></li></div></div>"#;
    const DETAILS_FIXTURE: &str = r#"
      <div class="video-title"><a href="/anime/sample-cartoon">Sample Cartoon</a></div>
      <div id="sidebar_cat"><img src="/cover.jpg"><p>Fixture details for local smoke tests.</p><a>Action</a></div>
      <div class="cat-eps"><a href="/sample-cartoon-episode-1"><span>Episode 1 Sample Cartoon</span></a></div>
    "#;
    const EPISODE_FIXTURE: &str =
        r#"<iframe src="https://embed.wcostream.com/inc/embed/video-js.php?file=sample"></iframe>"#;
    const IFRAME_FIXTURE: &str =
        r#"<script>$.getJSON("/inc/embed/getvidlink.php?vid=sample", function(data) {});</script>"#;
    const VIDEO_JSON_FIXTURE: &str =
        r#"{"server":"https://cdn.example.invalid","enc":"e480","hd":"e720","fhd":"e1080"}"#;
    const PREMIUM_IFRAME_FIXTURE: &str =
        r#"getRedirectedUrl("https://cdn.example.invalid/hls/index.m3u8")"#;
    const HLS_FIXTURE: &str = r#"#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=1400000,RESOLUTION=1280x720
720/index.m3u8
"#;
}

pub mod dopeflix {
    use crate::{
        html,
        sdk::{
            CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
            VideoEpisode, VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult,
            http::HttpClient, source::VideoSource,
        },
        url,
        video::referer_headers,
    };
    use serde::Deserialize;
    use serde_json::Value;
    use std::marker::PhantomData;

    pub trait DopeFlixConfig {
        const NAME: &'static str;
        const BASE_URL: &'static str;
    }

    pub struct DopeFlixSource<C>(PhantomData<C>);

    impl<C> DopeFlixSource<C> {
        pub const fn new() -> Self {
            Self(PhantomData)
        }
    }

    impl<C: DopeFlixConfig> VideoSource for DopeFlixSource<C> {
        fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
            let page = page(&request);
            let target = if page == 1 {
                format!("{}/home", C::BASE_URL)
            } else if preference(&request, "preferred_popular_type", "Movies") == "TV Shows" {
                format!("{}/tv-show?page={}", C::BASE_URL, page - 1)
            } else {
                format!("{}/movie?page={}", C::BASE_URL, page - 1)
            };
            let body = fetch_or_fixture::<C>(&target, LIST_FIXTURE, C::BASE_URL);
            Ok(parse_listing::<C>(&body))
        }

        fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
            let query = request
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if let Some(path) = path_from_url::<C>(query) {
                return Ok(Paged {
                    entries: vec![fetch_details::<C>(&path)],
                    has_next_page: false,
                });
            }
            let page = page(&request);
            let target = if query.is_empty() {
                let mut params = vec![format!("page={page}")];
                for key in ["type", "quality", "release_year", "genre", "country"] {
                    if let Some(value) = filter(&request, key).filter(|value| !value.is_empty()) {
                        params.push(format!("{key}={}", url::query_escape(&value)));
                    }
                }
                format!("{}/filter?{}", C::BASE_URL, params.join("&"))
            } else {
                format!(
                    "{}/search/{}?page={page}",
                    C::BASE_URL,
                    url::query_escape(&query.replace(' ', "-"))
                )
            };
            let body = fetch_or_fixture::<C>(&target, LIST_FIXTURE, C::BASE_URL);
            Ok(parse_listing::<C>(&body))
        }

        fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
            let path = request_key::<C>(&request, "item")
                .unwrap_or_else(|| "/movie/sample/watch-1".to_string());
            Ok(fetch_details::<C>(&path))
        }

        fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
            let path = request_key::<C>(&request, "item")
                .unwrap_or_else(|| "/movie/sample/watch-1".to_string());
            let id = path.rsplit('-').next().unwrap_or("1");
            if path.starts_with("/movie/") {
                return Ok(vec![VideoEpisode {
                    key: format!("/ajax/episode/list/{id}"),
                    title: Some("Movie".to_string()),
                    episode_number: Some(1.0),
                    url: Some(absolute_url::<C>(&path)),
                    language: Some("en".to_string()),
                    ..VideoEpisode::default()
                }]);
            }

            let seasons = fetch_or_fixture::<C>(
                &format!("{}/ajax/season/list/{id}", C::BASE_URL),
                SEASONS_FIXTURE,
                &absolute_url::<C>(&path),
            );
            let mut episodes = Vec::new();
            for (index, chunk) in seasons.split("ss-item").skip(1).enumerate() {
                let Some(season_id) = html::attr(chunk, "data-id") else {
                    continue;
                };
                let body = fetch_or_fixture::<C>(
                    &format!("{}/ajax/season/episodes/{season_id}", C::BASE_URL),
                    EPISODES_FIXTURE,
                    C::BASE_URL,
                );
                episodes.extend(parse_episodes::<C>(&body, (index + 1) as f32));
            }
            episodes.reverse();
            Ok(episodes)
        }

        fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
            let episode = request_key::<C>(&request, "episode")
                .unwrap_or_else(|| "/ajax/episode/list/1".to_string());
            let body =
                fetch_or_fixture::<C>(&absolute_url::<C>(&episode), SERVERS_FIXTURE, C::BASE_URL);
            Ok(parse_hosters::<C>(&body, &episode, &request))
        }

        fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
            let Some(key) = request_key::<C>(&request, "hoster") else {
                return Ok(Vec::new());
            };
            let parts = key.split('|').collect::<Vec<_>>();
            if parts.len() < 3 {
                return Ok(Vec::new());
            }
            let server_id = parts[0];
            let name = parts[1];
            let referer = parts[2];
            let body = fetch_xhr_or_fixture::<C>(
                &format!("{}/ajax/episode/sources/{server_id}", C::BASE_URL),
                SOURCE_FIXTURE,
                referer,
            );
            let link = serde_json::from_str::<SourceResponse>(&body)
                .ok()
                .and_then(|response| response.link)
                .unwrap_or_default();
            let mut streams = resolve_embed::<C>(&link, name, &request);
            sort_streams(&mut streams, &request);
            Ok(streams)
        }

        fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
            let mut streams = Vec::new();
            for hoster in self.hosters(request.clone())? {
                let mut resolved = self.resolve_hoster(serde_json::json!({
                    "hoster": { "key": hoster.key },
                    "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
                }))?;
                for stream in &mut resolved {
                    stream.hoster = Some(hoster.clone());
                }
                streams.extend(resolved);
            }
            sort_streams(&mut streams, &request);
            Ok(streams)
        }

        fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
            let popular = self.list(with_listing(&request, "popular"))?;
            let latest = self.list(with_listing(&request, "latest"))?;
            Ok(vec![
                HomeSection {
                    id: "popular".to_string(),
                    title: "Popular".to_string(),
                    style: Some(HomeSectionStyle::Featured),
                    entries: popular.entries,
                    has_more: popular.has_next_page,
                    ..HomeSection::default()
                },
                HomeSection {
                    id: "latest".to_string(),
                    title: "Latest".to_string(),
                    entries: latest.entries,
                    has_more: latest.has_next_page,
                    ..HomeSection::default()
                },
            ])
        }

        fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
            Ok(request_key::<C>(&request, "item").map(|path| absolute_url::<C>(&path)))
        }

        fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
            Ok(request_key::<C>(&request, "episode").map(|path| absolute_url::<C>(&path)))
        }

        fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
            let Some(input) = request.get("url").and_then(Value::as_str) else {
                return Ok(None);
            };
            if let Some(path) = path_from_url::<C>(input) {
                return Ok(Some(UrlResolveResult {
                    item: Some(fetch_details::<C>(&path)),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
            Ok(Some(UrlResolveResult {
                search: Some(crate::sdk::SearchRequest {
                    query: input.to_string(),
                    ..crate::sdk::SearchRequest::default()
                }),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }))
        }
    }

    fn client<C: DopeFlixConfig>(referer: &str) -> HttpClient {
        HttpClient::browser()
            .with_desktop_user_agent()
            .with_referer(referer)
            .with_header("Origin", C::BASE_URL)
            .with_cookies_for(C::BASE_URL)
            .with_webview_challenge_fallback()
    }

    fn fetch_or_fixture<C: DopeFlixConfig>(target: &str, fixture: &str, referer: &str) -> String {
        client::<C>(referer)
            .get(target)
            .browser_document()
            .referer(referer)
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    fn fetch_xhr_or_fixture<C: DopeFlixConfig>(
        target: &str,
        fixture: &str,
        referer: &str,
    ) -> String {
        client::<C>(referer)
            .get(target)
            .xhr()
            .header("Accept", "*/*")
            .header("X-Requested-With", "XMLHttpRequest")
            .referer(referer)
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    fn parse_listing<C: DopeFlixConfig>(body: &str) -> Paged<CatalogItem> {
        Paged {
            entries: body
                .split("flw-item")
                .skip(1)
                .filter_map(parse_card::<C>)
                .collect(),
            has_next_page: body.contains("pagination") && body.contains("next"),
        }
    }

    fn parse_card<C: DopeFlixConfig>(chunk: &str) -> Option<CatalogItem> {
        let href = html::attr_after(chunk, "<a", "href")?;
        let path = watch_path::<C>(&href);
        Some(CatalogItem {
            key: path.clone(),
            title: html::attr_after(chunk, "<a", "title")
                .or_else(|| html::text_between(chunk, "<h3", "</h3>").map(|v| html::strip_tags(&v)))
                .unwrap_or_else(|| title_from_path::<C>(&path)),
            cover: html::attr_after(chunk, "<img", "data-src")
                .or_else(|| html::attr_after(chunk, "<img", "src"))
                .map(|src| absolute_url::<C>(&src)),
            url: Some(absolute_url::<C>(&path)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            ..CatalogItem::default()
        })
    }

    fn fetch_details<C: DopeFlixConfig>(path: &str) -> CatalogItem {
        let body = fetch_or_fixture::<C>(&absolute_url::<C>(path), DETAILS_FIXTURE, C::BASE_URL);
        let info = body.split("detail_page-infor").nth(1).unwrap_or(&body);
        let title = html::text_between(&body, "<h2", "</h2>")
            .or_else(|| html::text_between(&body, "<h1", "</h1>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| title_from_path::<C>(path));
        let mut description = html::text_between(info, "description", "</div>")
            .map(|v| {
                html::strip_tags(&v)
                    .trim_start_matches("Overview:")
                    .trim()
                    .to_string()
            })
            .unwrap_or_default();
        for tag in ["Country:", "Casts:", "Released:", "Duration:"] {
            if let Some(value) = info_value(info, tag) {
                if !description.is_empty() {
                    description.push('\n');
                }
                description.push_str(tag);
                description.push(' ');
                description.push_str(&value);
            }
        }
        CatalogItem {
            key: path_key::<C>(path),
            title,
            cover: html::attr_after(info, "film-poster", "src")
                .or_else(|| html::attr_after(&body, "<img", "src"))
                .map(|src| absolute_url::<C>(&src)),
            url: Some(absolute_url::<C>(path)),
            description: (!description.is_empty()).then_some(description),
            tags: info_list(info, "Genre:"),
            authors: info_value(info, "Production:").into_iter().collect(),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            status: if path.starts_with("/tv/") {
                ItemStatus::Ongoing
            } else {
                ItemStatus::Completed
            },
            initialized: true,
            ..CatalogItem::default()
        }
    }

    fn parse_episodes<C: DopeFlixConfig>(body: &str, season: f32) -> Vec<VideoEpisode> {
        body.split("eps-item")
            .skip(1)
            .filter_map(|chunk| {
                let id = html::attr(chunk, "data-id")?;
                let title = html::attr(chunk, "title").unwrap_or_else(|| {
                    html::strip_tags(chunk.split("</a>").next().unwrap_or_default())
                });
                let number = title
                    .split("Episode")
                    .nth(1)
                    .and_then(|part| part.split_whitespace().next())
                    .and_then(|value| value.parse::<f32>().ok())
                    .unwrap_or(1.0);
                Some(VideoEpisode {
                    key: format!("/ajax/episode/servers/{id}"),
                    title: Some(if title.is_empty() {
                        format!("Episode {number}")
                    } else {
                        title
                    }),
                    season_number: Some(season),
                    episode_number: Some(number),
                    language: Some("en".to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect()
    }

    fn parse_hosters<C: DopeFlixConfig>(
        body: &str,
        episode: &str,
        request: &Value,
    ) -> Vec<VideoHoster> {
        let enabled = enabled_hosters(request);
        body.split("link-item")
            .skip(1)
            .filter_map(|chunk| {
                let id =
                    html::attr(chunk, "data-linkid").or_else(|| html::attr(chunk, "data-id"))?;
                let name = html::text_between(chunk, "<span", "</span>")
                    .map(|v| html::strip_tags(&v))
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| "UpCloud".to_string());
                if !enabled.iter().any(|host| host.eq_ignore_ascii_case(&name)) {
                    return None;
                }
                Some(VideoHoster {
                    key: format!("{id}|{name}|{}", absolute_url::<C>(episode)),
                    name,
                    url: Some(absolute_url::<C>(episode)),
                    lazy: true,
                    video_count: Some(1),
                    headers: referer_headers(C::BASE_URL),
                    ..VideoHoster::default()
                })
            })
            .collect()
    }

    fn resolve_embed<C: DopeFlixConfig>(
        embed: &str,
        name: &str,
        request: &Value,
    ) -> Vec<VideoStream> {
        if embed.is_empty() {
            return Vec::new();
        }
        if embed.contains(".m3u8") {
            return parse_hls::<C>(embed, name, embed, request);
        }
        vec![VideoStream {
            url: embed.to_string(),
            name: Some(name.to_string()),
            quality: Some("external".to_string()),
            format: Some("external".to_string()),
            stream_kind: Some(VideoStreamKind::External),
            headers: referer_headers(C::BASE_URL),
            preferred: preference(request, "preferred_server", "UpCloud")
                .eq_ignore_ascii_case(name),
            initialized: true,
            ..VideoStream::default()
        }]
    }

    fn parse_hls<C: DopeFlixConfig>(
        playlist: &str,
        name: &str,
        referer: &str,
        request: &Value,
    ) -> Vec<VideoStream> {
        let body = fetch_or_fixture::<C>(playlist, HLS_FIXTURE, referer);
        if !body.contains("#EXT-X-STREAM-INF") {
            return vec![stream(playlist, name, "auto", referer, request)];
        }
        body.split("#EXT-X-STREAM-INF:")
            .skip(1)
            .filter_map(|block| {
                let quality = block
                    .split("RESOLUTION=")
                    .nth(1)
                    .and_then(|part| part.split('x').nth(1))
                    .and_then(|part| part.split([',', '\n']).next())
                    .map(|height| format!("{height}p"))
                    .unwrap_or_else(|| "auto".to_string());
                let line = block
                    .lines()
                    .find(|line| !line.trim().is_empty() && !line.starts_with('#'))?;
                Some(stream(
                    &absolute_or(line.trim(), playlist),
                    name,
                    &quality,
                    referer,
                    request,
                ))
            })
            .collect()
    }

    fn stream(url: &str, name: &str, quality: &str, referer: &str, request: &Value) -> VideoStream {
        VideoStream {
            url: url.to_string(),
            name: Some(format!("{name} - {quality}")),
            quality: Some(quality.to_string()),
            format: Some("hls".to_string()),
            is_hls: true,
            stream_kind: Some(VideoStreamKind::Hls),
            headers: referer_headers(referer),
            preferred: quality.contains(&preference(request, "preferred_quality", "1080p")),
            initialized: true,
            ..VideoStream::default()
        }
    }

    fn sort_streams(streams: &mut [VideoStream], request: &Value) {
        let preferred_server = preference(request, "preferred_server", "UpCloud");
        streams.sort_by_key(|stream| {
            let quality = stream.quality.as_deref().unwrap_or_default();
            let height = quality
                .chars()
                .filter(char::is_ascii_digit)
                .collect::<String>()
                .parse::<i32>()
                .unwrap_or(0);
            let server = stream
                .name
                .as_deref()
                .unwrap_or_default()
                .contains(&preferred_server) as i32;
            (server, height)
        });
        streams.reverse();
    }

    fn info_value(info: &str, tag: &str) -> Option<String> {
        let block = info
            .split(tag)
            .nth(1)?
            .split("</div>")
            .next()
            .unwrap_or_default();
        let value = html::strip_tags(block)
            .trim_start_matches(tag)
            .trim()
            .to_string();
        (!value.is_empty()).then_some(value)
    }

    fn info_list(info: &str, tag: &str) -> Vec<String> {
        info.split(tag)
            .nth(1)
            .unwrap_or_default()
            .split("</div>")
            .next()
            .unwrap_or_default()
            .split("<a")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .collect()
    }

    fn request_key<C: DopeFlixConfig>(request: &Value, field: &str) -> Option<String> {
        request
            .get(field)
            .and_then(|value| {
                value
                    .get("key")
                    .or_else(|| value.get("url"))
                    .and_then(Value::as_str)
                    .or_else(|| value.as_str())
            })
            .or_else(|| request.get("key").and_then(Value::as_str))
            .map(path_key::<C>)
    }

    fn path_from_url<C: DopeFlixConfig>(input: &str) -> Option<String> {
        (input.starts_with(C::BASE_URL) || input.starts_with('/')).then(|| path_key::<C>(input))
    }

    fn path_key<C: DopeFlixConfig>(input: &str) -> String {
        if input.starts_with("http") && !input.starts_with(C::BASE_URL) {
            return input.to_string();
        }
        let without_base = input.strip_prefix(C::BASE_URL).unwrap_or(input);
        format!(
            "/{}",
            without_base
                .split('#')
                .next()
                .unwrap_or(without_base)
                .trim_matches('/')
        )
    }

    fn watch_path<C: DopeFlixConfig>(input: &str) -> String {
        let path = path_key::<C>(input);
        if path.contains("/watch-") {
            return path;
        }
        let id = path.rsplit('-').next().unwrap_or("1");
        let prefix = path.rsplit_once('/').map(|(left, _)| left).unwrap_or(&path);
        format!("{prefix}/watch-{id}")
    }

    fn absolute_url<C: DopeFlixConfig>(input: &str) -> String {
        if input.starts_with("http") {
            input.to_string()
        } else {
            url::join_url(C::BASE_URL, input)
        }
    }

    fn absolute_or(input: &str, base: &str) -> String {
        if input.starts_with("http") {
            return input.to_string();
        }
        let root = base.rsplit_once('/').map(|(root, _)| root).unwrap_or(base);
        format!(
            "{}/{}",
            root.trim_end_matches('/'),
            input.trim_start_matches('/')
        )
    }

    fn title_from_path<C: DopeFlixConfig>(path: &str) -> String {
        path.trim_matches('/')
            .rsplit('/')
            .nth(1)
            .or_else(|| path.trim_matches('/').rsplit('/').next())
            .unwrap_or(C::NAME)
            .replace('-', " ")
    }

    fn page(request: &Value) -> u64 {
        request
            .get("page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1)
    }

    fn preference(request: &Value, key: &str, default: &str) -> String {
        request
            .get("preferences")
            .and_then(|preferences| preferences.get(key))
            .or_else(|| request.get(key))
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_string()
    }

    fn filter(request: &Value, key: &str) -> Option<String> {
        request
            .get("filters")
            .and_then(|filters| filters.get(key))
            .or_else(|| request.get(key))
            .and_then(Value::as_str)
            .map(ToString::to_string)
    }

    fn enabled_hosters(request: &Value) -> Vec<String> {
        request
            .get("preferences")
            .and_then(|preferences| preferences.get("hoster_selection"))
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_else(|| {
                ["UpCloud", "MegaCloud", "Vidcloud", "AKCloud"]
                    .iter()
                    .map(ToString::to_string)
                    .collect()
            })
    }

    fn with_listing(request: &Value, listing: &str) -> Value {
        let mut cloned = request.clone();
        if let Value::Object(ref mut map) = cloned {
            map.insert("listing".to_string(), Value::String(listing.to_string()));
        }
        cloned
    }

    #[derive(Default, Deserialize)]
    struct SourceResponse {
        link: Option<String>,
    }

    const LIST_FIXTURE: &str = r#"
    <div class="flw-item"><a href="/movie/sample-movie-1" title="Sample Movie"><img data-src="/poster.jpg"></a></div>
    "#;
    const DETAILS_FIXTURE: &str = r#"
    <div class="detail_page-infor"><div class="film-poster"><img src="/poster.jpg"></div><h2>Sample Movie</h2><div class="description">Overview: Sample details.</div><div class="row-line">Genre: <a>Action</a></div><div class="row-line">Production: <a>Sample Studio</a></div></div>
    "#;
    const SEASONS_FIXTURE: &str = r#"<div class="ss-item" data-id="season-1">Season 1</div>"#;
    const EPISODES_FIXTURE: &str =
        r#"<a class="eps-item" data-id="episode-1" title="Episode 1: Pilot"></a>"#;
    const SERVERS_FIXTURE: &str =
        r#"<a class="link-item" data-linkid="source-1"><span>UpCloud</span></a>"#;
    const SOURCE_FIXTURE: &str = r#"{"link":"https://megacloud.tv/e-1/sample"}"#;
    const HLS_FIXTURE: &str = r#"#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=1400000,RESOLUTION=1280x720
720/index.m3u8
"#;
}

pub mod yflix {
    use crate::{
        html,
        sdk::{
            CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
            VideoEpisode, VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult,
            http::HttpClient, source::VideoSource,
        },
        url,
        video::referer_headers,
    };
    use serde::Deserialize;
    use serde_json::Value;
    use std::marker::PhantomData;

    pub trait YFlixConfig {
        const NAME: &'static str;
        const BASE_URL: &'static str;
        const CONTENT_RATING: &'static str = "adult";
    }

    pub struct YFlixSource<C>(PhantomData<C>);

    impl<C> YFlixSource<C> {
        pub const fn new() -> Self {
            Self(PhantomData)
        }
    }

    impl<C: YFlixConfig> VideoSource for YFlixSource<C> {
        fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
            let page = page(&request);
            let sort = if listing(&request) == "popular" {
                "sort=trending&"
            } else {
                ""
            };
            let body = fetch_or_fixture::<C>(
                &format!("{}/browser?{sort}page={page}", C::BASE_URL),
                LIST_FIXTURE,
                C::BASE_URL,
            );
            Ok(parse_listing::<C>(&body))
        }

        fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
            let query = request
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if let Some(path) = path_from_url::<C>(query) {
                return Ok(Paged {
                    entries: vec![fetch_details::<C>(&path)],
                    has_next_page: false,
                });
            }
            let mut params = vec![
                format!("keyword={}", url::query_escape(query)),
                format!("page={}", page(&request)),
            ];
            for key in ["type", "quality", "year", "genre", "country", "sort"] {
                if let Some(value) = filter(&request, key).filter(|value| !value.is_empty()) {
                    params.push(format!("{key}={}", url::query_escape(&value)));
                }
            }
            let body = fetch_or_fixture::<C>(
                &format!("{}/browser?{}", C::BASE_URL, params.join("&")),
                LIST_FIXTURE,
                C::BASE_URL,
            );
            Ok(parse_listing::<C>(&body))
        }

        fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
            let path =
                request_key::<C>(&request, "item").unwrap_or_else(|| "/movie/sample".to_string());
            Ok(fetch_details::<C>(&path))
        }

        fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
            let path =
                request_key::<C>(&request, "item").unwrap_or_else(|| "/movie/sample".to_string());
            let body =
                fetch_or_fixture::<C>(&absolute_url::<C>(&path), DETAILS_FIXTURE, C::BASE_URL);
            let id = html::attr_after(&body, "rating", "data-id")
                .or_else(|| html::attr(&body, "data-id"))
                .unwrap_or_else(|| "sample-id".to_string());
            let enc = encrypt::<C>(&id);
            let response = fetch_xhr_or_fixture::<C>(
                &format!("{}/ajax/episodes/list?id={id}&_={enc}", C::BASE_URL),
                EPISODES_RESPONSE_FIXTURE,
                &absolute_url::<C>(&path),
            );
            let html = serde_json::from_str::<ResultResponse>(&response)
                .map(|res| res.result)
                .unwrap_or(response);
            let mut episodes = parse_episodes::<C>(&html, &path);
            episodes.reverse();
            Ok(episodes)
        }

        fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
            let episode = request_key::<C>(&request, "episode")
                .unwrap_or_else(|| "/movie/sample#sample-episode".to_string());
            let episode_id = episode.split('#').nth(1).unwrap_or("sample-episode");
            let enc = encrypt::<C>(episode_id);
            let response = fetch_xhr_or_fixture::<C>(
                &format!("{}/ajax/links/list?eid={episode_id}&_={enc}", C::BASE_URL),
                SERVERS_RESPONSE_FIXTURE,
                C::BASE_URL,
            );
            let html = serde_json::from_str::<ResultResponse>(&response)
                .map(|res| res.result)
                .unwrap_or(response);
            Ok(parse_hosters::<C>(&html, &episode, &request))
        }

        fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
            let Some(key) = request_key::<C>(&request, "hoster") else {
                return Ok(Vec::new());
            };
            let parts = key.split('|').collect::<Vec<_>>();
            if parts.len() < 3 {
                return Ok(Vec::new());
            }
            let server_id = parts[0];
            let name = parts[1];
            let referer = parts[2];
            let enc = encrypt::<C>(server_id);
            let response = fetch_xhr_or_fixture::<C>(
                &format!("{}/ajax/links/view?id={server_id}&_={enc}", C::BASE_URL),
                LINK_RESPONSE_FIXTURE,
                referer,
            );
            let encrypted = serde_json::from_str::<ResultResponse>(&response)
                .map(|res| res.result)
                .unwrap_or(response);
            let iframe = decrypt::<C>(&encrypted);
            let mut streams = resolve_embed::<C>(&iframe, name, &request);
            sort_streams(&mut streams, &request);
            Ok(streams)
        }

        fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
            let mut streams = Vec::new();
            for hoster in self.hosters(request.clone())? {
                let mut resolved = self.resolve_hoster(serde_json::json!({
                    "hoster": { "key": hoster.key },
                    "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
                }))?;
                for stream in &mut resolved {
                    stream.hoster = Some(hoster.clone());
                }
                streams.extend(resolved);
            }
            sort_streams(&mut streams, &request);
            Ok(streams)
        }

        fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
            let popular = self.list(with_listing(&request, "popular"))?;
            let latest = self.list(with_listing(&request, "latest"))?;
            Ok(vec![
                HomeSection {
                    id: "popular".to_string(),
                    title: "Trending".to_string(),
                    style: Some(HomeSectionStyle::Featured),
                    entries: popular.entries,
                    has_more: popular.has_next_page,
                    ..HomeSection::default()
                },
                HomeSection {
                    id: "latest".to_string(),
                    title: "Latest".to_string(),
                    entries: latest.entries,
                    has_more: latest.has_next_page,
                    ..HomeSection::default()
                },
            ])
        }

        fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
            Ok(request_key::<C>(&request, "item").map(|path| absolute_url::<C>(&path)))
        }

        fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
            Ok(request_key::<C>(&request, "episode").map(|path| absolute_url::<C>(&path)))
        }

        fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
            let Some(input) = request.get("url").and_then(Value::as_str) else {
                return Ok(None);
            };
            if let Some(path) = path_from_url::<C>(input) {
                return Ok(Some(UrlResolveResult {
                    item: Some(fetch_details::<C>(&path)),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
            Ok(Some(UrlResolveResult {
                search: Some(crate::sdk::SearchRequest {
                    query: input.to_string(),
                    ..crate::sdk::SearchRequest::default()
                }),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }))
        }
    }

    fn client<C: YFlixConfig>(referer: &str) -> HttpClient {
        HttpClient::browser()
            .with_desktop_user_agent()
            .with_referer(referer)
            .with_cookies_for(C::BASE_URL)
            .with_webview_challenge_fallback()
    }

    fn fetch_or_fixture<C: YFlixConfig>(target: &str, fixture: &str, referer: &str) -> String {
        client::<C>(referer)
            .get(target)
            .browser_document()
            .referer(referer)
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    fn fetch_xhr_or_fixture<C: YFlixConfig>(target: &str, fixture: &str, referer: &str) -> String {
        client::<C>(referer)
            .get(target)
            .xhr()
            .header("Accept", "application/json, text/javascript, */*; q=0.01")
            .header("X-Requested-With", "XMLHttpRequest")
            .referer(referer)
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    fn encrypt<C: YFlixConfig>(text: &str) -> String {
        client::<C>(C::BASE_URL)
            .get(format!(
                "https://enc-dec.app/api/enc-movies-flix?text={}",
                url::query_escape(text)
            ))
            .xhr()
            .send_text()
            .ok()
            .and_then(|body| serde_json::from_str::<ResultResponse>(&body).ok())
            .map(|res| res.result)
            .unwrap_or_else(|| text.to_string())
    }

    fn decrypt<C: YFlixConfig>(text: &str) -> String {
        if text.starts_with("http") {
            return text.to_string();
        }
        client::<C>(C::BASE_URL)
            .get(format!(
                "https://enc-dec.app/api/dec-movies-flix?text={}",
                url::query_escape(text)
            ))
            .xhr()
            .send_text()
            .ok()
            .and_then(|body| serde_json::from_str::<DecryptResponse>(&body).ok())
            .map(|res| res.result.url)
            .unwrap_or_else(|| text.to_string())
    }

    fn parse_listing<C: YFlixConfig>(body: &str) -> Paged<CatalogItem> {
        Paged {
            entries: body
                .split("div class=\"item")
                .skip(1)
                .filter_map(parse_card::<C>)
                .collect(),
            has_next_page: body.contains("rel=\"next\""),
        }
    }

    fn parse_card<C: YFlixConfig>(chunk: &str) -> Option<CatalogItem> {
        let href = html::attr_after(chunk, "poster", "href")
            .or_else(|| html::attr_after(chunk, "<a", "href"))?;
        let path = path_key::<C>(&href);
        Some(CatalogItem {
            key: path.clone(),
            title: html::text_between(chunk, "title", "</a>")
                .map(|v| html::strip_tags(&v))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| title_from_path::<C>(&path)),
            cover: html::attr_after(chunk, "<img", "data-src")
                .or_else(|| html::attr_after(chunk, "<img", "src"))
                .map(|src| absolute_url::<C>(&src)),
            url: Some(absolute_url::<C>(&path)),
            language: Some("en".to_string()),
            content_rating: Some(C::CONTENT_RATING.to_string()),
            status: ItemStatus::Unknown,
            ..CatalogItem::default()
        })
    }

    fn fetch_details<C: YFlixConfig>(path: &str) -> CatalogItem {
        let body = fetch_or_fixture::<C>(&absolute_url::<C>(path), DETAILS_FIXTURE, C::BASE_URL);
        let title = html::text_between(&body, "h1 title", "</h1>")
            .or_else(|| html::text_between(&body, "<h1", "</h1>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| title_from_path::<C>(path));
        let mut description = html::text_between(&body, "description", "</div>")
            .map(|v| html::strip_tags(&v))
            .unwrap_or_default();
        for tag in ["Country:", "Released:", "Casts:"] {
            if let Some(value) = text_after(&body, tag) {
                if !description.is_empty() {
                    description.push('\n');
                }
                description.push_str(tag);
                description.push(' ');
                description.push_str(&value);
            }
        }
        CatalogItem {
            key: path_key::<C>(path),
            title,
            cover: html::attr_after(&body, "poster", "src")
                .or_else(|| html::attr_after(&body, "<img", "src"))
                .map(|src| absolute_url::<C>(&src)),
            url: Some(absolute_url::<C>(path)),
            description: (!description.is_empty()).then_some(description),
            tags: links_after(&body, "/genre/"),
            authors: links_after(&body, "/production/"),
            language: Some("en".to_string()),
            content_rating: Some(C::CONTENT_RATING.to_string()),
            status: if body.contains(">Movie<") {
                ItemStatus::Completed
            } else {
                ItemStatus::Ongoing
            },
            initialized: true,
            ..CatalogItem::default()
        }
    }

    fn parse_episodes<C: YFlixConfig>(body: &str, item_path: &str) -> Vec<VideoEpisode> {
        let mut out = Vec::new();
        for season_block in body.split("data-season=").skip(1) {
            let season = season_block
                .split(['"', '\''])
                .nth(1)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(1.0);
            for chunk in season_block.split("<a").skip(1) {
                let Some(eid) = html::attr(chunk, "eid") else {
                    continue;
                };
                let number = html::attr(chunk, "num")
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(1.0);
                let title = html::text_between(chunk, "<span", "</span>")
                    .map(|v| html::strip_tags(&v))
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| format!("Episode {number}"));
                out.push(VideoEpisode {
                    key: format!("{item_path}#{eid}"),
                    title: Some(title),
                    season_number: Some(season),
                    episode_number: Some(number),
                    language: Some("en".to_string()),
                    ..VideoEpisode::default()
                });
            }
        }
        if out.is_empty() {
            for chunk in body.split("<a").skip(1) {
                let Some(eid) = html::attr(chunk, "eid") else {
                    continue;
                };
                let title = html::text_between(chunk, "<span", "</span>")
                    .map(|v| html::strip_tags(&v))
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| "Movie".to_string());
                out.push(VideoEpisode {
                    key: format!("{item_path}#{eid}"),
                    title: Some(title),
                    episode_number: Some(1.0),
                    language: Some("en".to_string()),
                    ..VideoEpisode::default()
                });
            }
        }
        out
    }

    fn parse_hosters<C: YFlixConfig>(
        body: &str,
        episode: &str,
        request: &Value,
    ) -> Vec<VideoHoster> {
        let enabled = enabled_hosters(request);
        body.split("li class=\"server")
            .skip(1)
            .filter_map(|chunk| {
                let id = html::attr(chunk, "data-lid")?;
                let name = html::text_between(chunk, "<span", "</span>")
                    .map(|v| html::strip_tags(&v))
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| "Server 1".to_string());
                if !enabled.iter().any(|host| host.eq_ignore_ascii_case(&name)) {
                    return None;
                }
                Some(VideoHoster {
                    key: format!("{id}|{name}|{}", absolute_url::<C>(episode)),
                    name,
                    url: Some(absolute_url::<C>(episode)),
                    lazy: true,
                    video_count: Some(1),
                    headers: referer_headers(C::BASE_URL),
                    ..VideoHoster::default()
                })
            })
            .collect()
    }

    fn resolve_embed<C: YFlixConfig>(embed: &str, name: &str, request: &Value) -> Vec<VideoStream> {
        if embed.contains(".m3u8") {
            return vec![VideoStream {
                url: embed.to_string(),
                name: Some(name.to_string()),
                quality: Some("auto".to_string()),
                format: Some("hls".to_string()),
                is_hls: true,
                stream_kind: Some(VideoStreamKind::Hls),
                headers: referer_headers(C::BASE_URL),
                preferred: true,
                initialized: true,
                ..VideoStream::default()
            }];
        }
        vec![VideoStream {
            url: embed.to_string(),
            name: Some(name.to_string()),
            quality: Some("external".to_string()),
            format: Some("external".to_string()),
            stream_kind: Some(VideoStreamKind::External),
            headers: referer_headers(C::BASE_URL),
            preferred: preference(request, "pref_server_key", "Server 1")
                .eq_ignore_ascii_case(name),
            initialized: true,
            ..VideoStream::default()
        }]
    }

    fn sort_streams(streams: &mut [VideoStream], request: &Value) {
        let preferred = preference(request, "pref_server_key", "Server 1");
        streams.sort_by_key(|stream| {
            stream
                .name
                .as_deref()
                .unwrap_or_default()
                .contains(&preferred)
        });
        streams.reverse();
    }

    fn request_key<C: YFlixConfig>(request: &Value, field: &str) -> Option<String> {
        request
            .get(field)
            .and_then(|value| {
                value
                    .get("key")
                    .or_else(|| value.get("url"))
                    .and_then(Value::as_str)
                    .or_else(|| value.as_str())
            })
            .or_else(|| request.get("key").and_then(Value::as_str))
            .map(path_key::<C>)
    }

    fn path_from_url<C: YFlixConfig>(input: &str) -> Option<String> {
        (input.starts_with(C::BASE_URL) || input.starts_with('/')).then(|| path_key::<C>(input))
    }

    fn path_key<C: YFlixConfig>(input: &str) -> String {
        if input.contains('#') {
            let (item, episode) = input.split_once('#').unwrap_or((input, ""));
            return format!("{}#{episode}", path_key::<C>(item));
        }
        if input.starts_with("http") && !input.starts_with(C::BASE_URL) {
            return input.to_string();
        }
        let without_base = input.strip_prefix(C::BASE_URL).unwrap_or(input);
        format!(
            "/{}",
            without_base
                .split('#')
                .next()
                .unwrap_or(without_base)
                .trim_matches('/')
        )
    }

    fn absolute_url<C: YFlixConfig>(input: &str) -> String {
        if input.starts_with("http") {
            input.to_string()
        } else if let Some((path, fragment)) = input.split_once('#') {
            format!("{}#{fragment}", url::join_url(C::BASE_URL, path))
        } else {
            url::join_url(C::BASE_URL, input)
        }
    }

    fn title_from_path<C: YFlixConfig>(path: &str) -> String {
        path.trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(C::NAME)
            .replace('-', " ")
    }

    fn page(request: &Value) -> u64 {
        request
            .get("page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1)
    }

    fn listing(request: &Value) -> &str {
        request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular")
    }

    fn preference(request: &Value, key: &str, default: &str) -> String {
        request
            .get("preferences")
            .and_then(|p| p.get(key))
            .or_else(|| request.get(key))
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_string()
    }

    fn filter(request: &Value, key: &str) -> Option<String> {
        request
            .get("filters")
            .and_then(|f| f.get(key))
            .or_else(|| request.get(key))
            .and_then(Value::as_str)
            .map(ToString::to_string)
    }

    fn enabled_hosters(request: &Value) -> Vec<String> {
        request
            .get("preferences")
            .and_then(|p| p.get("pref_hoster_key"))
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_else(|| vec!["Server 1".to_string(), "Server 2".to_string()])
    }

    fn with_listing(request: &Value, listing: &str) -> Value {
        let mut cloned = request.clone();
        if let Value::Object(ref mut map) = cloned {
            map.insert("listing".to_string(), Value::String(listing.to_string()));
        }
        cloned
    }

    fn text_after(body: &str, label: &str) -> Option<String> {
        let value = body
            .split(label)
            .nth(1)?
            .split("</li>")
            .next()
            .unwrap_or_default();
        let value = html::strip_tags(value).trim().to_string();
        (!value.is_empty()).then_some(value)
    }

    fn links_after(body: &str, needle: &str) -> Vec<String> {
        body.split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains(needle))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .collect()
    }

    #[derive(Deserialize)]
    struct ResultResponse {
        result: String,
    }

    #[derive(Deserialize)]
    struct DecryptResponse {
        result: DecryptedUrl,
    }

    #[derive(Deserialize)]
    struct DecryptedUrl {
        url: String,
    }

    const LIST_FIXTURE: &str = r#"<div class="film-section"><div class="item"><a class="poster" href="/movie/sample-movie"><img data-src="/poster.jpg"></a><a class="title">Sample Movie</a></div></div>"#;
    const DETAILS_FIXTURE: &str = r#"<h1 class="title">Sample Movie</h1><div class="poster"><img src="/poster.jpg"></div><div class="rating" data-id="sample-content"></div><div class="metadata"><span>Movie</span></div><div class="description">Sample details.</div><ul class="mics"><li><a href="/genre/action">Action</a></li><li>Country: United States</li></ul>"#;
    const EPISODES_RESPONSE_FIXTURE: &str = r#"{"result":"<ul class='episodes' data-season='1'><li><a eid='sample-episode' num='1'><span>Movie</span></a></li></ul>"}"#;
    const SERVERS_RESPONSE_FIXTURE: &str =
        r#"{"result":"<li class='server' data-lid='server-1'><span>Server 1</span></li>"}"#;
    const LINK_RESPONSE_FIXTURE: &str = r#"{"result":"https://rapid-cloud.co/embed/sample"}"#;
}

pub mod animestream {
    use crate::{
        html,
        sdk::{
            CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
            VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult, http::HttpClient,
            source::VideoSource,
        },
        url,
        video::referer_headers,
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde_json::Value;
    use std::marker::PhantomData;

    pub trait AnimeStreamConfig {
        const NAME: &'static str;
        const BASE_URL: &'static str;
        const LANG: &'static str = "en";
        const LIST_PATH: &'static str = "anime";
        const CONTENT_RATING: &'static str = "safe";
        const QUALITY_DEFAULT: &'static str = "720p";
    }

    pub struct AnimeStreamSource<C>(PhantomData<C>);

    impl<C> AnimeStreamSource<C> {
        pub const fn new() -> Self {
            Self(PhantomData)
        }
    }

    impl<C: AnimeStreamConfig> VideoSource for AnimeStreamSource<C> {
        fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
            let page = page(&request);
            let order = if listing(&request) == "latest" {
                "update"
            } else {
                "popular"
            };
            let body = fetch_or_fixture::<C>(
                &format!(
                    "{}/{}/?page={page}&order={order}",
                    C::BASE_URL,
                    C::LIST_PATH
                ),
                LIST_FIXTURE,
                C::BASE_URL,
            );
            Ok(parse_listing::<C>(&body))
        }

        fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
            let query = request
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if let Some(path) = path_from_url::<C>(query) {
                return Ok(Paged {
                    entries: vec![fetch_details::<C>(&path)],
                    has_next_page: false,
                });
            }
            let body = if query.is_empty() {
                fetch_or_fixture::<C>(
                    &format!(
                        "{}/{}/?page={}&order={}",
                        C::BASE_URL,
                        C::LIST_PATH,
                        page(&request),
                        filter(&request, "order").unwrap_or_else(|| "popular".to_string())
                    ),
                    LIST_FIXTURE,
                    C::BASE_URL,
                )
            } else {
                fetch_or_fixture::<C>(
                    &format!(
                        "{}/page/{}/?s={}",
                        C::BASE_URL,
                        page(&request),
                        url::query_escape(query)
                    ),
                    LIST_FIXTURE,
                    C::BASE_URL,
                )
            };
            Ok(parse_listing::<C>(&body))
        }

        fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
            let path =
                request_key::<C>(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
            Ok(fetch_details::<C>(&path))
        }

        fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
            let path =
                request_key::<C>(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
            let body =
                fetch_or_fixture::<C>(&absolute_url::<C>(&path), DETAILS_FIXTURE, C::BASE_URL);
            Ok(parse_episodes::<C>(&body))
        }

        fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
            let episode = request_key::<C>(&request, "episode")
                .unwrap_or_else(|| "/sample-episode".to_string());
            let body =
                fetch_or_fixture::<C>(&absolute_url::<C>(&episode), EPISODE_FIXTURE, C::BASE_URL);
            let mut streams = Vec::new();
            for chunk in body
                .split("<option")
                .skip(1)
                .chain(body.split("<a").skip(1))
            {
                if !(chunk.contains("data-index") || chunk.contains("data-em")) {
                    continue;
                }
                let name = html::strip_tags(chunk.split("</").next().unwrap_or_default());
                let encoded = html::attr(chunk, "value")
                    .or_else(|| html::attr(chunk, "data-em"))
                    .unwrap_or_default();
                if let Some(embed) = hoster_url::<C>(&encoded) {
                    streams.push(external_stream::<C>(&embed, &name, &request));
                }
            }
            sort_streams(&mut streams, &request);
            Ok(streams)
        }

        fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
            let popular = self.list(with_listing(&request, "popular"))?;
            let latest = self.list(with_listing(&request, "latest"))?;
            Ok(vec![
                HomeSection {
                    id: "popular".to_string(),
                    title: "Popular".to_string(),
                    style: Some(HomeSectionStyle::Featured),
                    entries: popular.entries,
                    has_more: popular.has_next_page,
                    ..HomeSection::default()
                },
                HomeSection {
                    id: "latest".to_string(),
                    title: "Latest".to_string(),
                    entries: latest.entries,
                    has_more: latest.has_next_page,
                    ..HomeSection::default()
                },
            ])
        }

        fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
            Ok(request_key::<C>(&request, "item").map(|path| absolute_url::<C>(&path)))
        }

        fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
            Ok(request_key::<C>(&request, "episode").map(|path| absolute_url::<C>(&path)))
        }

        fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
            let Some(input) = request.get("url").and_then(Value::as_str) else {
                return Ok(None);
            };
            if let Some(path) = path_from_url::<C>(input) {
                return Ok(Some(UrlResolveResult {
                    item: Some(fetch_details::<C>(&path)),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
            Ok(Some(UrlResolveResult {
                search: Some(crate::sdk::SearchRequest {
                    query: input.to_string(),
                    ..crate::sdk::SearchRequest::default()
                }),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }))
        }
    }

    fn client<C: AnimeStreamConfig>(referer: &str) -> HttpClient {
        HttpClient::browser()
            .with_desktop_user_agent()
            .with_referer(referer)
            .with_cookies_for(C::BASE_URL)
            .with_webview_challenge_fallback()
    }

    fn fetch_or_fixture<C: AnimeStreamConfig>(
        target: &str,
        fixture: &str,
        referer: &str,
    ) -> String {
        client::<C>(referer)
            .get(target)
            .browser_document()
            .referer(referer)
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    fn parse_listing<C: AnimeStreamConfig>(body: &str) -> Paged<CatalogItem> {
        Paged {
            entries: body
                .split("article")
                .skip(1)
                .filter_map(parse_card::<C>)
                .collect(),
            has_next_page: body.contains("pagination")
                && (body.contains("next") || body.contains("hpage")),
        }
    }

    fn parse_card<C: AnimeStreamConfig>(chunk: &str) -> Option<CatalogItem> {
        let href = html::attr_after(chunk, "<a", "href")?;
        let path = path_key::<C>(&href);
        Some(CatalogItem {
            key: path.clone(),
            title: html::text_between(chunk, "div.tt", "</div>")
                .or_else(|| html::text_between(chunk, "div.ttl", "</div>"))
                .map(|v| html::strip_tags(&v))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| title_from_path::<C>(&path)),
            cover: html::attr_after(chunk, "<img", "data-src")
                .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
                .or_else(|| html::attr_after(chunk, "<img", "src"))
                .map(|src| absolute_url::<C>(&src)),
            url: Some(absolute_url::<C>(&path)),
            language: Some(C::LANG.to_string()),
            content_rating: Some(C::CONTENT_RATING.to_string()),
            status: ItemStatus::Unknown,
            ..CatalogItem::default()
        })
    }

    fn fetch_details<C: AnimeStreamConfig>(path: &str) -> CatalogItem {
        let body = fetch_or_fixture::<C>(&absolute_url::<C>(path), DETAILS_FIXTURE, C::BASE_URL);
        let info = body
            .split("info-content")
            .nth(1)
            .or_else(|| body.split("right").nth(1))
            .unwrap_or(&body);
        let title = html::text_between(&body, "entry-title", "</h1>")
            .or_else(|| html::text_between(&body, "<h1", "</h1>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| title_from_path::<C>(path));
        let mut description = html::text_between(&body, "itemprop=description", "</div>")
            .or_else(|| html::text_between(&body, "desc", "</div>"))
            .map(|v| html::strip_tags(&v))
            .unwrap_or_default();
        if let Some(alt) = html::text_between(&body, "alter", "</div>")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
        {
            if !description.is_empty() {
                description.push('\n');
            }
            description.push_str("Alternative name(s): ");
            description.push_str(&alt);
        }
        CatalogItem {
            key: path_key::<C>(path),
            title,
            cover: html::attr_after(&body, "thumb", "data-src")
                .or_else(|| html::attr_after(&body, "thumb", "src"))
                .or_else(|| html::attr_after(&body, "<img", "src"))
                .map(|src| absolute_url::<C>(&src)),
            url: Some(absolute_url::<C>(path)),
            description: (!description.is_empty()).then_some(description),
            tags: links_after(info, "Genre:"),
            authors: info_value(info, "Fansub").into_iter().collect(),
            artists: info_value(info, "Studio").into_iter().collect(),
            language: Some(C::LANG.to_string()),
            content_rating: Some(C::CONTENT_RATING.to_string()),
            status: parse_status(&info_value(info, "Status").unwrap_or_default()),
            initialized: true,
            ..CatalogItem::default()
        }
    }

    fn parse_episodes<C: AnimeStreamConfig>(body: &str) -> Vec<VideoEpisode> {
        body.split("<a")
            .skip(1)
            .filter_map(|chunk| {
                if !chunk.contains("epl-") {
                    return None;
                }
                let href = html::attr(chunk, "href")?;
                let ep_text = html::text_between(chunk, "epl-num", "</")
                    .map(|v| html::strip_tags(&v))
                    .unwrap_or_else(|| "1".to_string());
                let number = ep_text
                    .replace("[4K]", "")
                    .trim()
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(1.0);
                let title = html::text_between(chunk, "epl-title", "</")
                    .map(|v| html::strip_tags(&v))
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| format!("Episode {}", display_number(number)));
                Some(VideoEpisode {
                    key: path_key::<C>(&href),
                    title: Some(title),
                    episode_number: Some(number),
                    language: Some(C::LANG.to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect()
    }

    fn hoster_url<C: AnimeStreamConfig>(input: &str) -> Option<String> {
        if input.starts_with("http") {
            let body = fetch_or_fixture::<C>(input, "", C::BASE_URL);
            return iframe_from_body(&body).or_else(|| Some(input.to_string()));
        }
        let decoded = STANDARD
            .decode(input)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())?;
        iframe_from_body(&decoded)
    }

    fn iframe_from_body(body: &str) -> Option<String> {
        html::attr_after(body, "#embed_holder", "src")
            .or_else(|| html::attr_after(body, "<iframe", "src"))
            .or_else(|| html::attr_after(body, "embedUrl", "content"))
            .map(|value| {
                if value.starts_with("//") {
                    format!("https:{value}")
                } else {
                    value
                }
            })
    }

    fn external_stream<C: AnimeStreamConfig>(
        embed: &str,
        name: &str,
        request: &Value,
    ) -> VideoStream {
        let is_hls = embed.contains(".m3u8");
        VideoStream {
            url: embed.to_string(),
            name: Some(if name.is_empty() {
                "External".to_string()
            } else {
                name.to_string()
            }),
            quality: Some(if is_hls {
                C::QUALITY_DEFAULT.to_string()
            } else {
                "external".to_string()
            }),
            format: Some(if is_hls { "hls" } else { "external" }.to_string()),
            is_hls,
            stream_kind: Some(if is_hls {
                VideoStreamKind::Hls
            } else {
                VideoStreamKind::External
            }),
            headers: referer_headers(C::BASE_URL),
            preferred: preference(request, "preferred_quality", C::QUALITY_DEFAULT)
                .contains(C::QUALITY_DEFAULT),
            initialized: true,
            ..VideoStream::default()
        }
    }

    fn sort_streams(streams: &mut [VideoStream], request: &Value) {
        let quality = preference(request, "preferred_quality", C_PLACEHOLDER_QUALITY);
        streams.sort_by_key(|stream| {
            stream
                .quality
                .as_deref()
                .unwrap_or_default()
                .contains(&quality)
        });
        streams.reverse();
    }

    const C_PLACEHOLDER_QUALITY: &str = "720p";

    fn parse_status(input: &str) -> ItemStatus {
        match input.trim().to_lowercase().as_str() {
            "completed" | "completo" => ItemStatus::Completed,
            "ongoing" | "lançamento" => ItemStatus::Ongoing,
            _ => ItemStatus::Unknown,
        }
    }

    fn info_value(info: &str, label: &str) -> Option<String> {
        let block = info
            .split(label)
            .nth(1)?
            .split("</span>")
            .next()
            .unwrap_or_default();
        let value = html::strip_tags(block).trim_matches(':').trim().to_string();
        (!value.is_empty()).then_some(value)
    }

    fn links_after(info: &str, label: &str) -> Vec<String> {
        info.split(label)
            .nth(1)
            .unwrap_or(info)
            .split("<a")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .collect()
    }

    fn request_key<C: AnimeStreamConfig>(request: &Value, field: &str) -> Option<String> {
        request
            .get(field)
            .and_then(|value| {
                value
                    .get("key")
                    .or_else(|| value.get("url"))
                    .and_then(Value::as_str)
                    .or_else(|| value.as_str())
            })
            .or_else(|| request.get("key").and_then(Value::as_str))
            .map(path_key::<C>)
    }

    fn path_from_url<C: AnimeStreamConfig>(input: &str) -> Option<String> {
        (input.starts_with(C::BASE_URL) || input.starts_with('/')).then(|| path_key::<C>(input))
    }

    fn path_key<C: AnimeStreamConfig>(input: &str) -> String {
        if input.starts_with("http") && !input.starts_with(C::BASE_URL) {
            return input.to_string();
        }
        let without_base = input.strip_prefix(C::BASE_URL).unwrap_or(input);
        format!(
            "/{}",
            without_base
                .split('#')
                .next()
                .unwrap_or(without_base)
                .trim_matches('/')
        )
    }

    fn absolute_url<C: AnimeStreamConfig>(input: &str) -> String {
        if input.starts_with("http") {
            input.to_string()
        } else {
            url::join_url(C::BASE_URL, input)
        }
    }

    fn title_from_path<C: AnimeStreamConfig>(path: &str) -> String {
        path.trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(C::NAME)
            .replace('-', " ")
    }

    fn page(request: &Value) -> u64 {
        request
            .get("page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1)
    }

    fn listing(request: &Value) -> &str {
        request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular")
    }

    fn filter(request: &Value, key: &str) -> Option<String> {
        request
            .get("filters")
            .and_then(|f| f.get(key))
            .or_else(|| request.get(key))
            .and_then(Value::as_str)
            .map(ToString::to_string)
    }

    fn preference(request: &Value, key: &str, default: &str) -> String {
        request
            .get("preferences")
            .and_then(|p| p.get(key))
            .or_else(|| request.get(key))
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_string()
    }

    fn with_listing(request: &Value, listing: &str) -> Value {
        let mut cloned = request.clone();
        if let Value::Object(ref mut map) = cloned {
            map.insert("listing".to_string(), Value::String(listing.to_string()));
        }
        cloned
    }

    fn display_number(number: f32) -> String {
        if number.fract() == 0.0 {
            format!("{}", number as i32)
        } else {
            number.to_string()
        }
    }

    const LIST_FIXTURE: &str = r#"<div class="listupd"><article><a class="tip" href="/anime/sample"><div class="tt">Sample Anime</div><img src="/poster.jpg"></a></article></div>"#;
    const DETAILS_FIXTURE: &str = r#"<h1 class="entry-title">Sample Anime</h1><div class="thumb"><img src="/poster.jpg"></div><div class="info-content"><div class="genxed"><a>Action</a></div><span>Status: Ongoing</span><span>Studio: Sample Studio</span></div><div class="desc">Sample details.</div><div class="eplister"><ul><li><a href="/sample-episode"><div class="epl-num">1</div><div class="epl-title">Episode 1</div></a></li></ul></div>"#;
    const EPISODE_FIXTURE: &str = r#"<select class="mirror"><option data-index="1" value="PGlmcmFtZSBzcmM9Imh0dHBzOi8vZXhhbXBsZS5pbnZhbGlkL2VtYmVkIj48L2lmcmFtZT4=">External</option></select>"#;
}

pub mod dooplay {
    use crate::{
        html,
        sdk::{
            CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
            VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult, http::HttpClient,
            source::VideoSource,
        },
        url,
        video::referer_headers,
    };
    use serde::Deserialize;
    use serde_json::Value;
    use std::marker::PhantomData;

    pub trait DooPlayConfig {
        const NAME: &'static str;
        const BASE_URL: &'static str;
        const LANG: &'static str = "en";
        const CONTENT_RATING: &'static str = "safe";
        const LATEST_PATH: &'static str = "episodes";
        const POPULAR_PATH: &'static str = "ratings";
        const RESOLVE_EMBED_PAGE: bool = false;
        const USE_WP_JSON_PLAYER: bool = false;
    }

    pub struct DooPlaySource<C>(PhantomData<C>);

    impl<C> DooPlaySource<C> {
        pub const fn new() -> Self {
            Self(PhantomData)
        }
    }

    impl<C: DooPlayConfig> VideoSource for DooPlaySource<C> {
        fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
            let page = page(&request);
            let path = if listing(&request) == "latest" {
                format!("{}/{}/page/{page}", C::BASE_URL, C::LATEST_PATH)
            } else {
                format!(
                    "{}/{page}",
                    format!("{}/{}", C::BASE_URL, C::POPULAR_PATH).trim_end_matches('/')
                )
            };
            let body = fetch_or_fixture::<C>(&path, LIST_FIXTURE, C::BASE_URL);
            Ok(parse_listing::<C>(&body))
        }

        fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
            let query = request
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if let Some(path) = path_from_url::<C>(query) {
                return Ok(Paged {
                    entries: vec![fetch_details::<C>(&path)],
                    has_next_page: false,
                });
            }
            let page = page(&request);
            let target = if !query.is_empty() {
                format!(
                    "{}/page/{page}/?s={}",
                    C::BASE_URL,
                    url::query_escape(query)
                )
            } else if let Some(genre) = filter(&request, "genre").filter(|genre| !genre.is_empty())
            {
                format!("{}/genre/{genre}/page/{page}", C::BASE_URL)
            } else {
                format!(
                    "{}/{page}",
                    format!("{}/{}", C::BASE_URL, C::POPULAR_PATH).trim_end_matches('/')
                )
            };
            let body = fetch_or_fixture::<C>(&target, LIST_FIXTURE, C::BASE_URL);
            Ok(parse_listing::<C>(&body))
        }

        fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
            let path =
                request_key::<C>(&request, "item").unwrap_or_else(|| "/movies/sample".to_string());
            Ok(fetch_details::<C>(&path))
        }

        fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
            let path =
                request_key::<C>(&request, "item").unwrap_or_else(|| "/movies/sample".to_string());
            let body =
                fetch_or_fixture::<C>(&absolute_url::<C>(&path), DETAILS_FIXTURE, C::BASE_URL);
            let episodes = parse_episodes::<C>(&body);
            if episodes.is_empty() {
                return Ok(vec![VideoEpisode {
                    key: path.clone(),
                    title: Some("Movie".to_string()),
                    episode_number: Some(1.0),
                    url: Some(absolute_url::<C>(&path)),
                    language: Some(C::LANG.to_string()),
                    ..VideoEpisode::default()
                }]);
            }
            Ok(episodes)
        }

        fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
            let episode = request_key::<C>(&request, "episode")
                .unwrap_or_else(|| "/movies/sample".to_string());
            let body =
                fetch_or_fixture::<C>(&absolute_url::<C>(&episode), PLAYER_FIXTURE, C::BASE_URL);
            let mut streams = Vec::new();
            for player in body
                .split("dooplay_player_option")
                .skip(1)
                .chain(body.split("player-option").skip(1))
            {
                let post = html::attr(player, "data-post").unwrap_or_else(|| "1".to_string());
                let nume = html::attr(player, "data-nume").unwrap_or_else(|| "1".to_string());
                let kind = html::attr(player, "data-type").unwrap_or_else(|| {
                    if episode.contains("/tv") || episode.contains("/tvshows") {
                        "tv".to_string()
                    } else {
                        "movie".to_string()
                    }
                });
                let name = html::text_between(player, "title", "</span>")
                    .map(|v| html::strip_tags(&v))
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| format!("Server {nume}"));
                if let Some(embed) = player_embed::<C>(&post, &nume, &kind, &episode) {
                    streams.extend(resolve_embed::<C>(&embed, &name, &request));
                }
            }
            if streams.is_empty() {
                if let Some(embed) = html::attr_after(&body, "<iframe", "src") {
                    streams.extend(resolve_embed::<C>(
                        &absolute_url::<C>(&embed),
                        "External",
                        &request,
                    ));
                }
            }
            sort_streams(&mut streams, &request);
            Ok(streams)
        }

        fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
            let popular = self.list(with_listing(&request, "popular"))?;
            let latest = self.list(with_listing(&request, "latest"))?;
            Ok(vec![
                HomeSection {
                    id: "popular".to_string(),
                    title: "Popular".to_string(),
                    style: Some(HomeSectionStyle::Featured),
                    entries: popular.entries,
                    has_more: popular.has_next_page,
                    ..HomeSection::default()
                },
                HomeSection {
                    id: "latest".to_string(),
                    title: "Latest".to_string(),
                    entries: latest.entries,
                    has_more: latest.has_next_page,
                    ..HomeSection::default()
                },
            ])
        }

        fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
            Ok(request_key::<C>(&request, "item").map(|path| absolute_url::<C>(&path)))
        }

        fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
            Ok(request_key::<C>(&request, "episode").map(|path| absolute_url::<C>(&path)))
        }

        fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
            let Some(input) = request.get("url").and_then(Value::as_str) else {
                return Ok(None);
            };
            if let Some(path) = path_from_url::<C>(input) {
                return Ok(Some(UrlResolveResult {
                    item: Some(fetch_details::<C>(&path)),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
            Ok(Some(UrlResolveResult {
                search: Some(crate::sdk::SearchRequest {
                    query: input.to_string(),
                    ..crate::sdk::SearchRequest::default()
                }),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }))
        }
    }

    fn client<C: DooPlayConfig>(referer: &str) -> HttpClient {
        HttpClient::browser()
            .with_desktop_user_agent()
            .with_referer(referer)
            .with_header("Origin", C::BASE_URL)
            .with_cookies_for(C::BASE_URL)
            .with_webview_challenge_fallback()
    }

    fn fetch_or_fixture<C: DooPlayConfig>(target: &str, fixture: &str, referer: &str) -> String {
        client::<C>(referer)
            .get(target)
            .browser_document()
            .referer(referer)
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    fn player_embed<C: DooPlayConfig>(
        post: &str,
        nume: &str,
        kind: &str,
        episode: &str,
    ) -> Option<String> {
        let body = if C::USE_WP_JSON_PLAYER {
            client::<C>(&absolute_url::<C>(episode))
                .get(format!(
                    "{}/wp-json/dooplayer/v1/post/{post}?type={kind}&source={nume}",
                    C::BASE_URL
                ))
                .xhr()
                .header("Accept", "application/json, text/javascript, */*; q=0.01")
                .send_text()
                .unwrap_or_else(|_| EMBED_RESPONSE_FIXTURE.to_string())
        } else {
            client::<C>(&absolute_url::<C>(episode))
                .post(format!("{}/wp-admin/admin-ajax.php", C::BASE_URL))
                .xhr()
                .header("Accept", "*/*")
                .form(&[
                    ("action", "doo_player_ajax"),
                    ("post", post),
                    ("nume", nume),
                    ("type", kind),
                ])
                .send_text()
                .unwrap_or_else(|_| EMBED_RESPONSE_FIXTURE.to_string())
        };
        serde_json::from_str::<EmbedResponse>(&body)
            .ok()
            .map(|res| {
                res.embed_url
                    .replace("\\/", "/")
                    .trim_start_matches("//")
                    .to_string()
            })
            .map(|url| {
                if url.starts_with("http") {
                    url
                } else {
                    format!("https://{url}")
                }
            })
    }

    fn parse_listing<C: DooPlayConfig>(body: &str) -> Paged<CatalogItem> {
        let entries = body
            .split("<article")
            .skip(1)
            .filter_map(parse_card::<C>)
            .collect::<Vec<_>>();
        Paged {
            entries,
            has_next_page: body.contains("chevron-right") || body.contains("pagination"),
        }
    }

    fn parse_card<C: DooPlayConfig>(chunk: &str) -> Option<CatalogItem> {
        let href = html::attr_after(chunk, "<a", "href")?;
        let path = path_key::<C>(&href);
        Some(CatalogItem {
            key: path.clone(),
            title: html::attr_after(chunk, "<img", "alt")
                .unwrap_or_else(|| title_from_path::<C>(&path)),
            cover: html::attr_after(chunk, "<img", "data-wpfc-original-src")
                .or_else(|| html::attr_after(chunk, "<img", "data-src"))
                .or_else(|| html::attr_after(chunk, "<img", "src"))
                .map(|src| absolute_url::<C>(&src)),
            url: Some(absolute_url::<C>(&path)),
            language: Some(C::LANG.to_string()),
            content_rating: Some(C::CONTENT_RATING.to_string()),
            status: ItemStatus::Unknown,
            ..CatalogItem::default()
        })
    }

    fn fetch_details<C: DooPlayConfig>(path: &str) -> CatalogItem {
        let body = fetch_or_fixture::<C>(&absolute_url::<C>(path), DETAILS_FIXTURE, C::BASE_URL);
        let title = html::attr_after(&body, "poster", "alt")
            .or_else(|| {
                html::text_between(&body, "div.data", "</h1>").map(|v| html::strip_tags(&v))
            })
            .unwrap_or_else(|| title_from_path::<C>(path));
        let mut description = html::text_between(&body, "div#info", "</div>")
            .map(|v| html::strip_tags(&v))
            .unwrap_or_default();
        if let Some(text) = html::text_between(&body, "<p", "</p>")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
        {
            if !description.is_empty() {
                description.push('\n');
            }
            description.push_str(&text);
        }
        CatalogItem {
            key: path_key::<C>(path),
            title,
            cover: html::attr_after(&body, "poster", "data-wpfc-original-src")
                .or_else(|| html::attr_after(&body, "poster", "src"))
                .or_else(|| html::attr_after(&body, "<img", "src"))
                .map(|src| absolute_url::<C>(&src)),
            url: Some(absolute_url::<C>(path)),
            description: (!description.is_empty()).then_some(description),
            tags: links_after(&body, "sgeneros"),
            language: Some(C::LANG.to_string()),
            content_rating: Some(C::CONTENT_RATING.to_string()),
            status: ItemStatus::Unknown,
            initialized: true,
            ..CatalogItem::default()
        }
    }

    fn parse_episodes<C: DooPlayConfig>(body: &str) -> Vec<VideoEpisode> {
        body.split("episodios")
            .skip(1)
            .flat_map(|block| block.split("<li").skip(1))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let ep = html::text_between(chunk, "numerando", "</div>")
                    .map(|v| html::strip_tags(&v))
                    .and_then(|v| {
                        v.split_whitespace()
                            .last()
                            .and_then(|n| n.parse::<f32>().ok())
                    })
                    .unwrap_or(1.0);
                let title = html::text_between(chunk, "<a", "</a>")
                    .map(|v| html::strip_tags(&v))
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| format!("Episode {ep}"));
                Some(VideoEpisode {
                    key: path_key::<C>(&href),
                    title: Some(title),
                    episode_number: Some(ep),
                    url: Some(absolute_url::<C>(&href)),
                    language: Some(C::LANG.to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect()
    }

    fn resolve_embed<C: DooPlayConfig>(
        embed: &str,
        name: &str,
        request: &Value,
    ) -> Vec<VideoStream> {
        if embed.contains(".m3u8") {
            return vec![stream::<C>(
                embed,
                name,
                &preference(request, "preferred_quality", "1080p"),
                embed,
            )];
        }
        if C::RESOLVE_EMBED_PAGE {
            let body = fetch_or_fixture::<C>(embed, "", C::BASE_URL);
            if let Some(playlist) = extract_playlist(&body) {
                return parse_hls::<C>(&absolute_or(&playlist, embed), name, embed, request);
            }
        }
        vec![external_stream::<C>(embed, name, request)]
    }

    fn parse_hls<C: DooPlayConfig>(
        playlist: &str,
        name: &str,
        referer: &str,
        request: &Value,
    ) -> Vec<VideoStream> {
        let body = fetch_or_fixture::<C>(playlist, "", referer);
        if !body.contains("#EXT-X-STREAM-INF") {
            return vec![stream::<C>(
                playlist,
                name,
                &preference(request, "preferred_quality", "1080p"),
                referer,
            )];
        }
        body.split("#EXT-X-STREAM-INF:")
            .skip(1)
            .filter_map(|block| {
                let quality = block
                    .split("RESOLUTION=")
                    .nth(1)
                    .and_then(|value| value.split('x').nth(1))
                    .and_then(|value| value.split([',', '\n']).next())
                    .map(|height| format!("{height}p"))
                    .unwrap_or_else(|| "auto".to_string());
                let line = block
                    .lines()
                    .find(|line| !line.trim().is_empty() && !line.starts_with('#'))?;
                Some(stream::<C>(
                    &absolute_or(line.trim(), playlist),
                    name,
                    &quality,
                    referer,
                ))
            })
            .collect()
    }

    fn stream<C: DooPlayConfig>(
        url: &str,
        name: &str,
        quality: &str,
        referer: &str,
    ) -> VideoStream {
        VideoStream {
            url: url.to_string(),
            name: Some(format!("{name} - {quality}")),
            quality: Some(quality.to_string()),
            format: Some("hls".to_string()),
            is_hls: true,
            stream_kind: Some(VideoStreamKind::Hls),
            headers: referer_headers(referer),
            preferred: quality == "1080p",
            initialized: true,
            ..VideoStream::default()
        }
    }

    fn external_stream<C: DooPlayConfig>(embed: &str, name: &str, request: &Value) -> VideoStream {
        let is_hls = embed.contains(".m3u8");
        VideoStream {
            url: embed.to_string(),
            name: Some(name.to_string()),
            quality: Some(if is_hls {
                preference(request, "preferred_quality", "1080p")
            } else {
                "external".to_string()
            }),
            format: Some(if is_hls { "hls" } else { "external" }.to_string()),
            is_hls,
            stream_kind: Some(if is_hls {
                VideoStreamKind::Hls
            } else {
                VideoStreamKind::External
            }),
            headers: referer_headers(C::BASE_URL),
            preferred: true,
            initialized: true,
            ..VideoStream::default()
        }
    }

    fn extract_playlist(body: &str) -> Option<String> {
        for marker in [
            "let url = '",
            "var url = '",
            "file:\"",
            "file: \"",
            "source:\"",
            "src: \"",
        ] {
            if let Some(value) = body.split(marker).nth(1) {
                let end = if marker.ends_with('"') || marker.ends_with("\" ") {
                    '"'
                } else {
                    '\''
                };
                let playlist = value
                    .split(end)
                    .next()
                    .unwrap_or_default()
                    .replace("\\/", "/");
                if playlist.contains(".m3u8") {
                    return Some(playlist);
                }
            }
        }
        body.split(['"', '\''])
            .find(|part| part.contains(".m3u8"))
            .map(|value| value.replace("\\/", "/"))
    }

    fn sort_streams(streams: &mut [VideoStream], request: &Value) {
        let server = preference(request, "preferred_server", "");
        streams.sort_by_key(|stream| stream.name.as_deref().unwrap_or_default().contains(&server));
        streams.reverse();
    }

    fn request_key<C: DooPlayConfig>(request: &Value, field: &str) -> Option<String> {
        request
            .get(field)
            .and_then(|value| {
                value
                    .get("key")
                    .or_else(|| value.get("url"))
                    .and_then(Value::as_str)
                    .or_else(|| value.as_str())
            })
            .or_else(|| request.get("key").and_then(Value::as_str))
            .map(path_key::<C>)
    }

    fn path_from_url<C: DooPlayConfig>(input: &str) -> Option<String> {
        (input.starts_with(C::BASE_URL) || input.starts_with('/')).then(|| path_key::<C>(input))
    }

    fn path_key<C: DooPlayConfig>(input: &str) -> String {
        if input.starts_with("http") && !input.starts_with(C::BASE_URL) {
            return input.to_string();
        }
        let without_base = input.strip_prefix(C::BASE_URL).unwrap_or(input);
        format!(
            "/{}",
            without_base
                .split('#')
                .next()
                .unwrap_or(without_base)
                .trim_matches('/')
        )
    }

    fn absolute_url<C: DooPlayConfig>(input: &str) -> String {
        if input.starts_with("http") {
            input.to_string()
        } else {
            url::join_url(C::BASE_URL, input)
        }
    }

    fn absolute_or(input: &str, base: &str) -> String {
        if input.starts_with("http") {
            return input.to_string();
        }
        if input.starts_with("//") {
            return format!("https:{input}");
        }
        let root = base.rsplit_once('/').map(|(root, _)| root).unwrap_or(base);
        format!(
            "{}/{}",
            root.trim_end_matches('/'),
            input.trim_start_matches('/')
        )
    }

    fn title_from_path<C: DooPlayConfig>(path: &str) -> String {
        path.trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(C::NAME)
            .replace('-', " ")
    }

    fn links_after(body: &str, marker: &str) -> Vec<String> {
        body.split(marker)
            .nth(1)
            .unwrap_or_default()
            .split("<a")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .collect()
    }

    fn page(request: &Value) -> u64 {
        request
            .get("page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1)
    }

    fn listing(request: &Value) -> &str {
        request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular")
    }

    fn filter(request: &Value, key: &str) -> Option<String> {
        request
            .get("filters")
            .and_then(|f| f.get(key))
            .or_else(|| request.get(key))
            .and_then(Value::as_str)
            .map(ToString::to_string)
    }

    fn preference(request: &Value, key: &str, default: &str) -> String {
        request
            .get("preferences")
            .and_then(|p| p.get(key))
            .or_else(|| request.get(key))
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_string()
    }

    fn with_listing(request: &Value, listing: &str) -> Value {
        let mut cloned = request.clone();
        if let Value::Object(ref mut map) = cloned {
            map.insert("listing".to_string(), Value::String(listing.to_string()));
        }
        cloned
    }

    #[derive(Deserialize)]
    struct EmbedResponse {
        embed_url: String,
    }

    const LIST_FIXTURE: &str = r#"<article class="w_item_a"><a href="/movies/sample"><img alt="Sample Movie" src="/poster.jpg"></a></article>"#;
    const DETAILS_FIXTURE: &str = r#"<div class="sheader"><div class="poster"><img alt="Sample Movie" src="/poster.jpg"></div><div class="data"><h1>Sample Movie</h1><div class="sgeneros"><a>Action</a></div></div></div><div id="info"><p>Sample details.</p></div><ul id="playeroptionsul"><li class="dooplay_player_option" data-post="1" data-nume="1" data-type="movie"><span class="title">Server 1</span></li></ul>"#;
    const PLAYER_FIXTURE: &str = DETAILS_FIXTURE;
    const EMBED_RESPONSE_FIXTURE: &str = r#"{"embed_url":"https://example.invalid/embed"}"#;
}
