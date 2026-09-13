use base64::Engine;
use manatan_sdk::{
    browser::{
        self, WebViewExtractRequest, WebViewExtractResponse, WebViewRequestCapture, WebViewSession,
        WebViewSessionPersistence, WebViewWaitUntil,
    },
    client::Client,
    CatalogItem, Error, FilterDefinition, ImageRequest, MediaResourceKind, MediaTrack, OptionItem,
    Paged, ProcessedMedia, Result, SegmentProcessing, SegmentRule, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoSource, VideoStream,
};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use url::Url;

const BASE_URL: &str = "https://reanime.to";
const PAGE_SIZE: u32 = 36;
const EPISODE_PAGE_SIZE: u32 = 500;

#[derive(Default)]
pub struct ReAnime;

impl ReAnime {
    fn api<T: DeserializeOwned>(&self, path: &str, parameters: &[(&str, String)]) -> Result<T> {
        let mut url = Url::parse(&format!("{BASE_URL}{path}")).map_err(url_error)?;
        {
            let mut query = url.query_pairs_mut();
            for (key, value) in parameters {
                query.append_pair(key, value);
            }
        }
        validate_api_url(url.as_str())?;
        Client::browser()
            .get(url.as_str())
            .header("Accept", "application/json")
            .header("Referer", format!("{BASE_URL}/home"))
            .rate_limit("reanime-api", 150)
            .timeout_ms(30_000)
            .max_body_bytes(16 * 1024 * 1024)
            .send()?
            .error_for_status()?
            .json()
    }

    fn top(&self) -> Result<Paged<CatalogItem>> {
        let response: TopResponse = self.api(
            "/api/v1/top/anime",
            &[
                ("period", "month".to_string()),
                ("limit", PAGE_SIZE.to_string()),
            ],
        )?;
        Ok(Paged::new(safe_catalog(response.data, false), false))
    }

    fn latest_page(&self, page: u32) -> Result<Paged<CatalogItem>> {
        let target = page.max(1);
        let mut cursor: Option<String> = None;
        for current in 1..=target {
            let mut parameters = vec![("limit", PAGE_SIZE.to_string())];
            if let Some(value) = cursor.as_ref() {
                parameters.push(("cursor", value.clone()));
            }
            let response: CursorResponse = self.api("/api/v1/home/latest-aired", &parameters)?;
            if current == target {
                return Ok(Paged::new(
                    safe_catalog(response.data, false),
                    response.has_more,
                ));
            }
            if !response.has_more || response.next_cursor.trim().is_empty() {
                return Ok(Paged::default());
            }
            cursor = Some(response.next_cursor);
        }
        Ok(Paged::default())
    }

    fn search_page(&self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let page = page.max(1);
        let mut parameters = vec![
            ("q", query.trim().to_string()),
            ("limit", PAGE_SIZE.to_string()),
            (
                "offset",
                page.saturating_sub(1).saturating_mul(PAGE_SIZE).to_string(),
            ),
        ];
        for id in ["genre", "season", "format", "status", "country", "year"] {
            if let Some(value) = selected(filters, id) {
                parameters.push((id, value));
            }
        }
        parameters.push((
            "sort",
            selected(filters, "sort").unwrap_or_else(|| "popularity_desc".to_string()),
        ));
        let response: SearchResponse = self.api("/api/v1/search", &parameters)?;
        let has_next = response.offset.saturating_add(response.limit) < response.total;
        Ok(Paged::new(safe_catalog(response.results, false), has_next))
    }

    fn anime(&self, slug: &str) -> Result<AnimeSummary> {
        let slug = validate_slug(slug)?;
        let anime: AnimeSummary = self.api(&format!("/api/v1/anime/{slug}"), &[])?;
        ensure_allowed(&anime)?;
        if anime.anime_id != slug {
            return Err(Error::new("ReAnime returned mismatched anime metadata"));
        }
        Ok(anime)
    }

    fn episode_list(&self, slug: &str) -> Result<Vec<VideoEpisode>> {
        let slug = validate_slug(slug)?;
        let mut offset = 0_u32;
        let mut episodes = Vec::new();
        loop {
            let response: EpisodesResponse = self.api(
                &format!("/api/v1/anime/{slug}/episodes"),
                &[
                    ("limit", EPISODE_PAGE_SIZE.to_string()),
                    ("offset", offset.to_string()),
                ],
            )?;
            if response.data.is_empty() {
                break;
            }
            let count = response.data.len() as u32;
            episodes.extend(
                response
                    .data
                    .into_iter()
                    .filter(|episode| {
                        episode.episode_number > 0.0
                            && episode.playable
                            && (episode.subbed || episode.dubbed)
                    })
                    .map(|episode| video_episode(slug, episode)),
            );
            offset = offset.saturating_add(count);
            if offset >= response.total || count < EPISODE_PAGE_SIZE {
                break;
            }
            if offset > 10_000 {
                return Err(Error::new("ReAnime returned too many episodes"));
            }
        }
        episodes.sort_by(|left, right| {
            right
                .episode_number
                .partial_cmp(&left.episode_number)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(episodes)
    }

    fn player_servers(&self, slug: &str, episode: &VideoEpisode) -> Result<Vec<FlixServer>> {
        let anime = self.anime(slug)?;
        if anime.anilist_id == 0 {
            return Err(Error::new("ReAnime title is missing its AniList ID"));
        }
        let number = episode_number(episode)?;
        let response: FlixResponse =
            self.api(&format!("/api/flix/{}/{}", anime.anilist_id, number), &[])?;
        if !response.success {
            return Err(Error::new("ReAnime player server lookup failed"));
        }
        let servers = response
            .servers
            .into_iter()
            .filter(|server| matches!(server.data_type.as_str(), "sub" | "dub"))
            .filter(|server| validate_player_url(&server.data_link).is_ok())
            .collect::<Vec<_>>();
        if servers.is_empty() {
            return Err(Error::new("ReAnime episode has no playable servers"));
        }
        Ok(servers)
    }

    fn player_hosters(
        &self,
        item: &CatalogItem,
        episode: &VideoEpisode,
    ) -> Result<Vec<VideoHoster>> {
        validate_episode(&item.key, episode)?;
        Ok(hosters_from_servers(
            item,
            episode,
            self.player_servers(&item.key, episode)?,
        ))
    }

    fn capture_stream(&self, player_url: &str, language: &str) -> Result<Vec<VideoStream>> {
        validate_player_url(player_url)?;
        let response: WebViewExtractResponse = browser::extract(&WebViewExtractRequest {
            url: player_url.to_string(),
            method: Default::default(),
            body: None,
            cookie_url: None,
            session: Some(WebViewSession {
                id: "reanime-player".to_string(),
                persistence: WebViewSessionPersistence::Persistent,
                ..WebViewSession::default()
            }),
            headers: vec![("Referer".to_string(), format!("{BASE_URL}/"))],
            user_agent: None,
            wait_until: Some(WebViewWaitUntil::DomReady),
            wait_for_selector: None,
            wait_for_event: None,
            // FlixCloud can replace or move its video element after starting
            // playback. Keep the extraction boundary on the stable network
            // request instead of depending on that transient DOM shape.
            wait_for_script: Some("performance.now() >= 12000".to_string()),
            script: "(() => ({ playlistKey: window.__pk || '', video: document.querySelector('video')?.currentSrc || '' }))()".to_string(),
            timeout_ms: Some(45_000),
            capture_requests: vec![
                capture(".m3u8"),
                capture(".ass"),
                capture(".vtt"),
                capture(".srt"),
            ],
            capture_events: Vec::new(),
            cookies: false,
            headless: Some(true),
            preload_scripts: Vec::new(),
        })?;
        streams_from_capture(&response, language)
    }
}

impl VideoSource for ReAnime {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        if page > 1 {
            return Ok(Paged::default());
        }
        self.top()
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.latest_page(page)
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        self.search_page(query, page, filters)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        Ok(catalog_item(self.anime(&item.key)?, true))
    }

    fn episodes(&mut self, item: CatalogItem) -> Result<Vec<VideoEpisode>> {
        self.anime(&item.key)?;
        self.episode_list(&item.key)
    }

    fn streams(&mut self, item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoStream>> {
        let hosters = self.player_hosters(&item, &episode)?;
        let hoster = hosters
            .iter()
            .find(|hoster| hoster.key.ends_with(":sub"))
            .or_else(|| hosters.first())
            .ok_or_else(|| Error::new("ReAnime episode has no playable server"))?;
        let language = hoster
            .key
            .rsplit(':')
            .next()
            .filter(|value| matches!(*value, "sub" | "dub"))
            .ok_or_else(|| Error::new("invalid ReAnime hoster"))?;
        self.capture_stream(
            hoster
                .url
                .as_deref()
                .ok_or_else(|| Error::new("ReAnime hoster URL is missing"))?,
            language,
        )
    }

    fn hosters(&mut self, item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoHoster>> {
        self.player_hosters(&item, &episode)
    }

    fn hoster_streams(
        &mut self,
        item: CatalogItem,
        episode: VideoEpisode,
        hoster: VideoHoster,
    ) -> Result<Vec<VideoStream>> {
        let expected = self
            .player_hosters(&item, &episode)?
            .into_iter()
            .find(|candidate| candidate.key == hoster.key && candidate.url == hoster.url)
            .ok_or_else(|| Error::new("ReAnime hoster is no longer available"))?;
        let language = hoster
            .key
            .rsplit(':')
            .next()
            .filter(|value| matches!(*value, "sub" | "dub"))
            .ok_or_else(|| Error::new("invalid ReAnime hoster"))?;
        self.capture_stream(
            expected
                .url
                .as_deref()
                .ok_or_else(|| Error::new("ReAnime hoster URL is missing"))?,
            language,
        )
    }

    fn process_resource(
        &mut self,
        context: &Value,
        bytes: &[u8],
        _mime_type: Option<&str>,
    ) -> Result<Option<ProcessedMedia>> {
        if context.get("resourceType").and_then(Value::as_str) != Some("playlist") {
            return Ok(None);
        }
        let key = playlist_key_from_processing(context)
            .ok_or_else(|| Error::new("ReAnime playlist key is missing"))?;
        let plaintext = decrypt_playlist(bytes, key)?;
        Ok(Some(ProcessedMedia {
            bytes: plaintext,
            mime_type: Some("application/vnd.apple.mpegurl".to_string()),
        }))
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![
            select_filter(
                "genre",
                "Genre",
                &[
                    ("All", ""),
                    ("Action", "Action"),
                    ("Adventure", "Adventure"),
                    ("Comedy", "Comedy"),
                    ("Drama", "Drama"),
                    ("Fantasy", "Fantasy"),
                    ("Horror", "Horror"),
                    ("Mystery", "Mystery"),
                    ("Romance", "Romance"),
                    ("Sci-Fi", "Sci-Fi"),
                    ("Slice of Life", "Slice of Life"),
                    ("Sports", "Sports"),
                    ("Supernatural", "Supernatural"),
                    ("Thriller", "Thriller"),
                ],
            ),
            select_filter(
                "format",
                "Format",
                &[
                    ("All", ""),
                    ("TV", "TV"),
                    ("Movie", "MOVIE"),
                    ("OVA", "OVA"),
                    ("ONA", "ONA"),
                    ("Special", "SPECIAL"),
                    ("Music", "MUSIC"),
                ],
            ),
            select_filter(
                "status",
                "Status",
                &[
                    ("All", ""),
                    ("Finished", "Finished"),
                    ("Releasing", "Releasing"),
                    ("Not Yet Released", "Not Yet Released"),
                    ("Cancelled", "Cancelled"),
                ],
            ),
            select_filter(
                "season",
                "Season",
                &[
                    ("All", ""),
                    ("Winter", "WINTER"),
                    ("Spring", "SPRING"),
                    ("Summer", "SUMMER"),
                    ("Fall", "FALL"),
                ],
            ),
            select_filter(
                "country",
                "Origin",
                &[
                    ("All", ""),
                    ("Japan", "JP"),
                    ("South Korea", "KR"),
                    ("China", "CN"),
                    ("Taiwan", "TW"),
                ],
            ),
            select_filter(
                "sort",
                "Sort",
                &[
                    ("Popularity", "popularity_desc"),
                    ("Score", "score_desc"),
                    ("Year", "year_desc"),
                ],
            ),
        ])
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(format!(
            "{BASE_URL}/anime/{}",
            validate_slug(&item.key)?
        )))
    }

    fn episode_url(
        &mut self,
        item: &CatalogItem,
        episode: &VideoEpisode,
    ) -> Result<Option<String>> {
        validate_episode(&item.key, episode)?;
        let number = episode_number(episode)?;
        let hoster = self
            .player_hosters(item, episode)?
            .into_iter()
            .next()
            .ok_or_else(|| Error::new("ReAnime episode has no playable server"))?;
        let language = hoster
            .key
            .rsplit(':')
            .next()
            .filter(|value| matches!(*value, "sub" | "dub"))
            .ok_or_else(|| Error::new("invalid ReAnime hoster"))?;
        Ok(Some(watch_url(&item.key, &number, language)?))
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let Some(slug) = slug_from_url(candidate) else {
            return Ok(None);
        };
        Ok(Some(UrlResolveResult {
            item: Some(catalog_item(self.anime(&slug)?, true)),
            ..UrlResolveResult::default()
        }))
    }
}

#[derive(Debug, Default, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<AnimeSummary>,
    #[serde(default)]
    total: u32,
    #[serde(default)]
    limit: u32,
    #[serde(default)]
    offset: u32,
}

#[derive(Debug, Default, Deserialize)]
struct TopResponse {
    #[serde(default)]
    data: Vec<AnimeSummary>,
}

#[derive(Debug, Default, Deserialize)]
struct CursorResponse {
    #[serde(default)]
    data: Vec<AnimeSummary>,
    #[serde(default)]
    next_cursor: String,
    #[serde(default)]
    has_more: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct AnimeSummary {
    #[serde(default)]
    anime_id: String,
    #[serde(default)]
    anilist_id: u64,
    #[serde(default)]
    title: AnimeTitle,
    #[serde(default)]
    cover_image: CoverImage,
    #[serde(default)]
    banner_image: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    format: String,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    season_year: u32,
    #[serde(default)]
    average_score: f32,
    #[serde(default)]
    rating: String,
    #[serde(default)]
    is_adult: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct AnimeTitle {
    #[serde(default)]
    english: String,
    #[serde(default)]
    romaji: String,
    #[serde(default)]
    native: String,
    #[serde(default)]
    user_preferred: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct CoverImage {
    #[serde(default)]
    extra_large: String,
    #[serde(default)]
    large: String,
    #[serde(default)]
    medium: String,
}

#[derive(Debug, Default, Deserialize)]
struct EpisodesResponse {
    #[serde(default)]
    data: Vec<EpisodeDto>,
    #[serde(default)]
    total: u32,
}

#[derive(Debug, Default, Deserialize)]
struct EpisodeDto {
    #[serde(default)]
    #[serde(rename = "episodeId")]
    episode_id: String,
    #[serde(default)]
    episode_number: f32,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    duration: f64,
    #[serde(default)]
    thumbnail: String,
    #[serde(default)]
    playable: bool,
    #[serde(default)]
    subbed: bool,
    #[serde(default)]
    dubbed: bool,
    #[serde(default)]
    is_filler: bool,
}

#[derive(Debug, Default, Deserialize)]
struct FlixResponse {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    servers: Vec<FlixServer>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlixServer {
    #[serde(default)]
    server_name: String,
    #[serde(default)]
    data_link: String,
    #[serde(default)]
    data_type: String,
}

fn safe_catalog(items: Vec<AnimeSummary>, initialized: bool) -> Vec<CatalogItem> {
    items
        .into_iter()
        .filter(|item| !adult_only(item))
        .filter(|item| validate_slug(&item.anime_id).is_ok())
        .map(|item| catalog_item(item, initialized))
        .collect()
}

fn catalog_item(item: AnimeSummary, initialized: bool) -> CatalogItem {
    let title = preferred_title(&item.title);
    let mut tags = item.genres;
    if !item.format.trim().is_empty() {
        tags.push(item.format.clone());
    }
    if item.season_year > 0 {
        tags.push(item.season_year.to_string());
    }
    CatalogItem {
        key: item.anime_id.clone(),
        title,
        url: Some(format!("{BASE_URL}/anime/{}", item.anime_id)),
        cover: image(first_nonempty([
            &item.cover_image.extra_large,
            &item.cover_image.large,
            &item.cover_image.medium,
        ])),
        banner: image(nonempty(&item.banner_image)),
        description: nonempty(&clean_html(&item.description)),
        tags,
        status: nonempty(&item.status).map(Value::String),
        initialized,
        language: Some("en".to_string()),
        rating: (item.average_score > 0.0).then_some(item.average_score / 10.0),
        content_rating: Some("suggestive".to_string()),
        ..CatalogItem::default()
    }
}

fn video_episode(slug: &str, episode: EpisodeDto) -> VideoEpisode {
    let number = number_string(episode.episode_number);
    let key_part = nonempty(&episode.episode_id).unwrap_or_else(|| format!("ep-{number}"));
    let language = if episode.subbed { "sub" } else { "dub" };
    let mut labels = Vec::new();
    if episode.subbed {
        labels.push("English Sub".to_string());
    }
    if episode.dubbed {
        labels.push("English Dub".to_string());
    }
    let mut extra = std::collections::BTreeMap::new();
    extra.insert("episodeNumber".to_string(), Value::String(number.clone()));
    VideoEpisode {
        key: format!("{slug}:{key_part}"),
        title: nonempty(&episode.title).or_else(|| Some(format!("Episode {number}"))),
        description: nonempty(&clean_html(&episode.description)),
        episode_number: Some(episode.episode_number),
        thumbnail: image(nonempty(&episode.thumbnail)),
        url: watch_url(slug, &number, language).ok(),
        duration_seconds: (episode.duration > 0.0).then_some(episode.duration * 60.0),
        language: Some("en".to_string()),
        is_filler: episode.is_filler,
        labels,
        extra,
        ..VideoEpisode::default()
    }
}

fn hosters_from_servers(
    item: &CatalogItem,
    episode: &VideoEpisode,
    servers: Vec<FlixServer>,
) -> Vec<VideoHoster> {
    servers
        .into_iter()
        .map(|server| VideoHoster {
            key: format!(
                "{}:{}:{}:{}",
                item.key, episode.key, server.server_name, server.data_type
            ),
            name: format!(
                "{} - English {}",
                server.server_name,
                if server.data_type == "dub" {
                    "Dub"
                } else {
                    "Sub"
                }
            ),
            url: Some(server.data_link),
            lazy: true,
            ..VideoHoster::default()
        })
        .collect()
}

fn streams_from_capture(
    response: &WebViewExtractResponse,
    language: &str,
) -> Result<Vec<VideoStream>> {
    let mut playlist_urls = response
        .captured_requests
        .iter()
        .map(|request| request.url.clone())
        .filter(|url| is_media_url(url, ".m3u8"))
        .collect::<Vec<_>>();
    playlist_urls.sort();
    playlist_urls.dedup();
    let stream_url = playlist_urls
        .iter()
        .find(|url| url.to_ascii_lowercase().contains("/master.m3u8"))
        .or_else(|| playlist_urls.first())
        .cloned()
        .ok_or_else(|| Error::new("ReAnime player did not expose an HLS stream"))?;
    validate_stream_url(&stream_url)?;
    let playlist_key = response
        .value
        .as_ref()
        .and_then(|value| value.get("playlistKey"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new("ReAnime player did not expose its playlist key"))?;
    decode_base64(playlist_key, "ReAnime playlist key")?;

    let mut subtitles = response
        .captured_requests
        .iter()
        .map(|request| request.url.clone())
        .filter(|url| {
            is_media_url(url, ".ass") || is_media_url(url, ".vtt") || is_media_url(url, ".srt")
        })
        .collect::<Vec<_>>();
    subtitles.sort();
    subtitles.dedup();
    let subtitle_tracks = subtitles
        .into_iter()
        .filter(|url| validate_stream_url(url).is_ok())
        .map(|url| {
            let format = if is_media_url(&url, ".ass") {
                "ass"
            } else if is_media_url(&url, ".srt") {
                "srt"
            } else {
                "vtt"
            };
            MediaTrack {
                url,
                language: Some("en".to_string()),
                label: Some("English".to_string()),
                format: Some(format.to_string()),
                headers: media_headers(),
                is_default: true,
                ..MediaTrack::default()
            }
        })
        .collect();
    Ok(vec![VideoStream {
        url: stream_url,
        name: Some(if language == "dub" {
            "ReAnime - English Dub".to_string()
        } else {
            "ReAnime - English Sub".to_string()
        }),
        format: Some("hls".to_string()),
        is_hls: true,
        requires_proxy: true,
        preferred: true,
        initialized: true,
        headers: media_headers(),
        subtitles: subtitle_tracks,
        segment_processing: Some(playlist_processing(playlist_key)),
        ..VideoStream::default()
    }])
}

const PLAYLIST_KEY_HEADER: &str = "X-Manatan-ReAnime-Playlist-Key";

fn playlist_processing(playlist_key: &str) -> SegmentProcessing {
    SegmentProcessing {
        rewrite_playlist: true,
        guest_transform: true,
        max_resource_bytes: Some(2 * 1024 * 1024),
        rules: vec![SegmentRule {
            resource_types: vec![MediaResourceKind::Playlist],
            host_patterns: vec!["*.flixcloud.cc".to_string()],
            headers: [(PLAYLIST_KEY_HEADER.to_string(), playlist_key.to_string())]
                .into_iter()
                .collect(),
            ..SegmentRule::default()
        }],
        ..SegmentProcessing::default()
    }
}

fn playlist_key_from_processing(context: &Value) -> Option<&str> {
    context
        .get("processing")?
        .get("rules")?
        .as_array()?
        .iter()
        .filter_map(|rule| rule.get("headers").and_then(Value::as_object))
        .find_map(|headers| headers.get(PLAYLIST_KEY_HEADER).and_then(Value::as_str))
}

fn decrypt_playlist(bytes: &[u8], encoded_key: &str) -> Result<Vec<u8>> {
    let encoded = std::str::from_utf8(bytes)
        .map(str::trim)
        .map_err(|_| Error::new("ReAnime returned a non-text playlist"))?;
    let ciphertext = decode_base64(encoded, "ReAnime playlist")?;
    let key = decode_base64(encoded_key, "ReAnime playlist key")?;
    if key.is_empty() {
        return Err(Error::new("ReAnime playlist key is empty"));
    }
    let plaintext = ciphertext
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ key[index % key.len()])
        .collect::<Vec<_>>();
    if !plaintext.starts_with(b"#EXTM3U") {
        return Err(Error::new("ReAnime playlist decryption failed"));
    }
    Ok(plaintext)
}

fn decode_base64(value: &str, label: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| Error::new(format!("{label} is invalid")))
}

fn capture(needle: &str) -> WebViewRequestCapture {
    WebViewRequestCapture {
        url_contains: Some(needle.to_string()),
        limit: Some(16),
        ..WebViewRequestCapture::default()
    }
}

fn media_headers() -> std::collections::BTreeMap<String, String> {
    [
        ("Referer".to_string(), "https://flixcloud.cc/".to_string()),
        ("Origin".to_string(), "https://flixcloud.cc".to_string()),
    ]
    .into_iter()
    .collect()
}

fn ensure_allowed(item: &AnimeSummary) -> Result<()> {
    if adult_only(item) {
        return Err(Error::new("ReAnime adult-only titles are not supported"));
    }
    validate_slug(&item.anime_id)?;
    Ok(())
}

fn adult_only(item: &AnimeSummary) -> bool {
    item.is_adult
        || item
            .genres
            .iter()
            .any(|genre| genre.eq_ignore_ascii_case("hentai"))
        || item
            .rating
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("rx")
}

fn preferred_title(title: &AnimeTitle) -> String {
    first_nonempty([
        &title.english,
        &title.user_preferred,
        &title.romaji,
        &title.native,
    ])
    .unwrap_or_else(|| "Unknown title".to_string())
}

fn image(url: Option<String>) -> Option<ImageRequest> {
    url.filter(|value| {
        Url::parse(value)
            .ok()
            .is_some_and(|url| url.scheme() == "https" && url.host_str().is_some())
    })
    .map(ImageRequest::get)
}

fn first_nonempty<const N: usize>(values: [&str; N]) -> Option<String> {
    values.into_iter().find_map(nonempty)
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn clean_html(value: &str) -> String {
    let normalized = value
        .replace("<br><br>", "\n\n")
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");
    let mut output = String::with_capacity(normalized.len());
    let mut inside_tag = false;
    for character in normalized.chars() {
        match character {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => output.push(character),
            _ => {}
        }
    }
    output.trim().to_string()
}

fn select_filter(id: &str, name: &str, values: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Select {
        id: id.to_string(),
        name: name.to_string(),
        options: values
            .iter()
            .map(|(label, value)| OptionItem {
                label: (*label).to_string(),
                value: (*value).to_string(),
            })
            .collect(),
        default_index: 0,
    }
}

fn selected(filters: &Value, id: &str) -> Option<String> {
    filters
        .get(id)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn validate_slug(value: &str) -> Result<&str> {
    if !value.is_empty()
        && value.len() <= 240
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        Ok(value)
    } else {
        Err(Error::new("invalid ReAnime anime id"))
    }
}

fn validate_api_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    if url.scheme() != "https"
        || url.host_str() != Some("reanime.to")
        || !(url.path().starts_with("/api/v1/") || url.path().starts_with("/api/flix/"))
    {
        return Err(Error::new("unexpected ReAnime API URL"));
    }
    Ok(())
}

fn validate_player_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    if url.scheme() != "https"
        || url.host_str() != Some("flixcloud.cc")
        || !url.path().starts_with("/e/")
    {
        return Err(Error::new("unexpected ReAnime player URL"));
    }
    Ok(())
}

fn validate_stream_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || !(host == "flixcloud.cc"
            || host.ends_with(".flixcloud.cc")
            || host.ends_with(".atomic4cdn.top"))
    {
        return Err(Error::new("unexpected ReAnime media URL"));
    }
    Ok(())
}

fn watch_url(slug: &str, episode: &str, language: &str) -> Result<String> {
    let slug = validate_slug(slug)?;
    if episode.is_empty()
        || episode.len() > 16
        || !episode
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
        || !matches!(language, "sub" | "dub")
    {
        return Err(Error::new("invalid ReAnime episode selection"));
    }
    let mut url = Url::parse(&format!("{BASE_URL}/watch/{slug}")).map_err(url_error)?;
    url.query_pairs_mut()
        .append_pair("ep", episode)
        .append_pair("lang", language);
    Ok(url.into())
}

fn validate_episode(slug: &str, episode: &VideoEpisode) -> Result<()> {
    let slug = validate_slug(slug)?;
    if !episode.key.starts_with(&format!("{slug}:")) {
        return Err(Error::new("episode does not belong to this ReAnime title"));
    }
    episode_number(episode)?;
    Ok(())
}

fn episode_number(episode: &VideoEpisode) -> Result<String> {
    if let Some(value) = episode
        .extra
        .get("episodeNumber")
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 16
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'.')
        })
    {
        return Ok(value.to_string());
    }

    episode
        .episode_number
        .filter(|number| number.is_finite() && *number > 0.0)
        .map(number_string)
        .ok_or_else(|| Error::new("ReAnime episode number is missing"))
}

fn number_string(number: f32) -> String {
    if number.fract() == 0.0 {
        format!("{number:.0}")
    } else {
        number.to_string()
    }
}

fn is_media_url(url: &str, extension: &str) -> bool {
    url.split('?')
        .next()
        .is_some_and(|path| path.to_ascii_lowercase().ends_with(extension))
}

fn slug_from_url(candidate: &str) -> Option<String> {
    let url = Url::parse(candidate).ok()?;
    if url.scheme() != "https" || url.host_str()? != "reanime.to" {
        return None;
    }
    let mut parts = url.path_segments()?;
    let kind = parts.next()?;
    if !matches!(kind, "anime" | "watch") {
        return None;
    }
    let slug = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    validate_slug(slug).ok().map(str::to_string)
}

fn url_error(error: url::ParseError) -> Error {
    Error::new(format!("invalid URL: {error}"))
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(manatan_sdk::Extension::new().video("reanime", ReAnime));

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_catalog_and_excludes_adult_only_results() {
        let response: SearchResponse = serde_json::from_value(json!({
            "results": [
                {
                    "anime_id": "safe-show-abc123",
                    "title": {"english": "Safe Show"},
                    "cover_image": {"large": "https://s4.anilist.co/cover.jpg"},
                    "genres": ["Action"],
                    "average_score": 82
                },
                {
                    "anime_id": "adult-show-def456",
                    "title": {"english": "Adult Show"},
                    "genres": ["Hentai"],
                    "rating": "Rx - Hentai"
                }
            ],
            "total": 2,
            "limit": 36,
            "offset": 0
        }))
        .unwrap();
        let entries = safe_catalog(response.results, false);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "safe-show-abc123");
        assert_eq!(entries[0].title, "Safe Show");
        assert_eq!(entries[0].rating, Some(8.2));
    }

    #[test]
    fn exposes_sub_and_dub_as_separate_lazy_hosters() {
        let episode: EpisodeDto = serde_json::from_value(json!({
            "episodeId": "ep-12",
            "episode_number": 12,
            "title": "A Promise",
            "duration": 24,
            "playable": true,
            "subbed": true,
            "dubbed": true
        }))
        .unwrap();
        let item = CatalogItem::new("sample-anime-abc123", "Sample Anime");
        let episode = video_episode(&item.key, episode);
        let hosters = hosters_from_servers(
            &item,
            &episode,
            vec![
                FlixServer {
                    server_name: "HD-1".to_string(),
                    data_link: "https://flixcloud.cc/e/sub-player?v=1".to_string(),
                    data_type: "sub".to_string(),
                },
                FlixServer {
                    server_name: "HD-1".to_string(),
                    data_link: "https://flixcloud.cc/e/dub-player?v=1".to_string(),
                    data_type: "dub".to_string(),
                },
            ],
        );
        assert_eq!(hosters.len(), 2);
        assert_eq!(hosters[0].name, "HD-1 - English Sub");
        assert_eq!(hosters[1].name, "HD-1 - English Dub");

        let dub_only: EpisodeDto = serde_json::from_value(json!({
            "episodeId": "ep-13",
            "episode_number": 13,
            "playable": true,
            "subbed": false,
            "dubbed": true
        }))
        .unwrap();
        let item = CatalogItem::new("sample-anime-abc123", "Sample Anime");
        let episode = video_episode(&item.key, dub_only);
        assert!(episode.url.as_deref().unwrap().ends_with("?ep=13&lang=dub"));
        assert_eq!(
            watch_url(&item.key, &episode_number(&episode).unwrap(), "dub").unwrap(),
            episode.url.unwrap()
        );
    }

    #[test]
    fn resolves_hosters_after_episode_metadata_is_persisted() {
        let item = CatalogItem::new("sample-anime-abc123", "Sample Anime");
        let episode = VideoEpisode {
            key: "sample-anime-abc123:ep-12".to_string(),
            episode_number: Some(12.0),
            labels: vec!["English Sub".to_string()],
            ..VideoEpisode::default()
        };

        let hosters = hosters_from_servers(
            &item,
            &episode,
            vec![FlixServer {
                server_name: "HD-2".to_string(),
                data_link: "https://flixcloud.cc/e/player?v=2".to_string(),
                data_type: "sub".to_string(),
            }],
        );
        assert_eq!(hosters.len(), 1);
        assert_eq!(episode_number(&episode).unwrap(), "12");
        assert_eq!(hosters[0].name, "HD-2 - English Sub");
    }

    #[test]
    fn prefers_master_playlist_and_collects_subtitles() {
        let response = WebViewExtractResponse {
            value: Some(serde_json::json!({
                "playlistKey": base64::engine::general_purpose::STANDARD.encode(b"test-key")
            })),
            captured_requests: vec![
                manatan_sdk::browser::WebViewCapturedRequest {
                    url: "https://fetch8.flixcloud.cc/video/720/index.m3u8?token=a".to_string(),
                    ..Default::default()
                },
                manatan_sdk::browser::WebViewCapturedRequest {
                    url: "https://fetch8.flixcloud.cc/video/master.m3u8?token=a".to_string(),
                    ..Default::default()
                },
                manatan_sdk::browser::WebViewCapturedRequest {
                    url: "https://vault-96.atomic4cdn.top/subtitle/english.ass".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let streams = streams_from_capture(&response, "sub").unwrap();
        assert!(streams[0].url.contains("/master.m3u8"));
        assert_eq!(streams[0].subtitles.len(), 1);
        assert!(streams[0].requires_proxy);
        assert!(streams[0]
            .segment_processing
            .as_ref()
            .is_some_and(|processing| processing.guest_transform));
    }

    #[test]
    fn decrypts_only_playlist_resources_with_the_captured_key() {
        let key = b"playlist-key";
        let plaintext = b"#EXTM3U\n#EXT-X-VERSION:3\nsegment.ts\n";
        let ciphertext = plaintext
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ key[index % key.len()])
            .collect::<Vec<_>>();
        let encoded_key = base64::engine::general_purpose::STANDARD.encode(key);
        let context = serde_json::json!({
            "resourceType": "playlist",
            "processing": playlist_processing(&encoded_key),
        });
        let encoded_playlist = base64::engine::general_purpose::STANDARD.encode(ciphertext);

        let mut source = ReAnime;
        let processed = source
            .process_resource(&context, encoded_playlist.as_bytes(), Some("text/plain"))
            .unwrap()
            .unwrap();
        assert_eq!(processed.bytes, plaintext);
        assert_eq!(
            processed.mime_type.as_deref(),
            Some("application/vnd.apple.mpegurl")
        );
        assert!(source
            .process_resource(
                &serde_json::json!({ "resourceType": "segment" }),
                b"video",
                Some("video/mp2t"),
            )
            .unwrap()
            .is_none());
    }

    #[test]
    fn accepts_only_owned_urls_and_episode_keys() {
        assert!(validate_api_url("https://reanime.to/api/v1/search?q=test").is_ok());
        assert!(validate_api_url("https://example.com/api/v1/search").is_err());
        assert_eq!(
            slug_from_url("https://reanime.to/watch/sample-anime-abc123?ep=2"),
            Some("sample-anime-abc123".to_string())
        );
        assert!(slug_from_url("https://example.com/watch/sample-anime-abc123").is_none());
    }

    #[test]
    fn cleans_site_description_markup() {
        assert_eq!(
            clean_html("First line<br><br><b>Second line</b>"),
            "First line\n\nSecond line"
        );
    }
}
