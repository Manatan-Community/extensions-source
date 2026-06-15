use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack,
    UrlResolveResult, VideoEpisode, VideoStream, VideoStreamKind,
    abi::{ExtensionResult, system_time},
    export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const SOURCE: Mapple = Mapple;
const BASE_URL: &str = "https://mapple.uk";
const API_URL: &str = "https://db.videasy.to/3";
const MAPPLE_API: &str = "https://mapple.uk";
const DECRYPT_API: &str = "https://enc-dec.app/api";
const SUBTITLE_API: &str = "https://sub.wyzie.ru";
const IMAGE_URL: &str = "https://image.tmdb.org/t/p";

struct Mapple;

impl VideoSource for Mapple {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if listing(&request) == "latest" {
            return Ok(latest_page(&request));
        }
        let target = format!(
            "{API_URL}/trending/all/week?language=en-US&page={}",
            page(&request)
        );
        Ok(parse_media_page(
            &api_get_or_fixture(&target, LIST_FIXTURE),
            &request,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&path, &request)],
                has_next_page: false,
            });
        }
        if let Some(path) = query.strip_prefix("id:").filter(|path| valid_path(path)) {
            return Ok(Paged {
                entries: vec![fetch_details(&format!("/{path}"), &request)],
                has_next_page: false,
            });
        }

        if !query.is_empty() {
            let mut pages = latest_media_types(&request)
                .into_iter()
                .map(|media_type| {
                    let target = format!(
                        "{API_URL}/search/{media_type}?language=en-US&page={}&query={}",
                        page(&request),
                        url::query_escape(query)
                    );
                    fetch_page(&target)
                })
                .collect::<Vec<_>>();
            let has_next_page = pages.iter().any(|page| page.page < page.total_pages);
            let mut media = pages
                .drain(..)
                .flat_map(|page| page.results)
                .filter(|media| matches!(media.media_type(), "movie" | "tv"))
                .collect::<Vec<_>>();
            media.sort_by(|a, b| {
                b.popularity
                    .partial_cmp(&a.popularity)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            return Ok(Paged {
                entries: media
                    .into_iter()
                    .map(|media| media.to_item(&request))
                    .collect(),
                has_next_page,
            });
        }

        let media_type = filter(&request, "type", "movie");
        let target = discover_url(media_type, &request);
        Ok(parse_media_page(
            &api_get_or_fixture(&target, LIST_FIXTURE),
            &request,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(|| "/movie/1".to_string());
        Ok(fetch_details(&key, &request))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_else(|| "/movie/1".to_string());
        Ok(fetch_episodes(&key))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode_key = request_key(&request, "episode").unwrap_or_else(sample_episode_key);
        let data = serde_json::from_str::<EpisodeKey>(&episode_key)
            .unwrap_or_else(|_| EpisodeKey::from_path(&episode_key));
        let mut streams = resolve_episode(&data, &request);
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
        let base = base_url(&request);
        Ok(request_key(&request, "item").map(|key| format!("{base}{}", normalize_path(&key))))
    }

    fn episode_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(None)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&path, &request)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.to_string(),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn latest_page(request: &Value) -> Paged<CatalogItem> {
    let mut entries = Vec::new();
    let mut has_next_page = false;
    for media_type in latest_media_types(request) {
        let date_field = if media_type == "movie" {
            "primary_release_date"
        } else {
            "first_air_date"
        };
        let sort_by = if media_type == "movie" {
            "primary_release_date.desc"
        } else {
            "first_air_date.desc"
        };
        let target = format!(
            "{API_URL}/discover/{media_type}?language=en-US&sort_by={sort_by}&page={}&vote_count.gte=50&{date_field}.lte={}",
            page(request),
            today()
        );
        let page = fetch_page(&target);
        has_next_page |= page.page < page.total_pages;
        entries.extend(page.results.into_iter().map(|media| media.to_item(request)));
    }
    Paged {
        entries,
        has_next_page,
    }
}

fn discover_url(media_type: &str, request: &Value) -> String {
    let media_type = if media_type == "tv" { "tv" } else { "movie" };
    let sort = filter(request, "sort", "popularity.desc");
    let sort_by = match (sort, media_type) {
        ("release_date.asc", "tv") => "first_air_date.asc",
        ("release_date.desc", "tv") => "first_air_date.desc",
        ("release_date.asc", _) => "primary_release_date.asc",
        ("release_date.desc", _) => "primary_release_date.desc",
        (value, _) => value,
    };
    let mut pairs = vec![
        ("sort_by".to_string(), sort_by.to_string()),
        ("language".to_string(), "en-US".to_string()),
        ("page".to_string(), page(request).to_string()),
    ];
    let genre_ids = array_filter(request, "genres")
        .iter()
        .filter_map(|genre| genre_id(media_type, genre))
        .collect::<Vec<_>>();
    if !genre_ids.is_empty() {
        pairs.push(("with_genres".to_string(), genre_ids.join(",")));
    }
    let providers = array_filter(request, "providers");
    if !providers.is_empty() {
        pairs.push(("with_watch_providers".to_string(), providers.join("|")));
        pairs.push(("watch_region".to_string(), "US".to_string()));
    }
    let query = pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{API_URL}/discover/{media_type}?{query}")
}

fn fetch_details(path: &str, request: &Value) -> CatalogItem {
    let (media_type, id) = split_path(path).unwrap_or(("movie", "1"));
    let target = format!("{API_URL}/{media_type}/{id}?append_to_response=external_ids");
    let body = api_get_or_fixture(&target, DETAILS_FIXTURE);
    let mut item = if media_type == "tv" {
        serde_json::from_str::<TvDetailDto>(&body)
            .map(|tv| tv.to_item(request))
            .unwrap_or_else(|_| fallback_item(path, request))
    } else {
        serde_json::from_str::<MovieDetailDto>(&body)
            .map(|movie| movie.to_item(request))
            .unwrap_or_else(|_| fallback_item(path, request))
    };
    item.initialized = true;
    item
}

fn fetch_episodes(path: &str) -> Vec<VideoEpisode> {
    let (media_type, id) = split_path(path).unwrap_or(("movie", "1"));
    let target = format!("{API_URL}/{media_type}/{id}?append_to_response=external_ids");
    let body = api_get_or_fixture(&target, DETAILS_FIXTURE);
    if media_type == "tv" {
        let tv = serde_json::from_str::<TvDetailDto>(&body)
            .or_else(|_| serde_json::from_str::<TvDetailDto>(TV_DETAILS_FIXTURE));
        let Ok(tv) = tv else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for season in tv.seasons.iter().filter(|season| season.season_number > 0) {
            let target = format!("{API_URL}/tv/{}/season/{}", tv.id, season.season_number);
            let detail = serde_json::from_str::<TvSeasonDetailDto>(&api_get_or_fixture(
                &target,
                SEASON_FIXTURE,
            ))
            .unwrap_or_default();
            for episode in detail.episodes {
                let key = EpisodeKey {
                    path: format!(
                        "tv/{}/{}/{}",
                        tv.id, season.season_number, episode.episode_number
                    ),
                    title: tv.name.clone(),
                    year: tv
                        .first_air_date
                        .as_deref()
                        .unwrap_or_default()
                        .chars()
                        .take(4)
                        .collect(),
                    media_type: "tv".to_string(),
                    tmdb_id: tv.id.to_string(),
                    season: Some(season.season_number.to_string()),
                    episode: Some(episode.episode_number.to_string()),
                };
                out.push(VideoEpisode {
                    key: serde_json::to_string(&key).unwrap_or_default(),
                    title: Some(format!(
                        "S{} E{} - {}",
                        season.season_number, episode.episode_number, episode.name
                    )),
                    episode_number: Some(episode.episode_number as f32),
                    season_number: Some(season.season_number as f32),
                    date_uploaded: parse_date(&episode.air_date.unwrap_or_default()),
                    release_group: Some(format!("Season {}", season.season_number)),
                    language: Some("en".to_string()),
                    ..VideoEpisode::default()
                });
            }
        }
        out.sort_by(|a, b| {
            b.season_number
                .partial_cmp(&a.season_number)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    b.episode_number
                        .partial_cmp(&a.episode_number)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        return out;
    }

    let movie = serde_json::from_str::<MovieDetailDto>(&body)
        .or_else(|_| serde_json::from_str::<MovieDetailDto>(MOVIE_DETAILS_FIXTURE));
    let Ok(movie) = movie else {
        return Vec::new();
    };
    let key = EpisodeKey {
        path: format!("movie/{}", movie.id),
        title: movie.title.clone(),
        year: movie
            .release_date
            .as_deref()
            .unwrap_or_default()
            .chars()
            .take(4)
            .collect(),
        media_type: "movie".to_string(),
        tmdb_id: movie.id.to_string(),
        season: None,
        episode: None,
    };
    vec![VideoEpisode {
        key: serde_json::to_string(&key).unwrap_or_default(),
        title: Some("Movie".to_string()),
        episode_number: Some(1.0),
        date_uploaded: parse_date(&movie.release_date.unwrap_or_default()),
        language: Some("en".to_string()),
        ..VideoEpisode::default()
    }]
}

fn resolve_episode(episode: &EpisodeKey, request: &Value) -> Vec<VideoStream> {
    let session = fetch_session();
    let Some(session) = session else {
        return Vec::new();
    };
    let subtitles = fetch_subtitles(episode, request);
    let mut streams = Vec::new();
    for hoster in enabled_hosters(request) {
        let payload = vec![VideoRequestDto {
            media_id: episode.tmdb_id.clone(),
            media_type: episode.media_type.clone(),
            tv_slug: if episode.media_type == "tv" {
                format!(
                    "{}-{}",
                    episode.season.as_deref().unwrap_or("1"),
                    episode.episode.as_deref().unwrap_or("1")
                )
            } else {
                String::new()
            },
            source: hoster.key.to_string(),
            session_id: session.session_id.clone(),
        }];
        let request_url = format!(
            "{MAPPLE_API}/watch/{}/{}",
            episode.media_type, episode.tmdb_id
        );
        let response = mapple_client(request)
            .post(request_url)
            .xhr()
            .header("Accept", "text/x-component")
            .header("Content-Type", "text/plain;charset=UTF-8")
            .header("Next-Action", session.next_action.as_str())
            .json(serde_json::to_string(&payload).unwrap_or_else(|_| "[]".to_string()))
            .send_text();
        let Ok(response) = response else {
            continue;
        };
        let Some(data) = parse_video_response(&response) else {
            continue;
        };
        streams.extend(streams_from_source(
            hoster.name,
            &data.stream_url,
            &subtitles,
            request,
        ));
    }
    streams
}

fn fetch_session() -> Option<SessionResultDto> {
    HttpClient::browser()
        .get(format!("{DECRYPT_API}/enc-mapple"))
        .xhr()
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .ok()
        .and_then(|body| serde_json::from_str::<SessionResponseDto>(&body).ok())
        .map(|dto| dto.result)
}

fn parse_video_response(body: &str) -> Option<VideoDataDto> {
    for line in body.lines() {
        let Some(data) = line.trim().strip_prefix("1:") else {
            continue;
        };
        let response = serde_json::from_str::<VideoResponseDto>(data).ok()?;
        if response.success {
            return response.data;
        }
    }
    None
}

fn fetch_subtitles(episode: &EpisodeKey, request: &Value) -> Vec<SubtitleTrack> {
    let target = if episode.media_type == "movie" {
        format!("{SUBTITLE_API}/search?id={}", episode.tmdb_id)
    } else {
        format!(
            "{SUBTITLE_API}/search?id={}&season={}&episode={}",
            episode.tmdb_id,
            episode.season.as_deref().unwrap_or("1"),
            episode.episode.as_deref().unwrap_or("1")
        )
    };
    let limit = pref(request, "pref_sub_limit", "35")
        .parse::<usize>()
        .unwrap_or(35);
    let preferred = pref(request, "pref_sub", "en");
    let body = HttpClient::browser()
        .get(target)
        .xhr()
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .unwrap_or_else(|_| "[]".to_string());
    let mut subtitles = serde_json::from_str::<Vec<WyzieSubtitleDto>>(&body)
        .unwrap_or_default()
        .into_iter()
        .take(limit)
        .map(|sub| {
            let label = if sub.is_hearing_impaired {
                format!("{} (CC)", sub.language)
            } else {
                sub.language
            };
            SubtitleTrack {
                format: subtitle_format(&sub.url),
                language: language_code(&label),
                label: Some(label),
                url: sub.url,
                ..SubtitleTrack::default()
            }
        })
        .collect::<Vec<_>>();
    subtitles.sort_by_key(|sub| {
        i32::from(
            sub.language.as_deref() == Some(preferred.as_str())
                || sub
                    .label
                    .as_deref()
                    .is_some_and(|label| label.to_ascii_lowercase().starts_with(&preferred)),
        )
    });
    subtitles.reverse();
    subtitles
}

fn streams_from_source(
    server: &str,
    target: &str,
    subtitles: &[SubtitleTrack],
    request: &Value,
) -> Vec<VideoStream> {
    if target.to_ascii_lowercase().contains(".m3u8") {
        let expanded = expand_hls(server, target, subtitles, request);
        if !expanded.is_empty() {
            return expanded;
        }
    }
    vec![media_stream(server, target, "Auto", subtitles, request)]
}

fn expand_hls(
    server: &str,
    target: &str,
    subtitles: &[SubtitleTrack],
    request: &Value,
) -> Vec<VideoStream> {
    let headers = stream_headers(request);
    let body = HttpClient::browser()
        .get(target)
        .headers(headers)
        .send_text()
        .unwrap_or_default();
    if body.is_empty() {
        return vec![media_stream(server, target, "Auto", subtitles, request)];
    }
    let mut out = Vec::new();
    let mut pending_quality = None::<String>;
    for line in body.lines().map(str::trim) {
        if let Some(info) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending_quality = quality_from_stream_info(info);
        } else if !line.is_empty() && !line.starts_with('#') {
            let media_url = absolute_media_url(target, line);
            let quality = pending_quality.take().unwrap_or_else(|| "Auto".to_string());
            out.push(media_stream(
                server, &media_url, &quality, subtitles, request,
            ));
        }
    }
    out
}

fn media_stream(
    server: &str,
    target: &str,
    quality: &str,
    subtitles: &[SubtitleTrack],
    request: &Value,
) -> VideoStream {
    let format = stream_format(target);
    let is_hls = format == "hls";
    let is_dash = format == "dash";
    let mut label = format!("{server} - {quality}");
    if format != "direct" {
        label.push_str(" - ");
        label.push_str(&format.to_ascii_uppercase());
    }
    if !subtitles.is_empty() {
        label.push_str(&format!(" - {} subs", subtitles.len()));
    }
    VideoStream {
        url: target.to_string(),
        name: Some(label.clone()),
        quality: Some(label.clone()),
        format: Some(format.to_string()),
        is_hls,
        is_dash,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else if is_dash {
            VideoStreamKind::Dash
        } else {
            VideoStreamKind::Direct
        }),
        headers: stream_headers(request),
        subtitles: subtitles.to_vec(),
        preferred: is_preferred(&label, request),
        initialized: true,
        ..VideoStream::default()
    }
}

fn parse_media_page(body: &str, request: &Value) -> Paged<CatalogItem> {
    let page = serde_json::from_str::<PageDto<MediaItemDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap_or_default());
    Paged {
        has_next_page: page.page < page.total_pages,
        entries: page
            .results
            .into_iter()
            .filter(|media| matches!(media.media_type(), "movie" | "tv"))
            .map(|media| media.to_item(request))
            .collect(),
    }
}

fn fetch_page(target: &str) -> PageDto<MediaItemDto> {
    serde_json::from_str(&api_get_or_fixture(target, LIST_FIXTURE))
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap_or_default())
}

fn api_get_or_fixture(target: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .get(target)
        .xhr()
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn mapple_client(request: &Value) -> HttpClient {
    let base = base_url(request);
    HttpClient::browser()
        .with_referer(format!("{base}/"))
        .with_origin(&base)
        .with_cookies_for(&base)
        .with_webview_challenge_fallback()
}

fn stream_headers(request: &Value) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), format!("{}/", base_url(request)));
    headers
}

fn fallback_item(path: &str, request: &Value) -> CatalogItem {
    CatalogItem {
        key: normalize_path(path),
        title: "Mapple".to_string(),
        url: Some(format!("{}{}", base_url(request), normalize_path(path))),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

#[derive(Clone, Debug, Deserialize)]
struct PageDto<T> {
    #[serde(default = "default_page")]
    page: u64,
    #[serde(default)]
    results: Vec<T>,
    #[serde(default)]
    total_pages: u64,
}

impl<T> Default for PageDto<T> {
    fn default() -> Self {
        Self {
            page: 1,
            results: Vec::new(),
            total_pages: 0,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct MediaItemDto {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    popularity: f64,
}

impl MediaItemDto {
    fn media_type(&self) -> &str {
        self.media_type
            .as_deref()
            .unwrap_or(if self.title.is_some() { "movie" } else { "tv" })
    }

    fn title(&self) -> String {
        self.title
            .as_ref()
            .or(self.name.as_ref())
            .cloned()
            .unwrap_or_else(|| "No Title".to_string())
    }

    fn to_item(&self, request: &Value) -> CatalogItem {
        let media_type = self.media_type();
        let key = format!("/{media_type}/{}", self.id);
        CatalogItem {
            key: key.clone(),
            title: self.title(),
            cover: self
                .poster_path
                .as_ref()
                .map(|path| format!("{IMAGE_URL}/w500{path}")),
            url: Some(format!("{}{}", base_url(request), key)),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            status: ItemStatus::Unknown,
            ..CatalogItem::default()
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ExternalIdsDto {
    #[serde(default)]
    imdb_id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct GenreDto {
    #[serde(default)]
    name: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct CompanyDto {
    #[serde(default)]
    name: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct NetworkDto {
    #[serde(default)]
    name: String,
}

#[derive(Clone, Debug, Deserialize)]
struct MovieDetailDto {
    id: u64,
    title: String,
    #[serde(default)]
    genres: Vec<GenreDto>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    backdrop_path: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    vote_average: f32,
    #[serde(default)]
    production_companies: Vec<CompanyDto>,
    #[serde(default)]
    origin_country: Option<Vec<String>>,
    #[serde(default)]
    original_title: Option<String>,
    #[serde(default)]
    external_ids: Option<ExternalIdsDto>,
    #[serde(default)]
    tagline: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    runtime: Option<u64>,
}

impl MovieDetailDto {
    fn to_item(&self, request: &Value) -> CatalogItem {
        let key = format!("/movie/{}", self.id);
        CatalogItem {
            key: key.clone(),
            title: self.title.clone(),
            cover: self
                .poster_path
                .as_ref()
                .map(|path| format!("{IMAGE_URL}/w500{path}")),
            banner: self
                .backdrop_path
                .as_ref()
                .map(|path| format!("{IMAGE_URL}/w1280{path}")),
            url: Some(format!("{}{}", base_url(request), key)),
            authors: names(&self.production_companies),
            description: Some(movie_description(self)),
            tags: names(&self.genres),
            language: Some("en".to_string()),
            rating: (self.vote_average > 0.0).then_some(self.vote_average),
            content_rating: Some("adult".to_string()),
            status: parse_status(self.status.as_deref()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct TvDetailDto {
    id: u64,
    name: String,
    #[serde(default)]
    genres: Vec<GenreDto>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    backdrop_path: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    first_air_date: Option<String>,
    #[serde(default)]
    last_air_date: Option<String>,
    #[serde(default)]
    seasons: Vec<SeasonDto>,
    #[serde(default)]
    networks: Vec<NetworkDto>,
    #[serde(default)]
    production_companies: Vec<CompanyDto>,
    #[serde(default)]
    vote_average: f32,
    #[serde(default)]
    origin_country: Option<Vec<String>>,
    #[serde(default)]
    original_name: Option<String>,
    #[serde(default)]
    external_ids: Option<ExternalIdsDto>,
    #[serde(default)]
    tagline: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
}

impl TvDetailDto {
    fn to_item(&self, request: &Value) -> CatalogItem {
        let key = format!("/tv/{}", self.id);
        CatalogItem {
            key: key.clone(),
            title: self.name.clone(),
            cover: self
                .poster_path
                .as_ref()
                .map(|path| format!("{IMAGE_URL}/w500{path}")),
            banner: self
                .backdrop_path
                .as_ref()
                .map(|path| format!("{IMAGE_URL}/w1280{path}")),
            url: Some(format!("{}{}", base_url(request), key)),
            authors: names(&self.production_companies),
            artists: names(&self.networks),
            description: Some(tv_description(self)),
            tags: names(&self.genres),
            language: Some("en".to_string()),
            rating: (self.vote_average > 0.0).then_some(self.vote_average),
            content_rating: Some("adult".to_string()),
            status: parse_status(self.status.as_deref()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SeasonDto {
    season_number: i64,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct TvSeasonDetailDto {
    #[serde(default)]
    episodes: Vec<EpisodeDto>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct EpisodeDto {
    #[serde(default)]
    name: String,
    episode_number: i64,
    #[serde(default)]
    air_date: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct EpisodeKey {
    path: String,
    title: String,
    year: String,
    media_type: String,
    tmdb_id: String,
    season: Option<String>,
    episode: Option<String>,
}

impl EpisodeKey {
    fn from_path(path: &str) -> Self {
        let clean = normalize_path(path).trim_start_matches('/').to_string();
        let parts = clean.split('/').collect::<Vec<_>>();
        let media_type = parts.first().copied().unwrap_or("movie").to_string();
        Self {
            path: clean.clone(),
            title: String::new(),
            year: String::new(),
            media_type,
            tmdb_id: parts.get(1).copied().unwrap_or("1").to_string(),
            season: parts.get(2).map(|value| (*value).to_string()),
            episode: parts.get(3).map(|value| (*value).to_string()),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct SessionResponseDto {
    result: SessionResultDto,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionResultDto {
    session_id: String,
    next_action: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VideoRequestDto {
    media_id: String,
    media_type: String,
    #[serde(rename = "tv_slug")]
    tv_slug: String,
    source: String,
    session_id: String,
}

#[derive(Clone, Debug, Deserialize)]
struct VideoResponseDto {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    data: Option<VideoDataDto>,
}

#[derive(Clone, Debug, Deserialize)]
struct VideoDataDto {
    #[serde(rename = "stream_url")]
    stream_url: String,
}

#[derive(Clone, Debug, Deserialize)]
struct WyzieSubtitleDto {
    url: String,
    language: String,
    #[serde(default, rename = "isHearingImpaired")]
    is_hearing_impaired: bool,
}

#[derive(Clone, Copy, Debug)]
struct Hoster {
    name: &'static str,
    key: &'static str,
}

const HOSTERS: &[Hoster] = &[
    Hoster {
        name: "Mapple",
        key: "mapple",
    },
    Hoster {
        name: "Sakura",
        key: "sakura",
    },
    Hoster {
        name: "Pinecone",
        key: "alfa",
    },
    Hoster {
        name: "Oak",
        key: "oak",
    },
    Hoster {
        name: "Willow",
        key: "wiggles",
    },
];

fn movie_description(movie: &MovieDetailDto) -> String {
    let mut lines = Vec::new();
    if let Some(overview) = movie.overview.as_deref().filter(|value| !value.is_empty()) {
        lines.push(overview.to_string());
    }
    lines.push("Type: Movie".to_string());
    if movie.vote_average > 0.0 {
        lines.push(format!("Score: {:.1}", movie.vote_average));
    }
    push_field(&mut lines, "Tagline", movie.tagline.as_deref());
    push_field(&mut lines, "Release Date", movie.release_date.as_deref());
    if let Some(countries) = movie
        .origin_country
        .as_ref()
        .filter(|items| !items.is_empty())
    {
        lines.push(format!("Country: {}", countries.join(", ")));
    }
    if let Some(original) = movie
        .original_title
        .as_deref()
        .filter(|value| !value.is_empty() && value.trim() != movie.title.trim())
    {
        lines.push(format!("Original Title: {original}"));
    }
    if let Some(runtime) = movie.runtime.filter(|runtime| *runtime > 0) {
        let hours = runtime / 60;
        let minutes = runtime % 60;
        lines.push(if hours > 0 {
            format!("Runtime: {hours} hr {minutes} min")
        } else {
            format!("Runtime: {minutes} min")
        });
    }
    push_field(&mut lines, "Official Site", movie.homepage.as_deref());
    if let Some(imdb) = movie
        .external_ids
        .as_ref()
        .and_then(|ids| ids.imdb_id.as_deref())
    {
        lines.push(format!("IMDB: https://www.imdb.com/title/{imdb}"));
    }
    lines.join("\n\n")
}

fn tv_description(tv: &TvDetailDto) -> String {
    let mut lines = Vec::new();
    if let Some(overview) = tv.overview.as_deref().filter(|value| !value.is_empty()) {
        lines.push(overview.to_string());
    }
    lines.push("Type: TV Show".to_string());
    if tv.vote_average > 0.0 {
        lines.push(format!("Score: {:.1}", tv.vote_average));
    }
    push_field(&mut lines, "Tagline", tv.tagline.as_deref());
    push_field(&mut lines, "First Air Date", tv.first_air_date.as_deref());
    push_field(&mut lines, "Last Air Date", tv.last_air_date.as_deref());
    if let Some(countries) = tv.origin_country.as_ref().filter(|items| !items.is_empty()) {
        lines.push(format!("Country: {}", countries.join(", ")));
    }
    if let Some(original) = tv
        .original_name
        .as_deref()
        .filter(|value| !value.is_empty() && value.trim() != tv.name.trim())
    {
        lines.push(format!("Original Name: {original}"));
    }
    push_field(&mut lines, "Official Site", tv.homepage.as_deref());
    if let Some(imdb) = tv
        .external_ids
        .as_ref()
        .and_then(|ids| ids.imdb_id.as_deref())
    {
        lines.push(format!("IMDB: https://www.imdb.com/title/{imdb}"));
    }
    lines.join("\n\n")
}

fn push_field(lines: &mut Vec<String>, label: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        lines.push(format!("{label}: {value}"));
    }
}

trait Named {
    fn name(&self) -> &str;
}

impl Named for GenreDto {
    fn name(&self) -> &str {
        &self.name
    }
}

impl Named for CompanyDto {
    fn name(&self) -> &str {
        &self.name
    }
}

impl Named for NetworkDto {
    fn name(&self) -> &str {
        &self.name
    }
}

fn names<T: Named>(items: &[T]) -> Vec<String> {
    items
        .iter()
        .map(|item| item.name().to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.unwrap_or_default() {
        "Released" | "Ended" => ItemStatus::Completed,
        "Returning Series" | "In Production" => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn request_key(request: &Value, field: &str) -> Option<String> {
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
        .map(ToString::to_string)
}

fn base_url(request: &Value) -> String {
    pref(request, "pref_domain", BASE_URL)
}

fn pref(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn pref_array(request: &Value, key: &str, defaults: &[&str]) -> Vec<String> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_else(|| defaults.iter().map(|value| (*value).to_string()).collect())
}

fn filter<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

fn array_filter(request: &Value, key: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(ToString::to_string))
        .collect()
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .or_else(|| request.get("pageNumber"))
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

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

fn latest_media_types(request: &Value) -> Vec<&'static str> {
    if pref(request, "pref_latest", "movie") == "movie" {
        vec!["movie", "tv"]
    } else {
        vec!["tv", "movie"]
    }
}

fn enabled_hosters(request: &Value) -> Vec<&'static Hoster> {
    let enabled = pref_array(
        request,
        "hoster_selection",
        &["Mapple", "Sakura", "Pinecone"],
    );
    let preferred = pref(request, "preferred_server", "Mapple");
    let mut hosters = HOSTERS
        .iter()
        .filter(|hoster| enabled.iter().any(|name| name == hoster.name))
        .collect::<Vec<_>>();
    hosters.sort_by_key(|hoster| i32::from(hoster.name == preferred));
    hosters.reverse();
    hosters
}

fn path_from_url(input: &str) -> Option<String> {
    let without_scheme = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))?;
    let path_start = without_scheme.find('/')?;
    let path = &without_scheme[path_start..];
    let clean = path.split(['?', '#']).next().unwrap_or(path);
    valid_path(clean).then(|| normalize_path(clean))
}

fn valid_path(path: &str) -> bool {
    let parts = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
    matches!(parts.as_slice(), ["movie" | "tv", id] if id.parse::<u64>().is_ok())
}

fn normalize_path(path: &str) -> String {
    format!(
        "/{}",
        path.trim().trim_start_matches('/').trim_end_matches('/')
    )
}

fn split_path(path: &str) -> Option<(&str, &str)> {
    let mut parts = path.trim_start_matches('/').split('/');
    let media_type = parts.next()?;
    let id = parts.next()?;
    ((media_type == "movie" || media_type == "tv") && id.parse::<u64>().is_ok())
        .then_some((media_type, id))
}

fn stream_format(target: &str) -> &'static str {
    let lower = target.to_ascii_lowercase();
    if lower.contains(".m3u8") {
        "hls"
    } else if lower.contains(".mpd") {
        "dash"
    } else if lower.contains(".mp4") {
        "mp4"
    } else if lower.contains(".mkv") {
        "mkv"
    } else if lower.contains(".webm") {
        "webm"
    } else {
        "direct"
    }
}

fn quality_from_stream_info(info: &str) -> Option<String> {
    let resolution = info
        .split(',')
        .find_map(|part| part.trim().strip_prefix("RESOLUTION="))?;
    let height = resolution.split('x').nth(1)?;
    Some(format!("{height}p"))
}

fn absolute_media_url(master_url: &str, target: &str) -> String {
    if target.starts_with("http://") || target.starts_with("https://") {
        return target.to_string();
    }
    let base = master_url
        .rsplit_once('/')
        .map(|(base, _)| base)
        .unwrap_or(master_url);
    url::join_url(base, target)
}

fn subtitle_format(target: &str) -> Option<String> {
    let lower = target.to_ascii_lowercase();
    if lower.contains(".vtt") {
        Some("vtt".to_string())
    } else if lower.contains(".srt") {
        Some("srt".to_string())
    } else {
        None
    }
}

fn language_code(label: &str) -> Option<String> {
    let lower = label.to_ascii_lowercase();
    let code = if lower.starts_with("ar") || lower.contains("arabic") {
        "ar"
    } else if lower.starts_with("bn") || lower.contains("bengali") {
        "bn"
    } else if lower.starts_with("zh") || lower.contains("chinese") {
        "zh"
    } else if lower.starts_with("en") || lower.contains("english") {
        "en"
    } else if lower.starts_with("fr") || lower.contains("french") {
        "fr"
    } else if lower.starts_with("de") || lower.contains("german") {
        "de"
    } else if lower.starts_with("hi") || lower.contains("hindi") {
        "hi"
    } else if lower.starts_with("id") || lower.contains("indonesian") {
        "id"
    } else if lower.starts_with("it") || lower.contains("italian") {
        "it"
    } else if lower.starts_with("ja") || lower.contains("japanese") {
        "ja"
    } else if lower.starts_with("ko") || lower.contains("korean") {
        "ko"
    } else if lower.starts_with("fa") || lower.contains("persian") {
        "fa"
    } else if lower.starts_with("pt") || lower.contains("portuguese") {
        "pt"
    } else if lower.starts_with("ru") || lower.contains("russian") {
        "ru"
    } else if lower.starts_with("es") || lower.contains("spanish") {
        "es"
    } else if lower.starts_with("tr") || lower.contains("turkish") {
        "tr"
    } else if lower.starts_with("ur") || lower.contains("urdu") {
        "ur"
    } else if lower.starts_with("vi") || lower.contains("vietnamese") {
        "vi"
    } else {
        return None;
    };
    Some(code.to_string())
}

fn is_preferred(quality: &str, request: &Value) -> bool {
    quality.contains(&pref(request, "pref_quality", "1080"))
        || quality.contains(&pref(request, "preferred_server", "Mapple"))
}

fn quality_score(quality: &str) -> i32 {
    if quality.to_ascii_lowercase().contains("4k") {
        return 2160;
    }
    quality
        .chars()
        .map(|ch| if ch.is_ascii_digit() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .filter_map(|part| part.parse::<i32>().ok())
        .max()
        .unwrap_or(0)
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or_default();
        (
            i32::from(is_preferred(quality, request)),
            quality_score(quality),
        )
    });
    streams.reverse();
}

fn genre_id(media_type: &str, genre: &str) -> Option<&'static str> {
    match (media_type, genre) {
        ("movie", "Action") => Some("28"),
        ("movie", "Adventure") => Some("12"),
        ("movie", "Animation") => Some("16"),
        ("movie", "Comedy") => Some("35"),
        ("movie", "Crime") => Some("80"),
        ("movie", "Documentary") => Some("99"),
        ("movie", "Drama") => Some("18"),
        ("movie", "Family") => Some("10751"),
        ("movie", "Fantasy") => Some("14"),
        ("movie", "History") => Some("36"),
        ("movie", "Horror") => Some("27"),
        ("movie", "Music") => Some("10402"),
        ("movie", "Mystery") => Some("9648"),
        ("movie", "Romance") => Some("10749"),
        ("movie", "Science Fiction") => Some("878"),
        ("movie", "TV Movie") => Some("10770"),
        ("movie", "Thriller") => Some("53"),
        ("movie", "War") => Some("10752"),
        ("movie", "Western") => Some("37"),
        ("tv", "Action & Adventure") => Some("10759"),
        ("tv", "Animation") => Some("16"),
        ("tv", "Comedy") => Some("35"),
        ("tv", "Crime") => Some("80"),
        ("tv", "Documentary") => Some("99"),
        ("tv", "Drama") => Some("18"),
        ("tv", "Family") => Some("10751"),
        ("tv", "Kids") => Some("10762"),
        ("tv", "Mystery") => Some("9648"),
        ("tv", "News") => Some("10763"),
        ("tv", "Reality") => Some("10764"),
        ("tv", "Sci-Fi & Fantasy") => Some("10765"),
        ("tv", "Soap") => Some("10766"),
        ("tv", "Talk") => Some("10767"),
        ("tv", "War & Politics") => Some("10768"),
        ("tv", "Western") => Some("37"),
        _ => None,
    }
}

fn parse_date(date: &str) -> Option<i64> {
    let mut parts = date.split('-').filter_map(|part| part.parse::<i64>().ok());
    let year = parts.next()?;
    let month = parts.next()?;
    let day = parts.next()?;
    Some(days_from_civil(year, month, day) * 86_400_000)
}

fn today() -> String {
    let seconds = system_time().map(|time| time.unix_seconds).unwrap_or(0);
    let days = seconds.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month, day)
}

fn default_page() -> u64 {
    1
}

fn sample_episode_key() -> String {
    serde_json::to_string(&EpisodeKey {
        path: "movie/1".to_string(),
        title: "Sample Movie".to_string(),
        year: "2024".to_string(),
        media_type: "movie".to_string(),
        tmdb_id: "1".to_string(),
        season: None,
        episode: None,
    })
    .unwrap_or_default()
}

const LIST_FIXTURE: &str = r#"{"page":1,"total_pages":1,"results":[{"id":1,"title":"Sample Movie","media_type":"movie","poster_path":"/sample.jpg","popularity":1.0}]}"#;
const DETAILS_FIXTURE: &str = MOVIE_DETAILS_FIXTURE;
const MOVIE_DETAILS_FIXTURE: &str = r#"{"id":1,"title":"Sample Movie","genres":[{"name":"Action"}],"overview":"Fixture movie used when local smoke tests cannot reach the host.","poster_path":"/sample.jpg","backdrop_path":"/sample-backdrop.jpg","status":"Released","release_date":"2024-01-01","vote_average":7.2,"production_companies":[{"name":"Fixture Studio"}],"origin_country":["US"],"original_title":"Sample Movie","external_ids":{"imdb_id":"tt0000001"},"runtime":90}"#;
const TV_DETAILS_FIXTURE: &str = r#"{"id":2,"name":"Sample Show","genres":[{"name":"Drama"}],"overview":"Fixture show used when local smoke tests cannot reach the host.","poster_path":"/sample-tv.jpg","status":"Ended","first_air_date":"2024-01-01","last_air_date":"2024-02-01","seasons":[{"season_number":1}],"networks":[{"name":"Fixture Network"}],"production_companies":[{"name":"Fixture Studio"}],"vote_average":7.1,"origin_country":["US"],"original_name":"Sample Show","external_ids":{"imdb_id":"tt0000002"}}"#;
const SEASON_FIXTURE: &str =
    r#"{"episodes":[{"name":"First Episode","episode_number":1,"air_date":"2024-01-01"}]}"#;

export_video_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_fixture() {
        let page = parse_media_page(LIST_FIXTURE, &Value::Null);
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].key, "/movie/1");
    }

    #[test]
    fn builds_movie_episode_key() {
        let episodes = fetch_episodes("/movie/1");
        assert_eq!(episodes.len(), 1);
        assert!(episodes[0].key.contains("\"media_type\":\"movie\""));
    }

    #[test]
    fn parses_mapple_video_response() {
        let data = parse_video_response(
            r#"0:["$","div",null,{}]
1:{"success":true,"data":{"stream_url":"https://cdn.example.invalid/master.m3u8","source":"mapple"}}"#,
        );
        assert_eq!(
            data.unwrap().stream_url,
            "https://cdn.example.invalid/master.m3u8"
        );
    }
}
