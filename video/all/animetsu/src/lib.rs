use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Utc};
use manatan_sdk::{
    browser, client::Client, context, host, CatalogItem, Error, FilterDefinition,
    MediaResourceKind, MediaSegment, MediaTrack, OptionItem, Paged, PreferenceDefinition, Result,
    SegmentProcessing, SegmentRule, UrlResolveResult, VideoEpisode, VideoHoster, VideoSource,
    VideoStream,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;

const SOURCE_ID: &str = "animetsu";
const LANG: &str = "all";
const API_PATH: &str = "/v2/api";
const PROXY_URL: &str = "https://swiftstream.top/proxy";
const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/138 Safari/537.36";
const DOMAIN_HOSTS: &[&str] = &[
    "animetsu.net",
    "animetsu.live",
    "animetsu.bz",
    "animetsu.cc",
];
const AUDIO_VALUES: &[&str] = &["sub", "dub"];
const QUALITY_VALUES: &[&str] = &["1080", "720", "480", "360"];

const PREF_DOMAIN: &str = "preferred_domain";
const PREF_TITLE_LANG: &str = "preferred_title_lang";
const PREF_PREFERRED_SERVER: &str = "preferred_server";
const PREF_PREFERRED_AUDIO_TYPE: &str = "preferred_audio_type";
const PREF_AUDIO_TYPE_EXCLUDE: &str = "audio_type_exclusion";
const PREF_HOSTER_EXCLUDE: &str = "hoster_exclusion";
const PREF_QUALITY: &str = "preferred_quality";
const PREF_HIDE_ADULT: &str = "hide_adult_content";
const PREF_SHOW_EXTRA_INFO: &str = "show_extra_info";
const PREF_SHOW_RELATIONS: &str = "show_relations";
const PREF_SHOW_CHARACTERS: &str = "show_characters";
const PREF_SHOW_STAFF: &str = "show_staff";
const PREF_SHOW_TAGS: &str = "show_tags_in_genre";
const PREF_SHOW_TRACKERS: &str = "show_trackers";
const PREF_SHOW_TRAILER: &str = "show_trailer";
const PREF_SHOW_BANNER: &str = "show_banner";
const PREF_SHOW_EP_STATS: &str = "show_ep_stats";

#[derive(Default)]
struct AnimetsuSource;

impl AnimetsuSource {
    fn client(&self) -> Client {
        Client::browser()
    }

    fn base_url(&self) -> String {
        let configured = context::preference::<String>(PREF_DOMAIN)
            .ok()
            .flatten()
            .unwrap_or_else(|| "https://animetsu.net".to_string());
        let configured = configured.trim_end_matches('/').to_string();
        let selected_host = Url::parse(&configured)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned));
        if selected_host
            .as_deref()
            .is_some_and(|host| DOMAIN_HOSTS.contains(&host))
        {
            configured
        } else {
            "https://animetsu.net".to_string()
        }
    }

    fn api_url(&self) -> String {
        format!("{}{}", self.base_url(), API_PATH)
    }

    fn title_language(&self) -> String {
        context::preference::<String>(PREF_TITLE_LANG)
            .ok()
            .flatten()
            .filter(|value| matches!(value.as_str(), "romaji" | "english" | "native"))
            .unwrap_or_else(|| "romaji".to_string())
    }

    fn hide_adult(&self) -> bool {
        context::preference::<bool>(PREF_HIDE_ADULT)
            .ok()
            .flatten()
            .unwrap_or(true)
    }

    fn show_tags(&self) -> bool {
        pref_bool(PREF_SHOW_TAGS, true)
    }

    fn show_ep_stats(&self) -> bool {
        pref_bool(PREF_SHOW_EP_STATS, true)
    }

    fn metadata_prefs(&self) -> MetadataPreferences {
        MetadataPreferences {
            show_extra_info: pref_bool(PREF_SHOW_EXTRA_INFO, true),
            show_relations: pref_bool(PREF_SHOW_RELATIONS, true),
            show_characters: pref_bool(PREF_SHOW_CHARACTERS, true),
            show_staff: pref_bool(PREF_SHOW_STAFF, true),
            show_trackers: pref_bool(PREF_SHOW_TRACKERS, true),
            show_trailer: pref_bool(PREF_SHOW_TRAILER, true),
            show_banner: pref_bool(PREF_SHOW_BANNER, true),
        }
    }

    fn excluded_hosts(&self) -> Vec<String> {
        context::preference::<Vec<String>>(PREF_HOSTER_EXCLUDE)
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    fn preferred_server(&self) -> String {
        context::preference::<String>(PREF_PREFERRED_SERVER)
            .ok()
            .flatten()
            .unwrap_or_else(|| "none".to_string())
    }

    fn preferred_audio_type(&self) -> String {
        context::preference::<String>(PREF_PREFERRED_AUDIO_TYPE)
            .ok()
            .flatten()
            .unwrap_or_else(|| "none".to_string())
    }

    fn preferred_quality(&self) -> String {
        context::preference::<String>(PREF_QUALITY)
            .ok()
            .flatten()
            .unwrap_or_else(|| "1080".to_string())
    }

    fn enabled_audio_types(&self) -> Vec<String> {
        let excluded = context::preference::<Vec<String>>(PREF_AUDIO_TYPE_EXCLUDE)
            .ok()
            .flatten()
            .unwrap_or_default();
        AUDIO_VALUES
            .iter()
            .filter(|value| !excluded.iter().any(|item| item.eq_ignore_ascii_case(value)))
            .map(|value| (*value).to_string())
            .collect()
    }

    fn request_text(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        cookie_url: Option<&str>,
        rate_limit_key: &str,
        minimum_interval_ms: u32,
        max_body_bytes: Option<u64>,
    ) -> Result<(String, String)> {
        let mut request = self.client().get(url);
        if let Some(cookie_url) = cookie_url {
            request = request.cookies_for(cookie_url);
        }
        for (name, value) in headers.iter() {
            request = request.header(name.clone(), value.clone());
        }
        request = request
            .rate_limit(rate_limit_key, minimum_interval_ms)
            .timeout_ms(30_000);
        if let Some(limit) = max_body_bytes {
            request = request.max_body_bytes(limit);
        }
        let response = request.send()?;
        let response = if is_challenge_response(response.status(), response.text().ok()) {
            let cookie_url = cookie_url.unwrap_or(url);
            self.bootstrap_browser_session(url, cookie_url, &headers)?;
            let mut retry = self.client().get(url).timeout_ms(30_000);
            retry = retry.cookies_for(cookie_url);
            for (name, value) in headers.iter() {
                retry = retry.header(name.clone(), value.clone());
            }
            retry = retry.rate_limit(rate_limit_key, minimum_interval_ms);
            if let Some(limit) = max_body_bytes {
                retry = retry.max_body_bytes(limit);
            }
            let retry = retry.send()?;
            if is_challenge_response(retry.status(), retry.text().ok()) {
                return Err(Error::new(
                    "Cloudflare challenge still active after browser bootstrap; full live verification requires a host WebView session",
                ));
            }
            retry
        } else {
            response
        };
        Ok((response.text()?.to_owned(), response.final_url().to_owned()))
    }

    fn bootstrap_browser_session(
        &self,
        target_url: &str,
        cookie_url: &str,
        headers: &[(String, String)],
    ) -> Result<()> {
        let request = build_browser_bootstrap_request(target_url, cookie_url, headers);
        let response: browser::WebViewResponse = browser::open(&request)?;
        sync_webview_cookies(cookie_url, &response.cookies)?;
        if response.html.as_deref().is_some_and(is_challenge_html) {
            return Err(Error::new(
                "browser session reached a challenge page that could not be cleared headlessly",
            ));
        }
        Ok(())
    }

    fn api_headers(&self, referer: &str) -> Vec<(String, String)> {
        vec![
            ("Accept".into(), "application/json, text/plain, */*".into()),
            ("Accept-Language".into(), "en-US,en;q=0.9".into()),
            ("Referer".into(), referer.into()),
            ("Sec-Fetch-Dest".into(), "empty".into()),
            ("Sec-Fetch-Mode".into(), "cors".into()),
            ("Sec-Fetch-Site".into(), "same-origin".into()),
        ]
    }

    fn video_headers(&self, referer: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("Accept".into(), "*/*".into()),
            ("Accept-Language".into(), "en-US,en;q=0.9".into()),
            ("Referer".into(), referer.to_string()),
            ("Origin".into(), self.base_url()),
            ("Sec-Fetch-Dest".into(), "empty".into()),
            ("Sec-Fetch-Mode".into(), "cors".into()),
            ("Sec-Fetch-Site".into(), "cross-site".into()),
        ])
    }

    fn fetch_json<T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        referer: &str,
        rate_limit_key: &str,
    ) -> Result<T> {
        let (body, _) = self.request_text(
            url,
            self.api_headers(referer),
            Some(&self.base_url()),
            rate_limit_key,
            200,
            Some(4 * 1024 * 1024),
        )?;
        serde_json::from_str(&body).map_err(Error::from)
    }

    fn fetch_hls_variants(
        &self,
        url: &str,
        watch_referer: &str,
        rate_limit_key: &str,
    ) -> Result<Vec<HlsVariant>> {
        let headers = self
            .video_headers(watch_referer)
            .into_iter()
            .collect::<Vec<_>>();
        let (body, _) =
            self.request_text(url, headers, None, rate_limit_key, 200, Some(512 * 1024))?;
        parse_hls_variants(&body, url)
    }
}

impl VideoSource for AnimetsuSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let url = format!(
            "{}/anime/search/?sort=trending&page={}&per_page=35",
            self.api_url(),
            page.max(1)
        );
        let response: AnimetsuSearchResponse =
            self.fetch_json(&url, &format!("{}/browse", self.base_url()), "animetsu:api")?;
        Ok(to_search_page(
            response.results,
            response.page,
            response.last_page,
            &self.title_language(),
            self.show_tags(),
            self.hide_adult(),
            &self.base_url(),
        ))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let url = format!(
            "{}/anime/recent?page={}&per_page=35",
            self.api_url(),
            page.max(1)
        );
        let response: AnimetsuRecentResponse =
            self.fetch_json(&url, &format!("{}/browse", self.base_url()), "animetsu:api")?;
        Ok(to_recent_page(
            response.results,
            response.current_page,
            response.last_page,
            &self.title_language(),
            self.show_tags(),
            self.hide_adult(),
            &self.base_url(),
        ))
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let url = search_url(&self.api_url(), query, page, filters)?;
        let response: AnimetsuSearchResponse =
            self.fetch_json(&url, &format!("{}/browse", self.base_url()), "animetsu:api")?;
        Ok(to_search_page(
            response.results,
            response.page,
            response.last_page,
            &self.title_language(),
            self.show_tags(),
            self.hide_adult(),
            &self.base_url(),
        ))
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let id = item.key.clone();
        let url = format!("{}/anime/info/{}", self.api_url(), id);
        let referer = format!("{}/anime/{}", self.base_url(), id);
        let response: AnimetsuAnimeDto = self.fetch_json(&url, &referer, "animetsu:api")?;
        let mut mapped = response
            .to_catalog_item(
                &self.title_language(),
                self.show_tags(),
                &self.metadata_prefs(),
                &self.base_url(),
            )
            .ok_or_else(|| Error::new("Animetsu details response had no usable title"))?;
        mapped.key = item.key;
        mapped.url = Some(referer);
        Ok(mapped)
    }

    fn episodes(&mut self, item: CatalogItem) -> Result<Vec<VideoEpisode>> {
        let id = item.key.clone();
        let referer = format!("{}/anime/{}", self.base_url(), id);
        let url = format!("{}/anime/eps/{}", self.api_url(), id);
        let episodes: Vec<AnimetsuEpisodeDto> = self.fetch_json(&url, &referer, "animetsu:api")?;
        let mut mapped = episodes
            .into_iter()
            .filter_map(|episode| {
                episode.to_video_episode(
                    &id,
                    self.show_ep_stats(),
                    &self.base_url(),
                    item.language.as_deref().unwrap_or(LANG),
                )
            })
            .collect::<Vec<_>>();
        mapped.sort_by(|left, right| {
            right
                .episode_number
                .partial_cmp(&left.episode_number)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(mapped)
    }

    fn streams(&mut self, item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoStream>> {
        let hosters = self.hosters(item.clone(), episode.clone())?;
        let mut streams = Vec::new();
        let mut errors = Vec::new();
        for hoster in hosters {
            match self.hoster_streams(item.clone(), episode.clone(), hoster.clone()) {
                Ok(found) => streams.extend(found),
                Err(error) => errors.push(format!("{}: {error}", hoster.name)),
            }
        }
        if streams.is_empty() && !errors.is_empty() {
            return Err(Error::new(errors.join("; ")));
        }
        Ok(sort_streams(
            streams,
            &self.preferred_quality(),
            &self.preferred_server(),
            &self.preferred_audio_type(),
        ))
    }

    fn hosters(&mut self, _item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoHoster>> {
        let anime_id = episode_anime_id(&episode)?;
        let ep_number = episode
            .episode_number
            .ok_or_else(|| Error::new("Animetsu episode had no episode number"))?;
        let referer = format!("{}/watch/{}", self.base_url(), anime_id);
        let url = format!(
            "{}/anime/servers/{}/{}",
            self.api_url(),
            anime_id,
            number_label(ep_number)
        );
        let servers: Vec<AnimetsuServerDto> = self.fetch_json(&url, &referer, "animetsu:api")?;
        let excluded_hosts = self.excluded_hosts();
        let preferred_server = self.preferred_server();
        let preferred_audio = self.preferred_audio_type();
        let mut hosters = Vec::new();
        for server in servers.into_iter().filter(|server| {
            !excluded_hosts
                .iter()
                .any(|excluded| excluded.eq_ignore_ascii_case(&server.id))
        }) {
            for audio_type in self.enabled_audio_types() {
                let data = HosterData {
                    anime_id: anime_id.to_string(),
                    episode_number: number_label(ep_number),
                    server_id: server.id.clone(),
                    audio_type: audio_type.clone(),
                    watch_referer: referer.clone(),
                };
                hosters.push(VideoHoster {
                    key: format!("{}:{}", server.id, audio_type),
                    name: format!(
                        "{} - {}",
                        format_server_name(&server.id),
                        audio_label(&audio_type)
                    ),
                    lazy: true,
                    internal_data: Some(serde_json::to_string(&data)?),
                    extra: BTreeMap::from([
                        (
                            "preferredServer".into(),
                            json!(server.id.eq_ignore_ascii_case(&preferred_server)),
                        ),
                        (
                            "preferredAudio".into(),
                            json!(audio_type.eq_ignore_ascii_case(&preferred_audio)),
                        ),
                        ("tip".into(), json!(server.tip)),
                    ]),
                    ..VideoHoster::default()
                });
            }
        }
        hosters.sort_by_key(|hoster| {
            let server_match = hoster
                .key
                .split(':')
                .next()
                .is_some_and(|value| value.eq_ignore_ascii_case(&preferred_server));
            let audio_match = hoster
                .key
                .split(':')
                .nth(1)
                .is_some_and(|value| value.eq_ignore_ascii_case(&preferred_audio));
            (!server_match, !audio_match, hoster.name.clone())
        });
        Ok(hosters)
    }

    fn hoster_streams(
        &mut self,
        _item: CatalogItem,
        _episode: VideoEpisode,
        hoster: VideoHoster,
    ) -> Result<Vec<VideoStream>> {
        let data: HosterData = serde_json::from_str(
            hoster
                .internal_data
                .as_deref()
                .ok_or_else(|| Error::new("Animetsu hoster missing internal data"))?,
        )?;
        let url = format!(
            "{}/anime/oppai/{}/{}?server={}&source_type={}",
            self.api_url(),
            data.anime_id,
            data.episode_number,
            data.server_id,
            data.audio_type
        );
        let payload: AnimetsuVideoDto =
            self.fetch_json(&url, &data.watch_referer, "animetsu:api")?;
        streams_from_payload(
            &payload,
            &data,
            &self.base_url(),
            &self.preferred_server(),
            &self.preferred_audio_type(),
            &self.preferred_quality(),
            |playlist_url| {
                self.fetch_hls_variants(playlist_url, &data.watch_referer, "animetsu:swiftstream")
            },
        )
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(animetsu_filters())
    }

    fn preferences(&mut self) -> Result<Vec<PreferenceDefinition>> {
        Ok(animetsu_preferences())
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(format!("{}/anime/{}", self.base_url(), item.key)))
    }

    fn episode_url(
        &mut self,
        item: &CatalogItem,
        episode: &VideoEpisode,
    ) -> Result<Option<String>> {
        let episode_number = episode
            .episode_number
            .ok_or_else(|| Error::new("Animetsu episode had no episode number"))?;
        Ok(Some(format!(
            "{}/watch/{}/{}",
            self.base_url(),
            item.key,
            number_label(episode_number)
        )))
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        handle_url(candidate, &self.base_url())
    }
}

#[derive(Clone, Debug, Default)]
struct MetadataPreferences {
    show_extra_info: bool,
    show_relations: bool,
    show_characters: bool,
    show_staff: bool,
    show_trackers: bool,
    show_trailer: bool,
    show_banner: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuSearchResponse {
    results: Vec<AnimetsuAnimeDto>,
    page: u32,
    #[serde(rename = "last_page")]
    last_page: u32,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuRecentResponse {
    results: Vec<AnimetsuAnimeDto>,
    #[serde(rename = "current_page")]
    current_page: u32,
    #[serde(rename = "last_page")]
    last_page: u32,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuAnimeDto {
    id: String,
    #[serde(default)]
    title: Option<AnimetsuTitleDto>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "is_adult")]
    is_adult: bool,
    #[serde(default, rename = "cover_image")]
    cover_image: Option<AnimetsuCoverDto>,
    #[serde(default)]
    banner: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "total_eps")]
    total_eps: Option<i32>,
    #[serde(default, rename = "start_date")]
    start_date: Option<String>,
    #[serde(default, rename = "end_date")]
    end_date: Option<String>,
    #[serde(default, rename = "next_airing_ep")]
    next_airing_ep: Option<AnimetsuNextAiringEpisodeDto>,
    #[serde(default)]
    rank: Option<i32>,
    #[serde(default)]
    year: Option<i32>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    duration: Option<i32>,
    #[serde(default)]
    genres: Option<Vec<String>>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default, rename = "average_score")]
    average_score: Option<i32>,
    #[serde(default)]
    trailer: Option<String>,
    #[serde(default)]
    season: Option<String>,
    #[serde(default)]
    seasons: Option<Vec<AnimetsuSeasonDto>>,
    #[serde(default, rename = "anilist_id")]
    anilist_id: Option<i32>,
    #[serde(default, rename = "mal_id")]
    mal_id: Option<i32>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    hashtag: Option<String>,
    #[serde(default, rename = "mean_score")]
    mean_score: Option<i32>,
    #[serde(default)]
    popularity: Option<i32>,
    #[serde(default)]
    favourites: Option<i32>,
    #[serde(default)]
    trending: Option<i32>,
    #[serde(default)]
    synonyms: Option<Vec<String>>,
    #[serde(default)]
    studios: Option<Vec<AnimetsuStudioDto>>,
    #[serde(default)]
    relations: Option<Vec<AnimetsuRelationDto>>,
    #[serde(default)]
    characters: Option<Vec<AnimetsuCharacterDto>>,
    #[serde(default)]
    recommendations: Option<Vec<AnimetsuRecommendationDto>>,
    #[serde(default)]
    staff: Option<Vec<AnimetsuStaffDto>>,
    #[serde(default)]
    users: Option<i32>,
}

impl AnimetsuAnimeDto {
    fn to_catalog_item(
        &self,
        title_language: &str,
        show_tags: bool,
        metadata: &MetadataPreferences,
        base_url: &str,
    ) -> Option<CatalogItem> {
        let title = self.title.as_ref()?.preferred_title(title_language)?;
        let mut item = CatalogItem::new(self.id.clone(), title);
        item.url = Some(format!("{base_url}/anime/{}", self.id));
        item.cover = self
            .cover_image
            .as_ref()
            .and_then(|cover| {
                cover
                    .large
                    .clone()
                    .or(cover.medium.clone())
                    .or(cover.small.clone())
            })
            .map(Into::into);
        item.banner = metadata
            .show_banner
            .then(|| self.banner.as_deref())
            .flatten()
            .and_then(|banner| absolute_url(base_url, banner).ok())
            .map(Into::into);
        item.tags = self
            .genres
            .clone()
            .unwrap_or_default()
            .into_iter()
            .chain(
                show_tags
                    .then(|| self.tags.clone().unwrap_or_default())
                    .unwrap_or_default(),
            )
            .collect();
        item.status = Some(json!(status_label(self.status.as_deref())));
        item.language = Some(LANG.to_string());
        item.rating = self.average_score.map(|score| score as f32 / 20.0);
        item.content_rating = self.is_adult.then(|| "adult".to_string());
        item.authors = self
            .studios
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|studio| studio.name)
            .collect();
        item.artists = self
            .staff
            .clone()
            .unwrap_or_default()
            .into_iter()
            .filter(|staff| {
                matches!(
                    staff.role.as_deref(),
                    Some("Original Story" | "Original Creator" | "Original Character Design")
                )
            })
            .filter_map(|staff| staff.name)
            .collect();
        item.description = Some(build_description(self, metadata, base_url));
        item.initialized = true;
        item.extra = BTreeMap::from([
            ("adult".into(), json!(self.is_adult)),
            ("anilistId".into(), json!(self.anilist_id)),
            ("malId".into(), json!(self.mal_id)),
        ]);
        Some(item)
    }
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuTitleDto {
    #[serde(default)]
    romaji: Option<String>,
    #[serde(default)]
    english: Option<String>,
    #[serde(default)]
    native: Option<String>,
}

impl AnimetsuTitleDto {
    fn preferred_title(&self, language: &str) -> Option<String> {
        match language {
            "english" => self.english.clone(),
            "native" => self.native.clone(),
            _ => self.romaji.clone(),
        }
        .filter(|value| !value.trim().is_empty())
        .or_else(|| self.romaji.clone().filter(|value| !value.trim().is_empty()))
        .or_else(|| {
            self.english
                .clone()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| self.native.clone().filter(|value| !value.trim().is_empty()))
    }
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuCoverDto {
    #[serde(default)]
    large: Option<String>,
    #[serde(default)]
    medium: Option<String>,
    #[serde(default)]
    small: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuNextAiringEpisodeDto {
    #[serde(default, rename = "airing_at")]
    airing_at: Option<i64>,
    #[serde(default, rename = "ep_num")]
    ep_num: Option<i32>,
    #[serde(default, rename = "time_left")]
    time_left: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuSeasonDto {
    id: String,
    #[serde(default)]
    title: Option<AnimetsuTitleDto>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuEpisodeDto {
    #[serde(default, rename = "ep_num")]
    ep_num: Option<f32>,
    #[serde(default, rename = "aired_at")]
    aired_at: Option<String>,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default, rename = "is_filler")]
    is_filler: Option<bool>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    id: String,
    #[serde(default)]
    likes: Option<i32>,
    #[serde(default)]
    dislikes: Option<i32>,
    #[serde(default)]
    views: Option<i32>,
    #[serde(default)]
    img: Option<String>,
}

impl AnimetsuEpisodeDto {
    fn to_video_episode(
        self,
        anime_id: &str,
        show_stats: bool,
        base_url: &str,
        language: &str,
    ) -> Option<VideoEpisode> {
        let number = self.ep_num?;
        if number <= 0.0 {
            return None;
        }
        let label = number_label(number);
        let title = self
            .name
            .filter(|value| !value.trim().is_empty())
            .map(|value| format!("Ep. {label} - {value}"))
            .unwrap_or_else(|| format!("Ep. {label}"));
        let filler = self.is_filler.unwrap_or(false);
        Some(VideoEpisode {
            key: format!("{anime_id}/{label}"),
            title: Some(if filler {
                format!("{title} (Filler)")
            } else {
                title
            }),
            description: clean_html(self.desc.as_deref()),
            episode_number: Some(number),
            date_uploaded: parse_iso_datetime(self.aired_at.as_deref()),
            thumbnail: self
                .img
                .as_deref()
                .and_then(|value| absolute_url(base_url, value).ok())
                .map(Into::into),
            url: Some(format!("{base_url}/watch/{anime_id}/{label}")),
            release_group: show_stats
                .then(|| format_episode_stats(self.views, self.likes, self.dislikes)),
            language: Some(language.to_string()),
            is_filler: filler,
            extra: BTreeMap::from([
                ("animeId".into(), json!(anime_id)),
                ("episodeId".into(), json!(self.id)),
            ]),
            ..VideoEpisode::default()
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuServerDto {
    id: String,
    #[serde(default)]
    tip: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuVideoDto {
    sources: Vec<AnimetsuSourceDto>,
    #[serde(default)]
    subs: Option<Vec<AnimetsuSubDto>>,
    #[serde(default)]
    skips: Option<AnimetsuSkipsDto>,
    #[serde(default)]
    server: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuSourceDto {
    quality: String,
    url: String,
    #[serde(default)]
    #[allow(dead_code)]
    old_hls: bool,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default, rename = "need_proxy")]
    need_proxy: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuSubDto {
    url: String,
    #[serde(default)]
    lang: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuSkipsDto {
    #[serde(default)]
    intro: Option<AnimetsuSkipTimeDto>,
    #[serde(default)]
    outro: Option<AnimetsuSkipTimeDto>,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuSkipTimeDto {
    start: f64,
    end: f64,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuStudioDto {
    name: String,
    #[serde(default, rename = "is_main")]
    is_main: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuRelationDto {
    id: String,
    #[serde(default)]
    title: Option<AnimetsuTitleDto>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    season: Option<String>,
    #[serde(default)]
    year: Option<i32>,
    #[serde(default, rename = "relation_type")]
    relation_type: Option<String>,
    #[serde(default, rename = "total_eps")]
    total_eps: Option<i32>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuCharacterDto {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default, rename = "voice_actor")]
    voice_actor: Option<AnimetsuVoiceActorDto>,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuVoiceActorDto {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    language: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuRecommendationDto {
    id: String,
    #[serde(default)]
    title: Option<AnimetsuTitleDto>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    season: Option<String>,
    #[serde(default)]
    year: Option<i32>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AnimetsuStaffDto {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    role: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct HosterData {
    anime_id: String,
    episode_number: String,
    server_id: String,
    audio_type: String,
    watch_referer: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HlsVariant {
    url: String,
    quality: String,
}

fn to_search_page(
    results: Vec<AnimetsuAnimeDto>,
    page: u32,
    last_page: u32,
    title_language: &str,
    show_tags: bool,
    hide_adult: bool,
    base_url: &str,
) -> Paged<CatalogItem> {
    Paged::new(
        results
            .into_iter()
            .filter(|item| !hide_adult || !item.is_adult)
            .filter_map(|item| {
                item.to_catalog_item(
                    title_language,
                    show_tags,
                    &MetadataPreferences {
                        show_extra_info: false,
                        show_relations: false,
                        show_characters: false,
                        show_staff: false,
                        show_trackers: false,
                        show_trailer: false,
                        show_banner: false,
                    },
                    base_url,
                )
            })
            .collect(),
        page < last_page,
    )
}

fn to_recent_page(
    results: Vec<AnimetsuAnimeDto>,
    page: u32,
    last_page: u32,
    title_language: &str,
    show_tags: bool,
    hide_adult: bool,
    base_url: &str,
) -> Paged<CatalogItem> {
    to_search_page(
        results,
        page,
        last_page,
        title_language,
        show_tags,
        hide_adult,
        base_url,
    )
}

fn build_description(
    item: &AnimetsuAnimeDto,
    metadata: &MetadataPreferences,
    base_url: &str,
) -> String {
    let mut blocks = Vec::new();
    if let Some(summary) = score_summary(item.average_score, item.rank, item.trending) {
        blocks.push(summary);
    }
    if let Some(description) = clean_html(item.description.as_deref()) {
        blocks.push(description);
    }
    if metadata.show_extra_info {
        let mut extra = Vec::new();
        if let Some(format) = item.format.as_deref() {
            extra.push(title_case(format));
        }
        if let Some(status) = item.status.as_deref() {
            extra.push(status_display(status).to_string());
        }
        if let Some(total) = item.total_eps {
            extra.push(format!("Episodes: {total}"));
        }
        if let Some(duration) = item.duration {
            extra.push(format!("Duration: {duration} min"));
        }
        if let Some(season) = item.season.as_deref() {
            extra.push(
                item.year
                    .map(|year| format!("{} {year}", title_case(season)))
                    .unwrap_or_else(|| title_case(season)),
            );
        }
        if let Some(country) = item.country.as_deref() {
            extra.push(format!("Country: {country}"));
        }
        if let Some(source) = item.source.as_deref() {
            extra.push(format!("Source: {}", title_case(&source.replace('_', " "))));
        }
        if !extra.is_empty() {
            blocks.push(extra.join(" | "));
        }

        let mut schedule = Vec::new();
        if let Some(start) = item.start_date.as_deref() {
            schedule.push(format!("Start: {start}"));
        }
        if let Some(end) = item.end_date.as_deref() {
            schedule.push(format!("End: {end}"));
        }
        if let Some(next) = item.next_airing_ep.as_ref().and_then(format_next_airing) {
            schedule.push(next);
        }
        if !schedule.is_empty() {
            blocks.push(schedule.join(" | "));
        }

        if let Some(synonyms) = item.synonyms.as_ref().filter(|values| !values.is_empty()) {
            blocks.push(format!("Synonyms: {}", synonyms.join(", ")));
        }
        if let Some(hashtag) = item.hashtag.as_deref().filter(|value| !value.is_empty()) {
            blocks.push(format!("Hashtag: {hashtag}"));
        }
        if let Some(mean) = item
            .mean_score
            .filter(|mean| Some(*mean) != item.average_score)
        {
            blocks.push(format!("Mean Score: {mean}/100"));
        }

        let mut stats = Vec::new();
        if let Some(popularity) = item.popularity {
            stats.push(format!("Popularity: {popularity}"));
        }
        if let Some(favourites) = item.favourites {
            stats.push(format!("Favourites: {favourites}"));
        }
        if let Some(users) = item.users {
            stats.push(format!("Bookmarked: {users}"));
        }
        if !stats.is_empty() {
            blocks.push(stats.join(" | "));
        }

        if let Some(studios) = item.studios.as_ref().filter(|values| !values.is_empty()) {
            let main = studios
                .iter()
                .find(|studio| studio.is_main)
                .map(|studio| studio.name.clone());
            let others = studios
                .iter()
                .filter(|studio| !studio.is_main)
                .map(|studio| studio.name.clone())
                .collect::<Vec<_>>();
            let studio_line = match main {
                Some(main) if !others.is_empty() => {
                    format!("Studio: {main} ({})", others.join(", "))
                }
                Some(main) => format!("Studio: {main}"),
                None => format!(
                    "Studio: {}",
                    studios
                        .iter()
                        .map(|studio| studio.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            blocks.push(studio_line);
        }
    }

    if metadata.show_relations {
        if let Some(relations) = item.relations.as_ref().filter(|values| !values.is_empty()) {
            let relation_lines = relations
                .iter()
                .map(|relation| {
                    let title = relation
                        .title
                        .as_ref()
                        .and_then(|title| {
                            title
                                .english
                                .clone()
                                .or(title.romaji.clone())
                                .or(title.native.clone())
                        })
                        .unwrap_or_else(|| relation.id.clone());
                    let mut info = Vec::new();
                    if let Some(format) = relation.format.as_deref() {
                        info.push(title_case(format));
                    }
                    if let Some(season) = relation.season.as_deref() {
                        info.push(
                            relation
                                .year
                                .map(|year| format!("{} {year}", title_case(season)))
                                .unwrap_or_else(|| title_case(season)),
                        );
                    }
                    if let Some(kind) = relation.relation_type.as_deref() {
                        info.push(title_case(&kind.replace('_', " ")));
                    }
                    if info.is_empty() {
                        format!("- {title}")
                    } else {
                        format!("- {title} ({})", info.join(", "))
                    }
                })
                .collect::<Vec<_>>();
            if !relation_lines.is_empty() {
                blocks.push(format!("Relations:\n{}", relation_lines.join("\n")));
            }
        }
    }

    if metadata.show_characters {
        if let Some(characters) = item.characters.as_ref().filter(|values| !values.is_empty()) {
            let lines = characters
                .iter()
                .filter(|character| character.role.as_deref() == Some("MAIN"))
                .filter_map(|character| {
                    let name = character.name.as_deref()?;
                    let voice = character
                        .voice_actor
                        .as_ref()
                        .and_then(|actor| actor.name.as_deref())
                        .unwrap_or("Unknown");
                    let language = character
                        .voice_actor
                        .as_ref()
                        .and_then(|actor| actor.language.as_deref())
                        .unwrap_or("Unknown");
                    Some(format!("- {name} (VA: {voice} / {language})"))
                })
                .collect::<Vec<_>>();
            if !lines.is_empty() {
                blocks.push(format!("Main Characters:\n{}", lines.join("\n")));
            }
        }
    }

    if metadata.show_staff {
        if let Some(staff) = item.staff.as_ref().filter(|values| !values.is_empty()) {
            let lines = staff
                .iter()
                .filter_map(|staff| {
                    Some(format!(
                        "- {}: {}",
                        staff.role.as_deref()?,
                        staff.name.as_deref()?
                    ))
                })
                .collect::<Vec<_>>();
            if !lines.is_empty() {
                blocks.push(format!("Staff:\n{}", lines.join("\n")));
            }
        }
    }

    if metadata.show_trackers {
        let mut links = Vec::new();
        if let Some(anilist) = item.anilist_id {
            links.push(format!("[AniList](https://anilist.co/anime/{anilist})"));
        }
        if let Some(mal) = item.mal_id {
            links.push(format!("[MAL](https://myanimelist.net/anime/{mal})"));
        }
        if !links.is_empty() {
            blocks.push(links.join(" | "));
        }
    }

    if metadata.show_trailer {
        if let Some(trailer) = item
            .trailer
            .as_deref()
            .filter(|value| !value.is_empty() && *value != "-")
        {
            blocks.push(format!(
                "[Trailer](https://www.youtube.com/watch?v={trailer})"
            ));
        }
    }

    if metadata.show_banner {
        if let Some(banner) = item
            .banner
            .as_deref()
            .and_then(|banner| absolute_url(base_url, banner).ok())
        {
            blocks.push(format!("Banner: {banner}"));
        }
    }

    blocks
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn streams_from_payload<F>(
    payload: &AnimetsuVideoDto,
    data: &HosterData,
    base_url: &str,
    preferred_server: &str,
    preferred_audio: &str,
    preferred_quality: &str,
    fetch_hls: F,
) -> Result<Vec<VideoStream>>
where
    F: Fn(&str) -> Result<Vec<HlsVariant>>,
{
    let subtitles = payload
        .subs
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|subtitle| {
            let url = if subtitle.url.starts_with("http") {
                subtitle.url
            } else {
                absolute_url(base_url, &subtitle.url).unwrap_or(subtitle.url)
            };
            MediaTrack {
                url,
                language: subtitle.lang.clone(),
                label: subtitle.lang,
                headers: video_headers_for(base_url, &data.watch_referer),
                ..MediaTrack::default()
            }
        })
        .collect::<Vec<_>>();
    let intro = payload
        .skips
        .as_ref()
        .and_then(|skips| to_segment(skips.intro.as_ref()));
    let outro = payload
        .skips
        .as_ref()
        .and_then(|skips| to_segment(skips.outro.as_ref()));
    let mut streams = Vec::new();
    for source in &payload.sources {
        let full_url = if source.need_proxy {
            format!("{PROXY_URL}{}", source.url)
        } else if source.url.starts_with("http://") || source.url.starts_with("https://") {
            source.url.clone()
        } else {
            format!("{base_url}{}", source.url)
        };
        if is_mp4_type(source) {
            let quality = normalize_quality_label(&source.quality);
            streams.push(VideoStream {
                url: full_url.clone(),
                name: Some(stream_name(
                    &data.server_id,
                    &data.audio_type,
                    &quality,
                    is_softsub(payload),
                )),
                quality: Some(quality.clone()),
                format: Some("mp4".to_string()),
                headers: video_headers_for(base_url, &data.watch_referer),
                subtitles: subtitles.clone(),
                intro: intro.clone(),
                outro: outro.clone(),
                preferred: is_preferred_stream(
                    &data.server_id,
                    &data.audio_type,
                    &quality,
                    preferred_server,
                    preferred_audio,
                    preferred_quality,
                ),
                initialized: true,
                ..VideoStream::default()
            });
            continue;
        }
        if is_hls_type(source, &full_url) {
            let variants = fetch_hls(&full_url).unwrap_or_default();
            if variants.is_empty() {
                let quality = normalize_quality_label(&source.quality);
                streams.push(VideoStream {
                    url: full_url.clone(),
                    name: Some(stream_name(
                        &data.server_id,
                        &data.audio_type,
                        "Auto",
                        is_softsub(payload),
                    )),
                    quality: Some(quality),
                    format: Some("hls".to_string()),
                    is_hls: true,
                    headers: video_headers_for(base_url, &data.watch_referer),
                    subtitles: subtitles.clone(),
                    intro: intro.clone(),
                    outro: outro.clone(),
                    segment_processing: Some(animetsu_segment_processing()),
                    initialized: true,
                    ..VideoStream::default()
                });
            } else {
                for variant in variants {
                    streams.push(VideoStream {
                        url: variant.url.clone(),
                        name: Some(stream_name(
                            &data.server_id,
                            &data.audio_type,
                            &variant.quality,
                            is_softsub(payload),
                        )),
                        quality: Some(variant.quality.clone()),
                        format: Some("hls".to_string()),
                        is_hls: true,
                        headers: video_headers_for(base_url, &data.watch_referer),
                        subtitles: subtitles.clone(),
                        intro: intro.clone(),
                        outro: outro.clone(),
                        preferred: is_preferred_stream(
                            &data.server_id,
                            &data.audio_type,
                            &variant.quality,
                            preferred_server,
                            preferred_audio,
                            preferred_quality,
                        ),
                        segment_processing: Some(animetsu_segment_processing()),
                        initialized: true,
                        ..VideoStream::default()
                    });
                }
            }
        }
    }
    Ok(streams)
}

fn build_browser_bootstrap_request(
    target_url: &str,
    cookie_url: &str,
    headers: &[(String, String)],
) -> browser::WebViewRequest {
    browser::WebViewRequest {
        url: target_url.to_string(),
        cookie_url: Some(cookie_url.to_string()),
        wait_for: Some(browser::WebViewWait::Delay {
            milliseconds: 6_000,
        }),
        wait_until: Some(browser::WebViewWaitUntil::NetworkIdle),
        user_agent: Some(BROWSER_USER_AGENT.to_string()),
        headers: headers.to_vec(),
        timeout_ms: Some(45_000),
        return_html: true,
        ..browser::WebViewRequest::default()
    }
}

fn sync_webview_cookies(cookie_url: &str, cookies: &[browser::WebViewCookie]) -> Result<()> {
    if cookies.is_empty() {
        return Ok(());
    }
    let mapped = cookies
        .iter()
        .map(|cookie| manatan_sdk::cookies::Cookie {
            name: cookie.name.clone(),
            value: cookie.value.clone(),
            domain: cookie.domain.clone(),
            path: cookie.path.clone(),
            secure: cookie.secure.unwrap_or(false),
            http_only: cookie.http_only.unwrap_or(false),
            expires_at: cookie.expires_at,
        })
        .collect::<Vec<_>>();
    manatan_sdk::cookies::set(cookie_url, &mapped).map_err(Error::new)
}

fn parse_hls_variants(source: &str, playlist_url: &str) -> Result<Vec<HlsVariant>> {
    let Some(index) = source.find("#EXTM3U") else {
        return Ok(Vec::new());
    };
    let source = &source[index..];
    let resolution = Regex::new(r#"RESOLUTION=\d+x(\d+)"#).unwrap();
    let name = Regex::new(r#"NAME="([^"]+)""#).unwrap();
    let mut variants = Vec::new();
    let mut pending_quality = None::<String>;
    for line in source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if line.starts_with("#EXT-X-STREAM-INF:") {
            pending_quality = resolution
                .captures(line)
                .and_then(|captures| captures.get(1))
                .map(|capture| format!("{}p", capture.as_str()))
                .or_else(|| {
                    name.captures(line)
                        .and_then(|captures| captures.get(1))
                        .map(|capture| capture.as_str().to_string())
                })
                .or_else(|| Some("Auto".to_string()));
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if let Some(quality) = pending_quality.take() {
            variants.push(HlsVariant {
                url: absolute_url(playlist_url, line)?,
                quality,
            });
        }
    }
    Ok(variants)
}

fn animetsu_segment_processing() -> SegmentProcessing {
    SegmentProcessing {
        rewrite_playlist: true,
        max_resource_bytes: Some(64 * 1024 * 1024),
        rules: vec![SegmentRule {
            resource_types: vec![MediaResourceKind::Playlist, MediaResourceKind::Segment],
            auto_detect_media_offset: true,
            probe_bytes: Some(4096),
            ..SegmentRule::default()
        }],
        ..SegmentProcessing::default()
    }
}

fn search_url(api_url: &str, query: &str, page: u32, filters: &Value) -> Result<String> {
    let mut url = Url::parse(&format!("{api_url}/anime/search/"))
        .map_err(|error| Error::new(error.to_string()))?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("page", &page.max(1).to_string());
        pairs.append_pair("per_page", "35");
        if !query.trim().is_empty() {
            pairs.append_pair("query", query.trim());
        }
        for key in [
            "sort", "format", "status", "season", "year", "country", "source",
        ] {
            if let Some(value) = filter_string(filters, key) {
                pairs.append_pair(key, &value);
            }
        }
        if let Some(values) = filter_list(filters, "genres").filter(|value| !value.is_empty()) {
            pairs.append_pair("genres", &values.join(","));
        }
        if let Some(values) = filter_list(filters, "tags").filter(|value| !value.is_empty()) {
            pairs.append_pair("tags", &values.join(","));
        }
    }
    Ok(url.to_string())
}

fn animetsu_filters() -> Vec<FilterDefinition> {
    let current_year = DateTime::<Utc>::from_timestamp(host::now_millis() / 1_000, 0)
        .map(|time| time.year())
        .unwrap_or(2026)
        .max(2026)
        + 1;
    vec![
        select_filter(
            "sort",
            "Sort By",
            &[
                ("Popularity", "popularity"),
                ("Average Score", "average_score"),
                ("Release Date", "date_desc"),
                ("Favourites", "favourites"),
                ("Trending", "trending"),
            ],
        ),
        select_filter(
            "format",
            "Format",
            &[
                ("Any", ""),
                ("Movie", "MOVIE"),
                ("TV", "TV"),
                ("TV Short", "TV_SHORT"),
                ("Special", "SPECIAL"),
                ("OVA", "OVA"),
                ("ONA", "ONA"),
            ],
        ),
        select_filter(
            "status",
            "Airing Status",
            &[
                ("Any", ""),
                ("Ongoing", "RELEASING"),
                ("Finished", "FINISHED"),
                ("Upcoming", "NOT_YET_RELEASED"),
                ("Cancelled", "CANCELLED"),
            ],
        ),
        select_filter(
            "season",
            "Season",
            &[
                ("Any", ""),
                ("Winter", "WINTER"),
                ("Spring", "SPRING"),
                ("Summer", "SUMMER"),
                ("Fall", "FALL"),
            ],
        ),
        FilterDefinition::Group {
            id: "year".into(),
            name: "Year".into(),
            filters: std::iter::once(FilterDefinition::CheckBox {
                id: "".into(),
                name: "Any".into(),
                default: true,
            })
            .chain(
                (1970..=current_year)
                    .rev()
                    .map(|year| FilterDefinition::CheckBox {
                        id: year.to_string(),
                        name: year.to_string(),
                        default: false,
                    }),
            )
            .collect(),
        },
        select_filter(
            "country",
            "Country",
            &[
                ("Any", ""),
                ("Japan", "JP"),
                ("China", "CN"),
                ("Korea", "KR"),
                ("Taiwan", "TW"),
            ],
        ),
        select_filter(
            "source",
            "Source",
            &[
                ("Any", ""),
                ("Original", "ORIGINAL"),
                ("Manga", "MANGA"),
                ("Light Novel", "LIGHT_NOVEL"),
                ("Visual Novel", "VISUAL_NOVEL"),
                ("Video Game", "VIDEO_GAME"),
                ("Novel", "NOVEL"),
                ("Web Novel", "WEB_NOVEL"),
            ],
        ),
        checkbox_group("genres", "Genres", GENRES),
        checkbox_group("tags", "Tags", TAGS),
    ]
}

fn animetsu_preferences() -> Vec<PreferenceDefinition> {
    vec![
        PreferenceDefinition::Select {
            key: PREF_DOMAIN.into(),
            title: "Preferred Domain".into(),
            options: DOMAIN_HOSTS
                .iter()
                .map(|host| OptionItem {
                    label: (*host).into(),
                    value: format!("https://{host}"),
                })
                .collect(),
            default: "https://animetsu.net".into(),
        },
        PreferenceDefinition::Select {
            key: PREF_TITLE_LANG.into(),
            title: "Preferred Title Language".into(),
            options: vec![
                option("Romaji", "romaji"),
                option("English", "english"),
                option("Japanese (Native)", "native"),
            ],
            default: "romaji".into(),
        },
        PreferenceDefinition::Select {
            key: PREF_QUALITY.into(),
            title: "Preferred Quality".into(),
            options: QUALITY_VALUES
                .iter()
                .map(|value| option(&format!("{value}p"), value))
                .collect(),
            default: "1080".into(),
        },
        PreferenceDefinition::Select {
            key: PREF_PREFERRED_SERVER.into(),
            title: "Preferred Host".into(),
            options: vec![
                option("None", "none"),
                option("Kite - Multi Quality", "kite"),
                option("Dio - Multi Quality", "dio"),
                option("Sage - Multi Quality", "sage"),
                option("Meg - Multi Quality", "meg"),
            ],
            default: "none".into(),
        },
        PreferenceDefinition::Select {
            key: PREF_PREFERRED_AUDIO_TYPE.into(),
            title: "Preferred Audio Type".into(),
            options: vec![option("None", "none"), option("Sub", "sub"), option("Dub", "dub")],
            default: "none".into(),
        },
        PreferenceDefinition::MultiSelect {
            key: PREF_HOSTER_EXCLUDE.into(),
            title: "Exclude Hosts".into(),
            summary: Some("Choose which hosts you want to exclude".into()),
            options: vec![
                option("Kite - Multi Quality", "kite"),
                option("Dio - Multi Quality", "dio"),
                option("Sage - Multi Quality", "sage"),
                option("Meg - Multi Quality", "meg"),
            ],
            default: Vec::new(),
        },
        PreferenceDefinition::MultiSelect {
            key: PREF_AUDIO_TYPE_EXCLUDE.into(),
            title: "Exclude Audio Types".into(),
            summary: Some("Choose which audio types you want to exclude".into()),
            options: vec![option("Sub", "sub"), option("Dub", "dub")],
            default: Vec::new(),
        },
        PreferenceDefinition::Switch {
            key: PREF_HIDE_ADULT.into(),
            title: "Hide Adult Content".into(),
            summary: Some("Hides 18+ content from browse, search, and latest updates.".into()),
            default: true,
        },
        PreferenceDefinition::Switch {
            key: PREF_SHOW_EXTRA_INFO.into(),
            title: "Show Extra Info".into(),
            summary: Some("Shows extra information of a series in the description.".into()),
            default: true,
        },
        PreferenceDefinition::Switch {
            key: PREF_SHOW_RELATIONS.into(),
            title: "Show Relations".into(),
            summary: Some("Shows related anime in the description.".into()),
            default: true,
        },
        PreferenceDefinition::Switch {
            key: PREF_SHOW_CHARACTERS.into(),
            title: "Show Characters".into(),
            summary: Some("Shows main characters and voice actors in the description.".into()),
            default: true,
        },
        PreferenceDefinition::Switch {
            key: PREF_SHOW_STAFF.into(),
            title: "Show Staff".into(),
            summary: Some("Shows staff information in the description.".into()),
            default: true,
        },
        PreferenceDefinition::Switch {
            key: PREF_SHOW_TAGS.into(),
            title: "Show Tags in Genre".into(),
            summary: Some("Appends community tags to the genre field.".into()),
            default: true,
        },
        PreferenceDefinition::Switch {
            key: PREF_SHOW_TRACKERS.into(),
            title: "Show Tracker Links".into(),
            summary: Some("Shows AniList and MyAnimeList links in the description.".into()),
            default: true,
        },
        PreferenceDefinition::Switch {
            key: PREF_SHOW_TRAILER.into(),
            title: "Show Trailer".into(),
            summary: Some("Shows the YouTube trailer link in the description.".into()),
            default: true,
        },
        PreferenceDefinition::Switch {
            key: PREF_SHOW_BANNER.into(),
            title: "Show Banner".into(),
            summary: Some("Shows the banner URL in metadata and the banner field.".into()),
            default: true,
        },
        PreferenceDefinition::Switch {
            key: PREF_SHOW_EP_STATS.into(),
            title: "Show Episode Stats".into(),
            summary: Some("Shows Views, Likes, and Dislikes in episode metadata.".into()),
            default: true,
        },
        PreferenceDefinition::Info {
            title: "Cloudflare fallback".into(),
            summary: Some(
                "Direct API calls retry after a WebView bootstrap when the site serves a challenge page. Headless verification depends on host WebView support.".into(),
            ),
        },
    ]
}

fn handle_url(candidate: &str, base_url: &str) -> Result<Option<UrlResolveResult>> {
    let url = Url::parse(candidate).map_err(|error| Error::new(error.to_string()))?;
    let host = url
        .host_str()
        .ok_or_else(|| Error::new("URL host missing"))?;
    let allowed = DOMAIN_HOSTS
        .iter()
        .any(|value| host.eq_ignore_ascii_case(value));
    if !allowed {
        return Ok(None);
    }
    let path = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
    if path.len() == 2 && path[0] == "anime" {
        let id = path[1].to_string();
        return Ok(Some(UrlResolveResult {
            item: Some(CatalogItem {
                key: id.clone(),
                url: Some(format!("{base_url}/anime/{id}")),
                language: Some(LANG.to_string()),
                ..CatalogItem::default()
            }),
            ..UrlResolveResult::default()
        }));
    }
    if path.len() >= 2 && path[0] == "watch" {
        let id = path[1].to_string();
        let mut result = UrlResolveResult {
            item: Some(CatalogItem {
                key: id.clone(),
                url: Some(format!("{base_url}/anime/{id}")),
                language: Some(LANG.to_string()),
                ..CatalogItem::default()
            }),
            ..UrlResolveResult::default()
        };
        if let Some(number) = path.get(2).and_then(|value| value.parse::<f32>().ok()) {
            result.episode_key = Some(format!("{id}/{}", number_label(number)));
            result.video_episode = Some(VideoEpisode {
                key: format!("{id}/{}", number_label(number)),
                episode_number: Some(number),
                url: Some(candidate.to_string()),
                language: Some(LANG.to_string()),
                ..VideoEpisode::default()
            });
        }
        return Ok(Some(result));
    }
    Ok(None)
}

fn episode_anime_id(episode: &VideoEpisode) -> Result<&str> {
    episode
        .extra
        .get("animeId")
        .and_then(Value::as_str)
        .or_else(|| episode.key.split('/').next())
        .ok_or_else(|| Error::new("Animetsu episode missing anime id"))
}

fn is_challenge_response(status: u16, text: Option<&str>) -> bool {
    matches!(status, 403 | 429 | 503) || text.is_some_and(is_challenge_html)
}

fn is_challenge_html(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    [
        "just a moment",
        "cf-browser-verification",
        "cf-challenge",
        "cloudflare-static",
        "challenge-platform",
        "ddos-guard",
        "ddos guard",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn pref_bool(key: &str, default: bool) -> bool {
    context::preference::<bool>(key)
        .ok()
        .flatten()
        .unwrap_or(default)
}

fn parse_iso_datetime(value: Option<&str>) -> Option<i64> {
    let value = value?;
    DateTime::parse_from_rfc3339(value)
        .map(|datetime| datetime.timestamp_millis())
        .ok()
}

fn clean_html(source: Option<&str>) -> Option<String> {
    let source = source?;
    let line_breaks = Regex::new(r"(?i)<br\s*/?>").unwrap();
    let style_tags = Regex::new(r"(?i)</?(i|b|em)>").unwrap();
    Some(
        style_tags
            .replace_all(&line_breaks.replace_all(source, "\n"), "")
            .trim()
            .to_string(),
    )
    .filter(|value| !value.is_empty())
}

fn title_case(value: &str) -> String {
    value
        .split(' ')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str().to_lowercase()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn status_label(value: Option<&str>) -> &'static str {
    match value.unwrap_or_default() {
        "RELEASING" => "ongoing",
        "FINISHED" => "completed",
        "CANCELLED" => "cancelled",
        "NOT_YET_RELEASED" => "upcoming",
        _ => "unknown",
    }
}

fn status_display(value: &str) -> &'static str {
    match value {
        "RELEASING" => "Airing",
        "FINISHED" => "Finished",
        "NOT_YET_RELEASED" => "Upcoming",
        "CANCELLED" => "Cancelled",
        _ => "Unknown",
    }
}

fn score_summary(score: Option<i32>, rank: Option<i32>, trending: Option<i32>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(score) = score.filter(|score| *score > 0) {
        let stars = ((score as f32 / 20.0).round() as usize).clamp(1, 5);
        let fancy = format!("{}{} {}", "★".repeat(stars), "☆".repeat(5 - stars), score);
        parts.push(fancy);
    }
    if let Some(rank) = rank {
        parts.push(format!("#{rank}"));
    }
    if trending.unwrap_or_default() > 0 {
        parts.push("Trending".to_string());
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn format_next_airing(value: &AnimetsuNextAiringEpisodeDto) -> Option<String> {
    let episode = value.ep_num?;
    let label = value
        .time_left
        .map(format_time_left)
        .filter(|time| !time.is_empty())
        .map(|time| format!("Next Episode: Ep. {episode} in {time}"))
        .unwrap_or_else(|| format!("Next Episode: Ep. {episode}"));
    Some(label)
}

fn format_time_left(seconds: i64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 && days == 0 {
        parts.push(format!("{minutes}m"));
    }
    parts.join(" ")
}

fn format_number(number: i32) -> String {
    if number >= 1_000_000 {
        format!("{:.1}M", number as f64 / 1_000_000.0)
    } else if number >= 1_000 {
        format!("{:.1}k", number as f64 / 1_000.0)
    } else {
        number.to_string()
    }
}

fn format_episode_stats(views: Option<i32>, likes: Option<i32>, dislikes: Option<i32>) -> String {
    let mut parts = Vec::new();
    if let Some(views) = views {
        parts.push(format!("Views: {}", format_number(views)));
    }
    if let Some(likes) = likes {
        parts.push(format!("Likes: {}", format_number(likes)));
    }
    if let Some(dislikes) = dislikes {
        parts.push(format!("Dislikes: {}", format_number(dislikes)));
    }
    parts.join(" | ")
}

fn normalize_quality_label(value: &str) -> String {
    let upper = value.trim().to_uppercase();
    if upper == "MASTER" {
        "Auto".to_string()
    } else if value.trim().ends_with('p') || value.trim().ends_with('P') {
        value.trim().to_string().to_lowercase()
    } else if value.chars().all(|character| character.is_ascii_digit()) {
        format!("{}p", value.trim())
    } else {
        value.trim().to_string()
    }
}

fn stream_name(server: &str, audio: &str, quality: &str, softsub: bool) -> String {
    let suffix = if softsub {
        "[Soft Subs]"
    } else {
        "[Hard Subs]"
    };
    format!(
        "{}: {} ({}) {}",
        server.to_uppercase(),
        quality,
        audio_label(audio),
        suffix
    )
}

fn audio_label(audio: &str) -> &'static str {
    if audio.eq_ignore_ascii_case("dub") {
        "DUB"
    } else {
        "SUB"
    }
}

fn format_server_name(server: &str) -> String {
    match server {
        "kite" => "Kite".into(),
        "dio" => "Dio".into(),
        "sage" => "Sage".into(),
        "meg" => "Meg".into(),
        _ => title_case(server),
    }
}

fn is_softsub(payload: &AnimetsuVideoDto) -> bool {
    payload
        .server
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("kite"))
}

fn is_mp4_type(source: &AnimetsuSourceDto) -> bool {
    source
        .r#type
        .as_deref()
        .is_some_and(|kind| kind.to_ascii_lowercase().contains("mp4"))
}

fn is_hls_type(source: &AnimetsuSourceDto, full_url: &str) -> bool {
    source
        .r#type
        .as_deref()
        .is_some_and(|kind| kind.to_ascii_lowercase().contains("mpegurl"))
        || full_url.contains(".m3u8")
}

fn is_preferred_stream(
    server: &str,
    audio: &str,
    quality: &str,
    preferred_server: &str,
    preferred_audio: &str,
    preferred_quality: &str,
) -> bool {
    (preferred_server.eq_ignore_ascii_case("none") || preferred_server.eq_ignore_ascii_case(server))
        && (preferred_audio.eq_ignore_ascii_case("none")
            || preferred_audio.eq_ignore_ascii_case(audio))
        && quality
            .to_ascii_lowercase()
            .contains(&preferred_quality.to_ascii_lowercase())
}

fn sort_streams(
    mut streams: Vec<VideoStream>,
    preferred_quality: &str,
    preferred_server: &str,
    preferred_audio: &str,
) -> Vec<VideoStream> {
    let quality_rank = |quality: Option<&String>| {
        QUALITY_VALUES
            .iter()
            .position(|value| quality.is_some_and(|quality| quality.contains(value)))
            .unwrap_or(usize::MAX)
    };
    streams.sort_by_key(|stream| {
        let name = stream.name.clone().unwrap_or_default().to_ascii_lowercase();
        let quality = stream.quality.clone();
        (
            !quality
                .as_deref()
                .is_some_and(|value| value.contains(preferred_quality)),
            quality_rank(quality.as_ref()),
            !preferred_server.eq_ignore_ascii_case("none")
                && !name.contains(&preferred_server.to_ascii_lowercase()),
            !preferred_audio.eq_ignore_ascii_case("none")
                && !name.contains(&preferred_audio.to_ascii_lowercase()),
        )
    });
    streams
}

fn video_headers_for(base_url: &str, watch_referer: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Accept".into(), "*/*".into()),
        ("Accept-Language".into(), "en-US,en;q=0.9".into()),
        ("Referer".into(), watch_referer.to_string()),
        ("Origin".into(), base_url.to_string()),
        ("Sec-Fetch-Dest".into(), "empty".into()),
        ("Sec-Fetch-Mode".into(), "cors".into()),
        ("Sec-Fetch-Site".into(), "cross-site".into()),
    ])
}

fn absolute_url(base: &str, value: &str) -> Result<String> {
    if value.starts_with("http://") || value.starts_with("https://") {
        return Ok(value.to_string());
    }
    Url::parse(base)
        .and_then(|url| url.join(value))
        .map(|url| url.to_string())
        .map_err(|error| Error::new(error.to_string()))
}

fn to_segment(value: Option<&AnimetsuSkipTimeDto>) -> Option<MediaSegment> {
    let value = value?;
    (value.end > value.start).then(|| MediaSegment {
        start_seconds: value.start,
        end_seconds: value.end,
    })
}

fn number_label(number: f32) -> String {
    if (number.fract()).abs() < f32::EPSILON {
        format!("{}", number as i32)
    } else {
        let mut value = format!("{number}");
        while value.ends_with('0') {
            value.pop();
        }
        value.trim_end_matches('.').to_string()
    }
}

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters.get(key).and_then(|value| match value {
        Value::String(value) => (!value.is_empty() && value != "Any").then(|| value.clone()),
        Value::Object(values) => values
            .iter()
            .find_map(|(name, selected)| selected.as_bool().unwrap_or(false).then(|| name.clone()))
            .filter(|value| !value.is_empty()),
        _ => None,
    })
}

fn filter_list(filters: &Value, key: &str) -> Option<Vec<String>> {
    match filters.get(key) {
        Some(Value::Array(values)) => Some(
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.is_empty())
                .collect(),
        ),
        Some(Value::Object(values)) => Some(
            values
                .iter()
                .filter_map(|(name, selected)| {
                    selected.as_bool().unwrap_or(false).then(|| name.clone())
                })
                .collect(),
        ),
        _ => None,
    }
}

fn select_filter(id: &str, name: &str, options: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Select {
        id: id.to_string(),
        name: name.to_string(),
        options: options
            .iter()
            .map(|(label, value)| option(label, value))
            .collect(),
        default_index: 0,
    }
}

fn checkbox_group(id: &str, name: &str, options: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Group {
        id: id.to_string(),
        name: name.to_string(),
        filters: options
            .iter()
            .map(|(label, value)| FilterDefinition::CheckBox {
                id: (*value).to_string(),
                name: (*label).to_string(),
                default: false,
            })
            .collect(),
    }
}

fn option(label: &str, value: &str) -> OptionItem {
    OptionItem {
        label: label.to_string(),
        value: value.to_string(),
    }
}

const GENRES: &[(&str, &str)] = &[
    ("Action", "Action"),
    ("Adventure", "Adventure"),
    ("Comedy", "Comedy"),
    ("Drama", "Drama"),
    ("Ecchi", "Ecchi"),
    ("Fantasy", "Fantasy"),
    ("Horror", "Horror"),
    ("Mahou Shoujo", "Mahou Shoujo"),
    ("Mecha", "Mecha"),
    ("Music", "Music"),
    ("Mystery", "Mystery"),
    ("Psychological", "Psychological"),
    ("Romance", "Romance"),
    ("Sci-Fi", "Sci-Fi"),
    ("Slice of Life", "Slice of Life"),
    ("Sports", "Sports"),
    ("Supernatural", "Supernatural"),
    ("Thriller", "Thriller"),
];

const TAGS: &[(&str, &str)] = &[
    ("4-koma", "4-koma"),
    ("Achronological Order", "Achronological Order"),
    ("Afterlife", "Afterlife"),
    ("Age Gap", "Age Gap"),
    ("Airsoft", "Airsoft"),
    ("Aliens", "Aliens"),
    ("Alternate Universe", "Alternate Universe"),
    ("American Football", "American Football"),
    ("Amnesia", "Amnesia"),
    ("Anti-Hero", "Anti-Hero"),
    ("Archery", "Archery"),
    ("Assassins", "Assassins"),
    ("Athletics", "Athletics"),
    ("Augmented Reality", "Augmented Reality"),
    ("Aviation", "Aviation"),
    ("Badminton", "Badminton"),
    ("Band", "Band"),
    ("Bar", "Bar"),
    ("Baseball", "Baseball"),
    ("Basketball", "Basketball"),
    ("Battle Royale", "Battle Royale"),
    ("Biographical", "Biographical"),
    ("Bisexual", "Bisexual"),
    ("Body Swapping", "Body Swapping"),
    ("Boxing", "Boxing"),
    ("Bullying", "Bullying"),
    ("Calligraphy", "Calligraphy"),
    ("Card Battle", "Card Battle"),
    ("Cars", "Cars"),
    ("CGI", "CGI"),
    ("Chibi", "Chibi"),
    ("Chuunibyou", "Chuunibyou"),
    ("Classic Literature", "Classic Literature"),
    ("College", "College"),
    ("Coming of Age", "Coming of Age"),
    ("Cosplay", "Cosplay"),
    ("Crossdressing", "Crossdressing"),
    ("Crossover", "Crossover"),
    ("Cultivation", "Cultivation"),
    (
        "Cute Girls Doing Cute Things",
        "Cute Girls Doing Cute Things",
    ),
    ("Cyberpunk", "Cyberpunk"),
    ("Cycling", "Cycling"),
    ("Dancing", "Dancing"),
    ("Delinquents", "Delinquents"),
    ("Demons", "Demons"),
    ("Development", "Development"),
    ("Dragons", "Dragons"),
    ("Drawing", "Drawing"),
    ("Dystopian", "Dystopian"),
    ("Economics", "Economics"),
    ("Educational", "Educational"),
    ("Ensemble Cast", "Ensemble Cast"),
    ("Environmental", "Environmental"),
    ("Episodic", "Episodic"),
    ("Espionage", "Espionage"),
    ("Fairy Tale", "Fairy Tale"),
    ("Family Life", "Family Life"),
    ("Fashion", "Fashion"),
    ("Female Protagonist", "Female Protagonist"),
    ("Fishing", "Fishing"),
    ("Fitness", "Fitness"),
    ("Flash", "Flash"),
    ("Food", "Food"),
    ("Football", "Football"),
    ("Foreign", "Foreign"),
    ("Fugitive", "Fugitive"),
    ("Full CGI", "Full CGI"),
    ("Full Colour", "Full Colour"),
    ("Gambling", "Gambling"),
    ("Gangs", "Gangs"),
    ("Gender Bending", "Gender Bending"),
    ("Gender Neutral", "Gender Neutral"),
    ("Ghost", "Ghost"),
    ("Gods", "Gods"),
    ("Gore", "Gore"),
    ("Guns", "Guns"),
    ("Gyaru", "Gyaru"),
    ("Harem", "Harem"),
    ("Henshin", "Henshin"),
    ("Hikikomori", "Hikikomori"),
    ("Historical", "Historical"),
    ("Ice Skating", "Ice Skating"),
    ("Idol", "Idol"),
    ("Isekai", "Isekai"),
    ("Iyashikei", "Iyashikei"),
    ("Josei", "Josei"),
    ("Kaiju", "Kaiju"),
    ("Karuta", "Karuta"),
    ("Kemonomimi", "Kemonomimi"),
    ("Kids", "Kids"),
    ("Love Triangle", "Love Triangle"),
    ("Magic", "Magic"),
    ("Mahjong", "Mahjong"),
    ("Male Protagonist", "Male Protagonist"),
    ("Mafia", "Mafia"),
    ("Maids", "Maids"),
    ("Martial Arts", "Martial Arts"),
    ("Memory Manipulation", "Memory Manipulation"),
    ("Meta", "Meta"),
    ("Military", "Military"),
    ("Monster Girl", "Monster Girl"),
    ("Motorcycles", "Motorcycles"),
    ("Musical", "Musical"),
    ("Mythology", "Mythology"),
    ("Nekomimi", "Nekomimi"),
    ("Ninja", "Ninja"),
    ("No Dialogue", "No Dialogue"),
    ("Noir", "Noir"),
    ("Nudity", "Nudity"),
    ("Otaku Culture", "Otaku Culture"),
    ("Outdoor", "Outdoor"),
    ("Parody", "Parody"),
    ("Philosophy", "Philosophy"),
    ("Pirates", "Pirates"),
    ("Photography", "Photography"),
    ("Poker", "Poker"),
    ("Police", "Police"),
    ("Politics", "Politics"),
    ("Post-Apocalyptic", "Post-Apocalyptic"),
    ("Primarily Adult Cast", "Primarily Adult Cast"),
    ("Primarily Female Cast", "Primarily Female Cast"),
    ("Primarily Male Cast", "Primarily Male Cast"),
    ("Puppetry", "Puppetry"),
    ("Real Robot", "Real Robot"),
    ("Rehabilitation", "Rehabilitation"),
    ("Reincarnation", "Reincarnation"),
    ("Revenge", "Revenge"),
    ("Reverse Harem", "Reverse Harem"),
    ("Robots", "Robots"),
    ("Rugby", "Rugby"),
    ("Rural", "Rural"),
    ("Samurai", "Samurai"),
    ("Satire", "Satire"),
    ("School", "School"),
    ("School Club", "School Club"),
    ("Seinen", "Seinen"),
    ("Ships", "Ships"),
    ("Shogi", "Shogi"),
    ("Shoujo", "Shoujo"),
    ("Shoujo Ai", "Shoujo Ai"),
    ("Shounen", "Shounen"),
    ("Shounen Ai", "Shounen Ai"),
    ("Slapstick", "Slapstick"),
    ("Slavery", "Slavery"),
    ("Space", "Space"),
    ("Space Opera", "Space Opera"),
    ("Steampunk", "Steampunk"),
    ("Stop Motion", "Stop Motion"),
    ("Super Power", "Super Power"),
    ("Super Robot", "Super Robot"),
    ("Superhero", "Superhero"),
    ("Surreal Comedy", "Surreal Comedy"),
];

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().video(SOURCE_ID, AnimetsuSource::default())
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_search_url_with_filters() {
        let url = search_url(
            "https://animetsu.net/v2/api",
            "one piece",
            2,
            &json!({
                "sort": "average_score",
                "format": "TV",
                "status": "RELEASING",
                "season": "SUMMER",
                "year": {"2026": true},
                "country": "JP",
                "source": "MANGA",
                "genres": {"Action": true, "Adventure": true},
                "tags": {"Pirates": true}
            }),
        )
        .unwrap();
        assert!(url.contains("query=one+piece"));
        assert!(url.contains("genres=Action%2CAdventure"));
        assert!(url.contains("tags=Pirates"));
        assert!(url.contains("format=TV"));
    }

    #[test]
    fn maps_search_and_recent_payloads() {
        let search: AnimetsuSearchResponse =
            serde_json::from_str(include_str!("../tests/fixtures/search.json")).unwrap();
        let search_page = to_search_page(
            search.results,
            search.page,
            search.last_page,
            "english",
            true,
            true,
            "https://animetsu.net",
        );
        assert_eq!(
            search_page.entries[0].title,
            "That Time I Got Reincarnated as a Slime Season 4"
        );
        assert!(search_page.has_next_page);

        let recent: AnimetsuRecentResponse =
            serde_json::from_str(include_str!("../tests/fixtures/recent.json")).unwrap();
        let recent_page = to_recent_page(
            recent.results,
            recent.current_page,
            recent.last_page,
            "romaji",
            true,
            false,
            "https://animetsu.net",
        );
        assert_eq!(recent_page.entries.len(), 2);
        assert!(recent_page.entries[1].banner.is_none());
    }

    #[test]
    fn maps_details_with_metadata_preferences() {
        let dto: AnimetsuAnimeDto =
            serde_json::from_str(include_str!("../tests/fixtures/details.json")).unwrap();
        let item = dto
            .to_catalog_item(
                "english",
                true,
                &MetadataPreferences {
                    show_extra_info: true,
                    show_relations: true,
                    show_characters: true,
                    show_staff: true,
                    show_trackers: true,
                    show_trailer: true,
                    show_banner: true,
                },
                "https://animetsu.net",
            )
            .unwrap();
        assert_eq!(item.title, "Snack Hazama");
        assert!(item
            .description
            .as_deref()
            .unwrap()
            .contains("Main Characters"));
        assert!(item.tags.iter().any(|tag| tag == "Bar"));
    }

    #[test]
    fn maps_episodes_and_stats() {
        let episodes: Vec<AnimetsuEpisodeDto> =
            serde_json::from_str(include_str!("../tests/fixtures/episodes.json")).unwrap();
        let mapped = episodes
            .into_iter()
            .filter_map(|episode| {
                episode.to_video_episode(
                    "6a3ee67c945a0b6281a283be",
                    true,
                    "https://animetsu.net",
                    "all",
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(mapped[0].key, "6a3ee67c945a0b6281a283be/1");
        assert!(mapped[0]
            .release_group
            .as_deref()
            .unwrap()
            .contains("Views: 5.6k"));
    }

    #[test]
    fn builds_hosters_from_servers_and_audio_types() {
        let servers: Vec<AnimetsuServerDto> =
            serde_json::from_str(include_str!("../tests/fixtures/servers.json")).unwrap();
        let hosters = servers
            .into_iter()
            .flat_map(|server| {
                AUDIO_VALUES.iter().map(move |audio| VideoHoster {
                    key: format!("{}:{audio}", server.id),
                    name: format!(
                        "{} - {}",
                        format_server_name(&server.id),
                        audio_label(audio)
                    ),
                    ..VideoHoster::default()
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(hosters.len(), 8);
        assert_eq!(hosters[0].name, "Kite - SUB");
    }

    #[test]
    fn converts_mp4_hls_and_subtitle_payloads() {
        let meg: AnimetsuVideoDto =
            serde_json::from_str(include_str!("../tests/fixtures/oppai-meg-sub.json")).unwrap();
        let dio: AnimetsuVideoDto =
            serde_json::from_str(include_str!("../tests/fixtures/oppai-dio-sub.json")).unwrap();
        let kite: AnimetsuVideoDto =
            serde_json::from_str(include_str!("../tests/fixtures/oppai-kite-sub.json")).unwrap();
        let data = HosterData {
            anime_id: "one-piece".into(),
            episode_number: "1168".into(),
            server_id: "meg".into(),
            audio_type: "sub".into(),
            watch_referer: "https://animetsu.net/watch/one-piece".into(),
        };
        let mp4_streams = streams_from_payload(
            &meg,
            &data,
            "https://animetsu.net",
            "meg",
            "sub",
            "1080",
            |_| Ok(Vec::new()),
        )
        .unwrap();
        assert_eq!(mp4_streams.len(), 3);
        assert_eq!(mp4_streams[0].format.as_deref(), Some("mp4"));
        assert!(mp4_streams[0].preferred);

        let hls_streams = streams_from_payload(
            &dio,
            &HosterData {
                server_id: "dio".into(),
                ..data.clone()
            },
            "https://animetsu.net",
            "meg",
            "sub",
            "1080",
            |_| {
                parse_hls_variants(
                    include_str!("../tests/fixtures/master-playlist.m3u8"),
                    "https://swiftstream.top/master.m3u8",
                )
            },
        )
        .unwrap();
        assert_eq!(hls_streams.len(), 3);
        assert!(hls_streams[0].is_hls);
        assert!(hls_streams[0].segment_processing.is_some());

        let subtitle_streams = streams_from_payload(
            &kite,
            &HosterData {
                server_id: "kite".into(),
                ..data
            },
            "https://animetsu.net",
            "kite",
            "sub",
            "1080",
            |_| {
                Ok(vec![HlsVariant {
                    url: "https://swiftstream.top/720/index.m3u8".into(),
                    quality: "720p".into(),
                }])
            },
        )
        .unwrap();
        assert_eq!(subtitle_streams[0].subtitles.len(), 1);
        assert_eq!(
            subtitle_streams[0].subtitles[0].label.as_deref(),
            Some("English")
        );
    }

    #[test]
    fn parses_hls_variants_with_garbage_prefix() {
        let variants = parse_hls_variants(
            include_str!("../tests/fixtures/master-playlist.m3u8"),
            "https://swiftstream.top/master.m3u8",
        )
        .unwrap();
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[0].quality, "1080p");
        assert_eq!(variants[1].url, "https://swiftstream.top/720/index.m3u8");
    }

    #[test]
    fn expresses_segment_processing_rules() {
        let processing = animetsu_segment_processing();
        assert!(processing.rewrite_playlist);
        assert!(processing.rules[0].auto_detect_media_offset);
        assert!(processing.rules[0]
            .resource_types
            .contains(&MediaResourceKind::Playlist));
    }

    #[test]
    fn detects_challenge_responses_and_builds_browser_request() {
        assert!(is_challenge_response(
            403,
            Some("<title>Just a moment...</title>")
        ));
        assert!(is_challenge_html(
            "<script src=\"/cdn-cgi/challenge-platform/x\"></script>"
        ));
        assert!(!is_challenge_response(200, Some("{\"ok\":true}")));

        let request = build_browser_bootstrap_request(
            "https://animetsu.net/v2/api/anime/search/?page=1",
            "https://animetsu.net",
            &[("Referer".into(), "https://animetsu.net/browse".into())],
        );
        assert_eq!(request.cookie_url.as_deref(), Some("https://animetsu.net"));
        assert_eq!(
            request.wait_until,
            Some(browser::WebViewWaitUntil::NetworkIdle)
        );
        assert!(request.return_html);
    }

    #[test]
    fn resolves_item_and_episode_urls() {
        let item = handle_url("https://animetsu.net/anime/abc123", "https://animetsu.net")
            .unwrap()
            .unwrap();
        assert_eq!(item.item.unwrap().key, "abc123");

        let episode = handle_url(
            "https://animetsu.net/watch/abc123/12",
            "https://animetsu.net",
        )
        .unwrap()
        .unwrap();
        assert_eq!(episode.episode_key.as_deref(), Some("abc123/12"));
        assert_eq!(episode.video_episode.unwrap().episode_number, Some(12.0));
    }

    #[test]
    fn rejects_malformed_payloads() {
        assert!(serde_json::from_str::<AnimetsuSearchResponse>("{}").is_err());
        assert!(serde_json::from_str::<AnimetsuVideoDto>("{\"sources\":null}").is_err());
        assert!(
            parse_hls_variants("not a playlist", "https://swiftstream.top/master.m3u8")
                .unwrap()
                .is_empty()
        );
    }
}
