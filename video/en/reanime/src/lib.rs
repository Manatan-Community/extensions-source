use manatan_sdk::{
    browser::{
        self, WebViewExtractRequest, WebViewExtractResponse, WebViewRequestCapture, WebViewSession,
        WebViewSessionPersistence, WebViewWaitUntil,
    },
    client::Client,
    CatalogItem, Error, FilterDefinition, ImageRequest, MediaTrack, OptionItem, Paged, Result,
    UrlResolveResult, VideoEpisode, VideoHoster, VideoSource, VideoStream,
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

    fn capture_stream(&self, watch_url: &str, language: &str) -> Result<Vec<VideoStream>> {
        validate_watch_url(watch_url)?;
        let response: WebViewExtractResponse = browser::extract(&WebViewExtractRequest {
            url: watch_url.to_string(),
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
            wait_for_script: Some(
                "performance.now() >= 12000 && Boolean(document.querySelector('iframe[src*=\"flixcloud\"]'))"
                    .to_string(),
            ),
            script: "(() => ({ iframe: document.querySelector('iframe[src*=\"flixcloud\"]')?.src || '' }))()"
                .to_string(),
            timeout_ms: Some(45_000),
            capture_requests: vec![capture(".m3u8"), capture(".ass"), capture(".vtt")],
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
        let hoster = self
            .hosters(item.clone(), episode.clone())?
            .into_iter()
            .next()
            .ok_or_else(|| Error::new("ReAnime episode has no playable language"))?;
        self.hoster_streams(item, episode, hoster)
    }

    fn hosters(&mut self, item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoHoster>> {
        validate_episode(&item.key, &episode)?;
        let mut hosters = Vec::new();
        for (language, label) in [("sub", "English Sub"), ("dub", "English Dub")] {
            if episode.labels.iter().any(|value| value == label) {
                let url = watch_url(&item.key, episode_number(&episode)?, language)?;
                hosters.push(VideoHoster {
                    key: format!("{}:{}:{language}", item.key, episode.key),
                    name: label.to_string(),
                    url: Some(url),
                    lazy: true,
                    ..VideoHoster::default()
                });
            }
        }
        Ok(hosters)
    }

    fn hoster_streams(
        &mut self,
        item: CatalogItem,
        episode: VideoEpisode,
        hoster: VideoHoster,
    ) -> Result<Vec<VideoStream>> {
        validate_episode(&item.key, &episode)?;
        let prefix = format!("{}:{}:", item.key, episode.key);
        let language = hoster
            .key
            .strip_prefix(&prefix)
            .filter(|value| matches!(*value, "sub" | "dub"))
            .ok_or_else(|| Error::new("invalid ReAnime hoster"))?;
        let expected_label = if language == "dub" {
            "English Dub"
        } else {
            "English Sub"
        };
        if !episode.labels.iter().any(|value| value == expected_label) {
            return Err(Error::new(
                "ReAnime hoster language is unavailable for this episode",
            ));
        }
        let expected = watch_url(&item.key, episode_number(&episode)?, language)?;
        if hoster.url.as_deref() != Some(expected.as_str()) {
            return Err(Error::new("ReAnime hoster URL does not match the episode"));
        }
        self.capture_stream(&expected, language)
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
        Ok(Some(watch_url(
            &item.key,
            episode_number(episode)?,
            default_language(episode)?,
        )?))
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

    let mut subtitles = response
        .captured_requests
        .iter()
        .map(|request| request.url.clone())
        .filter(|url| is_media_url(url, ".ass") || is_media_url(url, ".vtt"))
        .collect::<Vec<_>>();
    subtitles.sort();
    subtitles.dedup();
    let subtitle_tracks = subtitles
        .into_iter()
        .filter(|url| validate_stream_url(url).is_ok())
        .map(|url| {
            let format = if is_media_url(&url, ".ass") {
                "ass"
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
        ..VideoStream::default()
    }])
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
        || !url.path().starts_with("/api/v1/")
    {
        return Err(Error::new("unexpected ReAnime API URL"));
    }
    Ok(())
}

fn validate_watch_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    if url.scheme() != "https"
        || url.host_str() != Some("reanime.to")
        || !url.path().starts_with("/watch/")
    {
        return Err(Error::new("unexpected ReAnime watch URL"));
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

fn episode_number(episode: &VideoEpisode) -> Result<&str> {
    episode
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
        .ok_or_else(|| Error::new("ReAnime episode number is missing"))
}

fn default_language(episode: &VideoEpisode) -> Result<&'static str> {
    if episode.labels.iter().any(|value| value == "English Sub") {
        Ok("sub")
    } else if episode.labels.iter().any(|value| value == "English Dub") {
        Ok("dub")
    } else {
        Err(Error::new("ReAnime episode has no playable language"))
    }
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
        let mut source = ReAnime;
        let item = CatalogItem::new("sample-anime-abc123", "Sample Anime");
        let episode = video_episode(&item.key, episode);
        let hosters = source.hosters(item, episode).unwrap();
        assert_eq!(hosters.len(), 2);
        assert_eq!(hosters[0].name, "English Sub");
        assert!(hosters[0]
            .url
            .as_deref()
            .unwrap()
            .ends_with("?ep=12&lang=sub"));
        assert_eq!(hosters[1].name, "English Dub");

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
            source.episode_url(&item, &episode).unwrap().as_deref(),
            episode.url.as_deref()
        );
    }

    #[test]
    fn prefers_master_playlist_and_collects_subtitles() {
        let response = WebViewExtractResponse {
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
