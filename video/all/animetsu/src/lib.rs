use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MediaSegment, Paged, SubtitleTrack,
    UrlResolveResult, VideoEpisode, VideoHoster, VideoStream, VideoStreamKind,
    abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Animetsu = Animetsu;
const DEFAULT_BASE_URL: &str = "https://animetsu.net";
const PROXY_URL: &str = "https://swiftstream.top/proxy";
const PER_PAGE: u64 = 35;

struct Animetsu;

impl VideoSource for Animetsu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base_url = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            let body = fetch_json_or_fixture(
                &request,
                &format!(
                    "{}/v2/api/anime/recent?page={page}&per_page={PER_PAGE}",
                    base_url
                ),
                RECENT_FIXTURE,
                &format!("{base_url}/browse"),
            );
            return Ok(parse_recent(&body, &request));
        }
        let body = fetch_json_or_fixture(
            &request,
            &format!(
                "{}/v2/api/anime/search/?sort=trending&page={page}&per_page={PER_PAGE}",
                base_url
            ),
            SEARCH_FIXTURE,
            &format!("{base_url}/browse"),
        );
        Ok(parse_search(&body, &request))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = anime_id_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&request, &id)],
                has_next_page: false,
            });
        }
        let base_url = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut target = format!("{base_url}/v2/api/anime/search/?page={page}&per_page={PER_PAGE}");
        if !query.is_empty() {
            target.push_str("&query=");
            target.push_str(&url::query_escape(query));
        }
        let body = fetch_json_or_fixture(
            &request,
            &target,
            SEARCH_FIXTURE,
            &format!("{base_url}/browse"),
        );
        Ok(parse_search(&body, &request))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(|| "sample".to_string());
        Ok(fetch_details(&request, &key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base_url = base_url(&request);
        let key = request_key(&request, "item").unwrap_or_else(|| "sample".to_string());
        let body = fetch_json_or_fixture(
            &request,
            &format!("{base_url}/v2/api/anime/eps/{key}"),
            EPISODES_FIXTURE,
            &format!("{base_url}/anime/{key}"),
        );
        Ok(parse_episodes(&body, &key))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let Some((anime_id, ep_num)) = episode_parts(&request) else {
            return Ok(Vec::new());
        };
        let base_url = base_url(&request);
        let body = fetch_json_or_fixture(
            &request,
            &format!("{base_url}/v2/api/anime/servers/{anime_id}/{ep_num}"),
            SERVERS_FIXTURE,
            &format!("{base_url}/watch/{anime_id}"),
        );
        Ok(parse_hosters(&body, &request, &anime_id, &ep_num))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(hoster_key) = request_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = hoster_key.split('|');
        let anime_id = parts.next().unwrap_or("sample");
        let ep_num = parts.next().unwrap_or("1");
        let server = parts.next().unwrap_or("baku");
        let audio_type = parts.next().unwrap_or("sub");
        Ok(fetch_streams(
            &request, anime_id, ep_num, server, audio_type,
        ))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some((anime_id, ep_num)) = episode_parts(&request) else {
            return Ok(Vec::new());
        };
        let hosters = self.hosters(request.clone())?;
        let mut streams = Vec::new();
        for hoster in hosters {
            let mut resolved = self.resolve_hoster(json!({
                "hoster": { "key": hoster.key },
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null),
            }))?;
            for stream in &mut resolved {
                stream.hoster = Some(hoster.clone());
            }
            streams.extend(resolved);
        }
        if streams.is_empty() {
            streams = fetch_streams(&request, &anime_id, &ep_num, "baku", "sub");
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({
            "listing": "popular",
            "preferences": request.get("preferences").cloned().unwrap_or(Value::Null),
        }))?;
        let latest = self.list(json!({
            "listing": "latest",
            "preferences": request.get("preferences").cloned().unwrap_or(Value::Null),
        }))?;
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
        Ok(request_key(&request, "item").map(|key| format!("{}/anime/{key}", base_url(&request))))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(episode_parts(&request)
            .map(|(anime_id, _)| format!("{}/watch/{anime_id}", base_url(&request))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = anime_id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&request, &id)),
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

fn client(base_url: &str, referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_referer(referer)
        .with_header("Accept", "application/json, text/plain, */*")
        .with_header("Sec-Fetch-Dest", "empty")
        .with_header("Sec-Fetch-Mode", "cors")
        .with_header("Sec-Fetch-Site", "same-origin")
        .with_cookies_for(base_url)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(request: &Value, target: &str, fixture: &str, referer: &str) -> String {
    let base_url = base_url(request);
    client(&base_url, referer)
        .get(target)
        .xhr()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(request: &Value, key: &str) -> CatalogItem {
    let base_url = base_url(request);
    let body = fetch_json_or_fixture(
        request,
        &format!("{base_url}/v2/api/anime/info/{key}"),
        DETAILS_FIXTURE,
        &format!("{base_url}/anime/{key}"),
    );
    serde_json::from_str::<AnimetsuAnimeDto>(&body)
        .ok()
        .and_then(|dto| dto.into_catalog(request, true))
        .unwrap_or_else(|| fallback_item(key, &base_url))
}

fn fetch_streams(
    request: &Value,
    anime_id: &str,
    ep_num: &str,
    server: &str,
    audio_type: &str,
) -> Vec<VideoStream> {
    let base_url = base_url(request);
    let target = format!(
        "{base_url}/v2/api/anime/oppai/{anime_id}/{ep_num}?server={server}&source_type={audio_type}"
    );
    let body = fetch_json_or_fixture(
        request,
        &target,
        VIDEO_FIXTURE,
        &format!("{base_url}/watch/{anime_id}"),
    );
    let dto: AnimetsuVideoDto = serde_json::from_str(&body).unwrap_or_default();
    parse_streams(dto, request, &base_url, server, audio_type)
}

fn parse_search(body: &str, request: &Value) -> Paged<CatalogItem> {
    let dto: AnimetsuSearchDto = serde_json::from_str(body).unwrap_or_default();
    let hide_adult = preference_bool(request, "hideAdultContent", true);
    Paged {
        entries: dto
            .results
            .into_iter()
            .filter(|item| !hide_adult || !item.is_adult)
            .filter_map(|item| item.into_catalog(request, false))
            .collect(),
        has_next_page: dto.page < dto.last_page,
    }
}

fn parse_recent(body: &str, request: &Value) -> Paged<CatalogItem> {
    let dto: AnimetsuRecentDto = serde_json::from_str(body).unwrap_or_default();
    let hide_adult = preference_bool(request, "hideAdultContent", true);
    Paged {
        entries: dto
            .results
            .into_iter()
            .filter(|item| !hide_adult || !item.is_adult)
            .filter_map(|item| item.into_catalog(request, false))
            .collect(),
        has_next_page: dto.current_page < dto.last_page,
    }
}

fn parse_episodes(body: &str, anime_id: &str) -> Vec<VideoEpisode> {
    let mut episodes: Vec<_> = serde_json::from_str::<Vec<AnimetsuEpisodeDto>>(body)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|episode| episode.into_episode(anime_id))
        .collect();
    episodes.sort_by(|a, b| {
        b.episode_number
            .partial_cmp(&a.episode_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    episodes
}

fn parse_hosters(body: &str, request: &Value, anime_id: &str, ep_num: &str) -> Vec<VideoHoster> {
    let mut servers: Vec<AnimetsuServerDto> = serde_json::from_str(body).unwrap_or_default();
    let preferred_server =
        preference(request, "preferredServer").unwrap_or_else(|| "none".to_string());
    servers.sort_by_key(|server| server.id != preferred_server);
    let audio_types = sorted_audio_types(request);
    servers
        .into_iter()
        .flat_map(|server| {
            audio_types.iter().map(move |audio_type| VideoHoster {
                key: format!("{anime_id}|{ep_num}|{}|{audio_type}", server.id),
                name: format!("{} {}", server.id.to_uppercase(), audio_type.to_uppercase()),
                url: Some(format!("{}/watch/{anime_id}", base_url(request))),
                lazy: true,
                video_count: Some(1),
                ..VideoHoster::default()
            })
        })
        .collect()
}

fn parse_streams(
    dto: AnimetsuVideoDto,
    request: &Value,
    base_url: &str,
    server: &str,
    audio_type: &str,
) -> Vec<VideoStream> {
    let subtitles = dto
        .subs
        .unwrap_or_default()
        .into_iter()
        .map(|sub| SubtitleTrack {
            url: sub.url,
            language: sub.lang.clone(),
            label: sub.lang,
            format: Some("vtt".to_string()),
            ..SubtitleTrack::default()
        })
        .collect::<Vec<_>>();
    dto.sources
        .into_iter()
        .map(|source| {
            let quality = source.quality.clone();
            let stream_url = if source.need_proxy {
                format!("{PROXY_URL}{}", source.url)
            } else if source.url.starts_with("http") {
                source.url.clone()
            } else {
                url::join_url(base_url, &source.url)
            };
            let is_hls = source
                .content_type
                .as_deref()
                .is_some_and(|kind| kind.contains("mpegurl"))
                || stream_url.contains(".m3u8");
            let mut headers = Context::new();
            headers.insert("Referer".to_string(), format!("{base_url}/"));
            headers.insert("Origin".to_string(), base_url.to_string());
            VideoStream {
                url: stream_url,
                name: Some(format!(
                    "{}: {} ({}){}",
                    server.to_uppercase(),
                    quality,
                    audio_type.to_uppercase(),
                    sub_label(server)
                )),
                quality: Some(quality.clone()),
                format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
                is_hls,
                stream_kind: Some(if is_hls {
                    VideoStreamKind::Hls
                } else {
                    VideoStreamKind::Direct
                }),
                headers,
                subtitles: subtitles.clone(),
                intro: dto
                    .skips
                    .as_ref()
                    .and_then(|skips| skips.intro.as_ref())
                    .map(|time| MediaSegment {
                        start_seconds: time.start,
                        end_seconds: time.end,
                        ..MediaSegment::default()
                    }),
                outro: dto
                    .skips
                    .as_ref()
                    .and_then(|skips| skips.outro.as_ref())
                    .map(|time| MediaSegment {
                        start_seconds: time.start,
                        end_seconds: time.end,
                        ..MediaSegment::default()
                    }),
                preferred: quality.contains(
                    &preference(request, "preferredQuality").unwrap_or_else(|| "1080".to_string()),
                ),
                ..VideoStream::default()
            }
        })
        .collect()
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let quality = preference(request, "preferredQuality").unwrap_or_else(|| "1080".to_string());
    streams.sort_by_key(|stream| {
        !stream
            .quality
            .as_deref()
            .unwrap_or_default()
            .contains(&quality)
    });
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
        .and_then(|value| {
            if field == "item" {
                anime_id_from_url(value).or_else(|| Some(value.to_string()))
            } else {
                Some(value.to_string())
            }
        })
}

fn episode_parts(request: &Value) -> Option<(String, String)> {
    let key = request_key(request, "episode")?;
    let key = key.trim_matches('/');
    if let Some(path) = key.split("/watch/").nth(1) {
        let anime_id = path.split(['/', '?', '#']).next()?.to_string();
        return Some((anime_id, "1".to_string()));
    }
    let mut parts = key.split('/');
    let anime_id = parts.next()?.to_string();
    let ep_num = parts.next().unwrap_or("1").to_string();
    Some((anime_id, ep_num))
}

fn anime_id_from_url(input: &str) -> Option<String> {
    input
        .split("/anime/")
        .nth(1)
        .and_then(|tail| tail.split(['/', '?', '#']).next())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn base_url(request: &Value) -> String {
    preference(request, "preferredDomain")
        .or_else(|| preference(request, "preferred_domain"))
        .filter(|value| value.starts_with("https://animetsu."))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn preference(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn preference_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn sorted_audio_types(request: &Value) -> Vec<String> {
    let preferred = preference(request, "preferredAudioType").unwrap_or_else(|| "none".to_string());
    let mut values = vec!["sub".to_string(), "dub".to_string()];
    values.sort_by_key(|value| value != &preferred);
    values
}

fn sub_label(server: &str) -> &'static str {
    match server {
        "baku" | "dio" | "meg" => " [Hard Subs]",
        "kite" => " [Soft Subs]",
        _ => "",
    }
}

fn fallback_item(key: &str, base_url: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: key.replace(['-', '_'], " "),
        url: Some(format!("{base_url}/anime/{key}")),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

impl AnimetsuAnimeDto {
    fn into_catalog(self, request: &Value, initialized: bool) -> Option<CatalogItem> {
        let title_lang =
            preference(request, "preferredTitleLang").unwrap_or_else(|| "romaji".to_string());
        let title = self.title.as_ref()?.preferred(&title_lang)?;
        let mut description = self.description.map(|value| html::strip_tags(&value));
        if initialized {
            let mut extra = Vec::new();
            if let Some(format) = self.format {
                extra.push(format.replace('_', " "));
            }
            if let Some(total_eps) = self.total_eps {
                extra.push(format!("Episodes: {total_eps}"));
            }
            if let Some(duration) = self.duration {
                extra.push(format!("Duration: {duration} min"));
            }
            if !extra.is_empty() {
                let existing = description.unwrap_or_default();
                description = Some(if existing.is_empty() {
                    extra.join(" | ")
                } else {
                    format!("{existing}\n\n{}", extra.join(" | "))
                });
            }
        }
        Some(CatalogItem {
            key: self.id.clone(),
            title,
            alternate_titles: self
                .title
                .map(AnimetsuTitleDto::all_titles)
                .unwrap_or_default(),
            cover: self
                .cover_image
                .and_then(|cover| cover.large.or(cover.medium).or(cover.small)),
            banner: self.banner,
            url: Some(format!("{}/anime/{}", base_url(request), self.id)),
            authors: self
                .studios
                .unwrap_or_default()
                .into_iter()
                .map(|studio| studio.name)
                .collect(),
            description,
            tags: [
                self.genres.unwrap_or_default(),
                self.tags.unwrap_or_default(),
            ]
            .concat(),
            language: Some("all".to_string()),
            rating: self.average_score.map(|score| score as f32 / 20.0),
            content_rating: Some(if self.is_adult { "adult" } else { "safe" }.to_string()),
            status: parse_status(self.status.as_deref()),
            initialized,
            ..CatalogItem::default()
        })
    }
}

impl AnimetsuTitleDto {
    fn preferred(&self, language: &str) -> Option<String> {
        match language {
            "english" => self.english.clone(),
            "native" => self.native_title.clone(),
            _ => self.romaji.clone(),
        }
        .or_else(|| self.romaji.clone())
        .or_else(|| self.english.clone())
        .or_else(|| self.native_title.clone())
        .filter(|value| !value.is_empty())
    }

    fn all_titles(self) -> Vec<String> {
        [self.romaji, self.english, self.native_title]
            .into_iter()
            .flatten()
            .collect()
    }
}

impl AnimetsuEpisodeDto {
    fn into_episode(self, anime_id: &str) -> Option<VideoEpisode> {
        let ep_num = self.ep_num?;
        if ep_num <= 0.0 {
            return None;
        }
        let ep_num_str = if ep_num.fract() == 0.0 {
            format!("{}", ep_num as i64)
        } else {
            ep_num.to_string()
        };
        Some(VideoEpisode {
            key: format!("{anime_id}/{ep_num_str}"),
            title: Some(match self.name {
                Some(name) if !name.is_empty() => format!("Ep. {ep_num_str} - {name}"),
                _ => format!("Ep. {ep_num_str}"),
            }),
            description: self.desc,
            episode_number: Some(ep_num as f32),
            url: Some(format!("{DEFAULT_BASE_URL}/watch/{anime_id}")),
            is_filler: self.is_filler.unwrap_or(false),
            labels: vec!["sub".to_string(), "dub".to_string()],
            ..VideoEpisode::default()
        })
    }
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status {
        Some("RELEASING") => ItemStatus::Ongoing,
        Some("FINISHED") => ItemStatus::Completed,
        Some("CANCELLED") => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

#[derive(Default, Deserialize)]
struct AnimetsuSearchDto {
    #[serde(default)]
    results: Vec<AnimetsuAnimeDto>,
    #[serde(default)]
    page: u64,
    #[serde(default, rename = "last_page")]
    last_page: u64,
}

#[derive(Default, Deserialize)]
struct AnimetsuRecentDto {
    #[serde(default)]
    results: Vec<AnimetsuAnimeDto>,
    #[serde(default, rename = "current_page")]
    current_page: u64,
    #[serde(default, rename = "last_page")]
    last_page: u64,
}

#[derive(Default, Deserialize)]
struct AnimetsuAnimeDto {
    id: String,
    title: Option<AnimetsuTitleDto>,
    status: Option<String>,
    #[serde(default, rename = "is_adult")]
    is_adult: bool,
    #[serde(default, rename = "cover_image")]
    cover_image: Option<AnimetsuCoverDto>,
    banner: Option<String>,
    description: Option<String>,
    #[serde(default, rename = "total_eps")]
    total_eps: Option<u32>,
    format: Option<String>,
    duration: Option<u32>,
    #[serde(default)]
    genres: Option<Vec<String>>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default, rename = "average_score")]
    average_score: Option<u32>,
    #[serde(default)]
    studios: Option<Vec<AnimetsuStudioDto>>,
}

#[derive(Default, Deserialize)]
struct AnimetsuTitleDto {
    romaji: Option<String>,
    english: Option<String>,
    #[serde(rename = "native")]
    native_title: Option<String>,
}

#[derive(Default, Deserialize)]
struct AnimetsuCoverDto {
    large: Option<String>,
    medium: Option<String>,
    small: Option<String>,
}

#[derive(Default, Deserialize)]
struct AnimetsuStudioDto {
    name: String,
}

#[derive(Default, Deserialize)]
struct AnimetsuEpisodeDto {
    #[serde(rename = "ep_num")]
    ep_num: Option<f64>,
    desc: Option<String>,
    #[serde(rename = "is_filler")]
    is_filler: Option<bool>,
    name: Option<String>,
}

#[derive(Clone, Default, Deserialize)]
struct AnimetsuServerDto {
    id: String,
}

#[derive(Default, Deserialize)]
struct AnimetsuVideoDto {
    #[serde(default)]
    sources: Vec<AnimetsuSourceDto>,
    #[serde(default)]
    subs: Option<Vec<AnimetsuSubDto>>,
    #[serde(default)]
    skips: Option<AnimetsuSkipsDto>,
}

#[derive(Default, Deserialize)]
struct AnimetsuSourceDto {
    quality: String,
    url: String,
    #[serde(rename = "type")]
    content_type: Option<String>,
    #[serde(default, rename = "need_proxy")]
    need_proxy: bool,
}

#[derive(Clone, Deserialize)]
struct AnimetsuSubDto {
    url: String,
    lang: Option<String>,
}

#[derive(Default, Deserialize)]
struct AnimetsuSkipsDto {
    intro: Option<AnimetsuSkipTimeDto>,
    outro: Option<AnimetsuSkipTimeDto>,
}

#[derive(Deserialize)]
struct AnimetsuSkipTimeDto {
    start: f64,
    end: f64,
}

const SEARCH_FIXTURE: &str = r#"{"results":[{"id":"sample","title":{"romaji":"Sample Animetsu","english":"Sample Animetsu"},"status":"RELEASING","is_adult":false,"cover_image":{"large":"https://img.example/cover.jpg"},"genres":["Action"],"tags":["Adventure"],"average_score":80}],"page":1,"last_page":1,"total":1}"#;
const RECENT_FIXTURE: &str = r#"{"results":[{"id":"sample","title":{"romaji":"Sample Animetsu"},"status":"RELEASING","is_adult":false,"cover_image":{"large":"https://img.example/cover.jpg"}}],"current_page":1,"last_page":1}"#;
const DETAILS_FIXTURE: &str = r#"{"id":"sample","title":{"romaji":"Sample Animetsu","english":"Sample Animetsu"},"status":"FINISHED","is_adult":false,"cover_image":{"large":"https://img.example/cover.jpg"},"banner":"https://img.example/banner.jpg","description":"Sample <b>description</b>.","total_eps":2,"format":"TV","duration":24,"genres":["Action"],"tags":["Adventure"],"average_score":80,"studios":[{"name":"Sample Studio"}]}"#;
const EPISODES_FIXTURE: &str = r#"[{"ep_num":1,"name":"Arrival","desc":"First episode.","is_filler":false},{"ep_num":2,"name":"Lantern Street","desc":"Second episode.","is_filler":false}]"#;
const SERVERS_FIXTURE: &str = r#"[{"id":"baku","default":true},{"id":"kite","default":false}]"#;
const VIDEO_FIXTURE: &str = r#"{"sources":[{"quality":"1080p","url":"https://media.example/animetsu/master.m3u8","type":"application/x-mpegURL","need_proxy":false},{"quality":"720p","url":"https://media.example/animetsu/video.mp4","type":"video/mp4","need_proxy":false}],"subs":[{"url":"https://media.example/animetsu/en.vtt","lang":"English"}],"skips":{"intro":{"start":12.0,"end":85.0},"outro":{"start":1300.0,"end":1375.0}}}"#;

export_video_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_fixture() {
        let page = parse_search(SEARCH_FIXTURE, &json!({}));
        assert_eq!(page.entries[0].key, "sample");
    }

    #[test]
    fn parses_hoster_keys() {
        let hosters = parse_hosters(SERVERS_FIXTURE, &json!({}), "sample", "1");
        assert!(
            hosters
                .iter()
                .any(|hoster| hoster.key == "sample|1|baku|sub")
        );
    }

    #[test]
    fn parses_streams() {
        let dto: AnimetsuVideoDto = serde_json::from_str(VIDEO_FIXTURE).unwrap();
        let streams = parse_streams(dto, &json!({}), DEFAULT_BASE_URL, "baku", "sub");
        assert_eq!(streams.len(), 2);
        assert!(streams[0].subtitles.len() == 1);
    }
}
