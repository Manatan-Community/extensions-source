use std::collections::{BTreeMap, BTreeSet};

use manatan_sdk::{
    browser::{
        self, WebViewCapturedRequest, WebViewExtractRequest, WebViewExtractResponse,
        WebViewRequestCapture, WebViewSession, WebViewSessionPersistence, WebViewWaitUntil,
    },
    client::Client,
    model::{
        CatalogItem, FilterDefinition, ImageRequest, OptionItem, Paged, UrlResolveResult,
        VideoEpisode, VideoStream,
    },
    runtime, Error, Result, VideoSource,
};
use serde_json::{json, Value};
use url::Url;

const BASE_URL: &str = "https://silip.pages.dev";
const TMDB_URL: &str = "https://api.themoviedb.org/3";
const IMAGE_URL: &str = "https://image.tmdb.org/t/p";
const VIDCORE_URL: &str = "https://vidcore.org";
// This is the browser key published by Silip's current client bundle.
const TMDB_KEY: &str = "1ffd85b911c0187b3bbf2cb9a20c1e3e";
// VidCore loads its player in a cross-origin child frame. The host captures
// requests from every frame, but page JavaScript cannot inspect that frame's
// performance entries, so keep a bounded capture window instead of polling an
// inaccessible resource list.
const WAIT_FOR_MEDIA_SCRIPT: &str = "(() => { const now = Date.now(); const started = window.__manatanSilipCaptureStarted ||= now; return now - started >= 15000; })()";

#[derive(Default)]
pub struct Silip;

impl Silip {
    fn tmdb(&self, route: &str, parameters: &[(&str, String)]) -> Result<Value> {
        let mut url = Url::parse(&format!("{TMDB_URL}{route}")).map_err(url_error)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("api_key", TMDB_KEY);
            query.append_pair("language", "en-US");
            query.append_pair("include_adult", "false");
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

    fn discover(
        &self,
        kind: &str,
        page: u32,
        sort: &str,
        genre: Option<&str>,
        adult: bool,
    ) -> Result<Value> {
        let date_sort = if kind == "tv" {
            "first_air_date.desc"
        } else {
            "primary_release_date.desc"
        };
        let sort = match sort {
            "rating" => "vote_average.desc",
            "newest" => date_sort,
            _ => "popularity.desc",
        };
        let mut common = vec![
            ("with_origin_country", "PH".to_string()),
            ("with_original_language", "tl|fil".to_string()),
            ("sort_by", sort.to_string()),
            ("page", page.max(1).to_string()),
        ];
        if let Some(genre) = genre.filter(|value| !value.is_empty()) {
            common.push(("with_genres", genre.to_string()));
        }
        if !adult {
            common.push((
                if kind == "tv" {
                    "without_networks"
                } else {
                    "without_companies"
                },
                if kind == "tv" { "4569" } else { "149142" }.to_string(),
            ));
            common.push(("certification_country", "PH".to_string()));
            common.push(("certification.lte", "R-16".to_string()));
            return self.tmdb(&format!("/discover/{kind}"), &common);
        }

        let mut provider = common.clone();
        provider.push((
            if kind == "tv" {
                "with_networks"
            } else {
                "with_companies"
            },
            if kind == "tv" { "4569" } else { "149142" }.to_string(),
        ));
        let mut certified = common;
        certified.push(("certification_country", "PH".to_string()));
        certified.push(("certification", "R-18".to_string()));
        let provider = self.tmdb(&format!("/discover/{kind}"), &provider)?;
        let certified = self.tmdb(&format!("/discover/{kind}"), &certified)?;
        Ok(merge_tmdb_pages(&provider, &certified, sort))
    }

    fn browse(&self, page: u32, sort: &str, filters: &Value) -> Result<Paged<CatalogItem>> {
        let kind = filter(filters, "type");
        let genre = filter(filters, "genre");
        let adult = filter(filters, "catalog") == Some("r18");
        match kind {
            Some("movie" | "tv") => parse_tmdb_page(
                &self.discover(kind.unwrap_or("movie"), page, sort, genre, adult)?,
                Some(kind.unwrap_or("movie")),
                adult,
            ),
            _ => {
                let movies = self.discover("movie", page, sort, genre, adult)?;
                let television = self.discover("tv", page, sort, genre, adult)?;
                let merged = merge_kinds(&movies, &television, sort);
                parse_tmdb_page(&merged, None, adult)
            }
        }
    }

    fn capture_stream(&self, embed_url: &str) -> Result<Vec<VideoStream>> {
        validate_vidcore_url(embed_url)?;
        let navigation_url = cache_busted_url(embed_url, runtime::now_millis())?;
        let response: WebViewExtractResponse = browser::extract(&WebViewExtractRequest {
            url: navigation_url,
            method: Default::default(),
            body: None,
            cookie_url: None,
            session: Some(WebViewSession {
                id: "silip-vidcore-player".to_string(),
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
            wait_for_script: Some(WAIT_FOR_MEDIA_SCRIPT.to_string()),
            script: "(() => { const video = document.querySelector('video'); const resources = performance.getEntriesByType('resource').map(entry => entry.name); return { url: video?.currentSrc || video?.src || '', urls: Array.from(new Set(resources.filter(url => typeof url === 'string' && (url.includes('.m3u8') || url.includes('.mpd') || url.includes('/stream/mpd'))))) }; })()".to_string(),
            timeout_ms: Some(120_000),
            capture_requests: vec![capture(".m3u8"), capture(".mpd"), capture("/stream/")],
            capture_events: Vec::new(),
            cookies: false,
            headless: Some(true),
            preload_scripts: Vec::new(),
        })?;
        let streams = streams_from_capture(&response, embed_url)?;
        let requests = streams
            .iter()
            .map(|stream| {
                let request = Client::browser()
                    .get(&stream.url)
                    .cookies_for(&stream.url)
                    .timeout_ms(20_000)
                    .max_body_bytes(1024 * 1024);
                stream
                    .headers
                    .iter()
                    .fold(request, |request, (name, value)| {
                        request.header(name, value)
                    })
            })
            .collect();
        let candidates = streams
            .into_iter()
            .zip(Client::send_many(requests, 3))
            .filter_map(|(stream, response)| {
                let response = response.ok()?.error_for_status().ok()?;
                let body = response.text().ok()?.to_string();
                let valid = if stream.is_hls {
                    body.trim_start().starts_with("#EXTM3U")
                } else {
                    body.contains("<MPD")
                };
                valid.then_some((stream, body))
            })
            .collect();
        let direct_url = response
            .value
            .as_ref()
            .and_then(|value| value.get("url"))
            .and_then(Value::as_str);
        let streams = root_manifest_streams(candidates, direct_url);
        if streams.is_empty() {
            return Err(Error::new("Silip player manifests are unavailable"));
        }
        Ok(streams)
    }
}

impl VideoSource for Silip {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse(page, "popular", &json!({}))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse(page, "newest", &json!({}))
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        if query.trim().is_empty() {
            return self.browse(page, filter(filters, "sort").unwrap_or("popular"), filters);
        }
        let value = self.tmdb(
            "/search/multi",
            &[
                ("query", query.trim().to_string()),
                ("page", page.max(1).to_string()),
            ],
        )?;
        let filtered = filter_search_results(&value);
        parse_tmdb_page(&filtered, None, false)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let (kind, id) = parse_key(&item.key)?;
        let value = self.tmdb(
            &format!("/{kind}/{id}"),
            &[(
                "append_to_response",
                if kind == "movie" {
                    "credits,videos,similar,release_dates"
                } else {
                    "credits,videos,similar,content_ratings"
                }
                .to_string(),
            )],
        )?;
        let adult = item.content_rating.as_deref() == Some("adult") || is_r18(&value, kind);
        parse_tmdb_item(&value, kind, adult)
    }

    fn episodes(&mut self, item: CatalogItem) -> Result<Vec<VideoEpisode>> {
        let (kind, id) = parse_key(&item.key)?;
        if kind == "movie" {
            return Ok(vec![VideoEpisode {
                key: item.key,
                title: Some("Movie".to_string()),
                episode_number: Some(1.0),
                url: Some(format!("{BASE_URL}/watch/{id}?type=movie")),
                ..VideoEpisode::default()
            }]);
        }
        let details = self.tmdb(&format!("/tv/{id}"), &[])?;
        let seasons = details
            .get("seasons")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::new("Silip returned no seasons"))?;
        let mut episodes = Vec::new();
        for season in seasons {
            let season_number = season
                .get("season_number")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if season_number == 0 {
                continue;
            }
            let value = self.tmdb(&format!("/tv/{id}/season/{season_number}"), &[])?;
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
                    key: format!("tv:{id}:{season_number}:{episode_number}"),
                    title: string(episode, "name"),
                    description: string(episode, "overview"),
                    episode_number: Some(episode_number as f32),
                    season_number: Some(season_number as f32),
                    thumbnail: image(episode, "still_path", "w500"),
                    url: Some(format!(
                        "{BASE_URL}/watch/{id}?type=tv&season={season_number}&episode={episode_number}"
                    )),
                    ..VideoEpisode::default()
                });
            }
        }
        episodes.sort_by(|left, right| {
            right
                .season_number
                .partial_cmp(&left.season_number)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    right
                        .episode_number
                        .partial_cmp(&left.episode_number)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        Ok(episodes)
    }

    fn streams(&mut self, item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoStream>> {
        let (kind, id) = parse_key(&item.key)?;
        let embed = if kind == "movie" {
            format!("{VIDCORE_URL}/embed/movie/{id}")
        } else {
            let (episode_id, season, number) = parse_episode_key(&episode.key)?;
            if episode_id != id {
                return Err(Error::new("episode does not belong to this Silip item"));
            }
            format!("{VIDCORE_URL}/embed/tv/{id}/{season}/{number}")
        };
        self.capture_stream(&embed)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![
            select_filter(
                "type",
                "Type",
                &[("All", ""), ("Movies", "movie"), ("Series", "tv")],
            ),
            select_filter(
                "catalog",
                "Catalog",
                &[("General", "general"), ("R-18", "r18")],
            ),
            select_filter(
                "sort",
                "Sort",
                &[
                    ("Popular", "popular"),
                    ("Highest Rated", "rating"),
                    ("Newest", "newest"),
                ],
            ),
            select_filter(
                "genre",
                "Genre",
                &[
                    ("All", ""),
                    ("Action", "28"),
                    ("Comedy", "35"),
                    ("Drama", "18"),
                    ("Horror", "27"),
                    ("Romance", "10749"),
                    ("Thriller", "53"),
                ],
            ),
        ])
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        let (kind, id) = parse_key(&item.key)?;
        Ok(Some(format!("{BASE_URL}/{kind}/{id}")))
    }

    fn episode_url(
        &mut self,
        _item: &CatalogItem,
        episode: &VideoEpisode,
    ) -> Result<Option<String>> {
        Ok(episode.url.clone())
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let Some((kind, id, season, episode)) = parse_silip_url(candidate)? else {
            return Ok(None);
        };
        let mut item = CatalogItem::new(format!("{kind}:{id}"), "");
        item.url = Some(format!("{BASE_URL}/{kind}/{id}"));
        item.language = Some("tl".to_string());
        let video_episode = if kind == "movie" {
            episode.map(|_| VideoEpisode {
                key: format!("movie:{id}"),
                title: Some("Movie".to_string()),
                episode_number: Some(1.0),
                url: Some(candidate.to_string()),
                ..VideoEpisode::default()
            })
        } else {
            match (season, episode) {
                (Some(season), Some(episode)) => Some(VideoEpisode {
                    key: format!("tv:{id}:{season}:{episode}"),
                    episode_number: Some(episode as f32),
                    season_number: Some(season as f32),
                    url: Some(candidate.to_string()),
                    ..VideoEpisode::default()
                }),
                _ => None,
            }
        };
        Ok(Some(UrlResolveResult {
            item: Some(item),
            video_episode,
            ..UrlResolveResult::default()
        }))
    }
}

fn parse_tmdb_page(
    value: &Value,
    forced_kind: Option<&str>,
    adult: bool,
) -> Result<Paged<CatalogItem>> {
    let mut entries = Vec::new();
    for raw in value
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::new("TMDB response did not contain results"))?
    {
        let kind = forced_kind
            .map(str::to_string)
            .or_else(|| string(raw, "media_type"))
            .or_else(|| {
                raw.get("first_air_date")
                    .is_some()
                    .then(|| "tv".to_string())
            })
            .unwrap_or_else(|| "movie".to_string());
        if !matches!(kind.as_str(), "movie" | "tv") {
            continue;
        }
        entries.push(parse_tmdb_item(raw, &kind, adult)?);
    }
    let page = value.get("page").and_then(Value::as_u64).unwrap_or(1);
    let total = value
        .get("total_pages")
        .and_then(Value::as_u64)
        .unwrap_or(page);
    Ok(Paged::new(entries, page < total))
}

fn parse_tmdb_item(raw: &Value, kind: &str, adult: bool) -> Result<CatalogItem> {
    let id = raw
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::new("TMDB item is missing id"))?;
    let title = string(raw, "title")
        .or_else(|| string(raw, "name"))
        .ok_or_else(|| Error::new("TMDB item is missing title"))?;
    let mut authors = Vec::new();
    if kind == "tv" {
        authors.extend(
            raw.get("created_by")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|person| string(person, "name")),
        );
    } else {
        authors.extend(
            raw.pointer("/credits/crew")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|person| person.get("job").and_then(Value::as_str) == Some("Director"))
                .filter_map(|person| string(person, "name")),
        );
    }
    let status = string(raw, "status").map(|status| {
        json!(match status.as_str() {
            "Released" | "Ended" | "Canceled" => "completed",
            "Returning Series" | "In Production" | "Post Production" => "ongoing",
            _ => "unknown",
        })
    });
    Ok(CatalogItem {
        key: format!("{kind}:{id}"),
        title,
        url: Some(format!("{BASE_URL}/{kind}/{id}")),
        cover: image(raw, "poster_path", "w500"),
        banner: image(raw, "backdrop_path", "w1280"),
        description: string(raw, "overview"),
        authors,
        tags: raw
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| string(genre, "name"))
            .collect(),
        initialized: raw.get("genres").is_some(),
        language: string(raw, "original_language").or_else(|| Some("tl".to_string())),
        rating: raw
            .get("vote_average")
            .and_then(Value::as_f64)
            .map(|rating| rating as f32),
        status,
        content_rating: Some(if adult { "adult" } else { "suggestive" }.to_string()),
        ..CatalogItem::default()
    })
}

fn filter_search_results(value: &Value) -> Value {
    let mut value = value.clone();
    if let Some(results) = value.get_mut("results").and_then(Value::as_array_mut) {
        results.retain(|entry| {
            matches!(
                entry.get("media_type").and_then(Value::as_str),
                Some("movie" | "tv")
            ) && matches!(
                entry.get("original_language").and_then(Value::as_str),
                Some("tl" | "fil")
            )
        });
    }
    value
}

fn merge_kinds(movies: &Value, television: &Value, sort: &str) -> Value {
    let mut results = Vec::new();
    for (page, kind) in [(movies, "movie"), (television, "tv")] {
        for entry in page
            .get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let mut entry = entry.clone();
            entry["media_type"] = Value::String(kind.to_string());
            results.push(entry);
        }
    }
    sort_results(&mut results, sort);
    json!({
        "page": movies.get("page").and_then(Value::as_u64).unwrap_or(1),
        "total_pages": movies.get("total_pages").and_then(Value::as_u64).unwrap_or(1)
            .max(television.get("total_pages").and_then(Value::as_u64).unwrap_or(1)),
        "results": results,
    })
}

fn merge_tmdb_pages(left: &Value, right: &Value, sort: &str) -> Value {
    let mut seen = BTreeSet::new();
    let mut results = Vec::new();
    for entry in [left, right].into_iter().flat_map(|page| {
        page.get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
    }) {
        let Some(id) = entry.get("id").and_then(Value::as_u64) else {
            continue;
        };
        if seen.insert(id) {
            results.push(entry.clone());
        }
    }
    sort_results(&mut results, sort);
    json!({
        "page": left.get("page").and_then(Value::as_u64).unwrap_or(1),
        "total_pages": left.get("total_pages").and_then(Value::as_u64).unwrap_or(1)
            .max(right.get("total_pages").and_then(Value::as_u64).unwrap_or(1)),
        "results": results,
    })
}

fn sort_results(results: &mut [Value], sort: &str) {
    results.sort_by(|left, right| match sort {
        "newest" => date(right).cmp(date(left)),
        "rating" => number(right, "vote_average").total_cmp(&number(left, "vote_average")),
        _ => number(right, "popularity").total_cmp(&number(left, "popularity")),
    });
}

fn date(value: &Value) -> &str {
    value
        .get("release_date")
        .or_else(|| value.get("first_air_date"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or_default()
}

fn is_r18(value: &Value, kind: &str) -> bool {
    if value.get("adult").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    let path = if kind == "movie" {
        "/release_dates/results"
    } else {
        "/content_ratings/results"
    };
    value
        .pointer(path)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| entry.get("iso_3166_1").and_then(Value::as_str) == Some("PH"))
        .any(|entry| {
            entry.get("rating").and_then(Value::as_str) == Some("R-18")
                || entry
                    .get("release_dates")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|release| {
                        release.get("certification").and_then(Value::as_str) == Some("R-18")
                    })
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

fn filter<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).and_then(Value::as_str)
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
    let mut candidates: Vec<(String, Option<&WebViewCapturedRequest>)> = response
        .captured_requests
        .iter()
        .filter(|request| media_url(&request.url))
        .map(|request| (request.url.clone(), Some(request)))
        .collect();
    if let Some(url) = response
        .value
        .as_ref()
        .and_then(|value| value.get("url"))
        .and_then(Value::as_str)
        .filter(|url| media_url(url))
    {
        candidates.push((url.to_string(), None));
    }
    candidates.extend(
        response
            .value
            .as_ref()
            .and_then(|value| value.get("urls"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|url| media_url(url))
            .map(|url| (url.to_string(), None)),
    );
    let mut seen = BTreeSet::new();
    candidates.retain(|(url, _)| seen.insert(url.clone()));
    if candidates.is_empty() {
        return Err(Error::new("Silip player did not expose a playable stream"));
    }
    candidates
        .into_iter()
        .map(|(url, request)| {
            validate_vidcore_media_url(&url)?;
            let lower = url.to_ascii_lowercase();
            let is_hls = lower.contains(".m3u8");
            let is_dash = lower.contains(".mpd") || lower.contains("/stream/mpd");
            let mut headers: BTreeMap<String, String> = request
                .map(|request| {
                    request
                        .headers
                        .iter()
                        .filter(|(name, _)| {
                            matches!(
                                name.to_ascii_lowercase().as_str(),
                                "referer" | "origin" | "user-agent"
                            )
                        })
                        .map(|(name, value)| {
                            let name = match name.to_ascii_lowercase().as_str() {
                                "referer" => "Referer",
                                "origin" => "Origin",
                                "user-agent" => "User-Agent",
                                _ => name,
                            };
                            (name.to_string(), value.clone())
                        })
                        .collect()
                })
                .unwrap_or_default();
            headers
                .entry("Referer".to_string())
                .or_insert_with(|| referer.to_string());
            Ok(VideoStream {
                url,
                name: Some("Silip — Denise".to_string()),
                format: Some(if is_dash { "dash" } else { "hls" }.to_string()),
                is_hls,
                is_dash,
                requires_proxy: true,
                initialized: true,
                headers,
                ..VideoStream::default()
            })
        })
        .collect()
}

fn media_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains(".m3u8") || lower.contains(".mpd") || lower.contains("/stream/mpd")
}

fn root_manifest_streams(
    candidates: Vec<(VideoStream, String)>,
    direct_url: Option<&str>,
) -> Vec<VideoStream> {
    let mut children = BTreeSet::new();
    for (stream, body) in &candidates {
        if !stream.is_hls || !body.contains("#EXT-X-STREAM-INF:") {
            continue;
        }
        let Ok(base) = Url::parse(&stream.url) else {
            continue;
        };
        for line in body.lines().map(str::trim) {
            if !line.is_empty() && !line.starts_with('#') {
                if let Ok(url) = base.join(line) {
                    children.insert(url.to_string());
                }
            } else {
                let mut rest = line;
                while let Some((_, tail)) = rest.split_once("URI=\"") {
                    let Some((uri, tail)) = tail.split_once('"') else {
                        break;
                    };
                    if let Ok(url) = base.join(uri) {
                        children.insert(url.to_string());
                    }
                    rest = tail;
                }
            }
        }
    }
    let mut streams: Vec<_> = candidates
        .into_iter()
        .filter(|(stream, _)| !children.contains(&stream.url))
        .filter(|(stream, body)| {
            stream.is_dash
                || body.contains("#EXT-X-STREAM-INF:")
                || direct_url == Some(stream.url.as_str())
        })
        .map(|(mut stream, body)| {
            stream.preferred = stream.is_dash || body.contains("#EXT-X-STREAM-INF:");
            stream
        })
        .collect();
    streams.sort_by_key(|stream| !stream.preferred);
    streams
}

fn parse_key(key: &str) -> Result<(&str, u64)> {
    let (kind, id) = key
        .split_once(':')
        .ok_or_else(|| Error::new("invalid Silip key"))?;
    if !matches!(kind, "movie" | "tv") {
        return Err(Error::new("invalid Silip media type"));
    }
    let id = id
        .parse()
        .map_err(|_| Error::new("invalid Silip TMDB id"))?;
    Ok((kind, id))
}

fn parse_episode_key(key: &str) -> Result<(u64, u64, u64)> {
    let mut parts = key.split(':');
    if parts.next() != Some("tv") {
        return Err(Error::new("invalid Silip episode key"));
    }
    let id = parts.next().and_then(|value| value.parse().ok());
    let season = parts.next().and_then(|value| value.parse().ok());
    let episode = parts.next().and_then(|value| value.parse().ok());
    if parts.next().is_some() {
        return Err(Error::new("invalid Silip episode key"));
    }
    Ok((
        id.ok_or_else(|| Error::new("missing TMDB id"))?,
        season.ok_or_else(|| Error::new("missing season"))?,
        episode.ok_or_else(|| Error::new("missing episode"))?,
    ))
}

type ResolvedUrl = (&'static str, u64, Option<u64>, Option<u64>);

fn parse_silip_url(candidate: &str) -> Result<Option<ResolvedUrl>> {
    let url = Url::parse(candidate).map_err(url_error)?;
    if url.scheme() != "https" || url.host_str() != Some("silip.pages.dev") {
        return Ok(None);
    }
    let parts = url
        .path_segments()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [kind @ ("movie" | "tv"), id] => {
            let Ok(id) = id.parse() else {
                return Ok(None);
            };
            let kind = if *kind == "tv" { "tv" } else { "movie" };
            Ok(Some((kind, id, None, None)))
        }
        ["watch", id] => {
            let Ok(id) = id.parse() else {
                return Ok(None);
            };
            let query = url
                .query_pairs()
                .collect::<std::collections::BTreeMap<_, _>>();
            let kind = if query.get("type").map(|value| value.as_ref()) == Some("tv") {
                "tv"
            } else {
                "movie"
            };
            let season = query.get("season").and_then(|value| value.parse().ok());
            let episode = query.get("episode").and_then(|value| value.parse().ok());
            Ok(Some((
                kind,
                id,
                season,
                episode.or((kind == "movie").then_some(1)),
            )))
        }
        _ => Ok(None),
    }
}

fn validate_vidcore_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    if url.scheme() != "https" || url.host_str() != Some("vidcore.org") {
        return Err(Error::new("unexpected Silip player URL"));
    }
    Ok(())
}

fn validate_vidcore_media_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || !(host == "vidcore.org"
            || host == "edgeone.dev"
            || host.ends_with(".edgeone.dev")
            || host == "cdn.fastcloud.live")
    {
        return Err(Error::new("unexpected Silip media URL"));
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
manatan_sdk::export_extension!(manatan_sdk::Extension::new().video("silip", Silip));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_filters_tmdb_results() {
        let page = parse_tmdb_page(
            &json!({
                "page": 1,
                "total_pages": 2,
                "results": [
                    {"id": 1, "media_type": "movie", "title": "Film", "original_language": "tl"},
                    {"id": 2, "media_type": "tv", "name": "Series", "original_language": "fil"}
                ]
            }),
            None,
            false,
        )
        .unwrap();
        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.entries[0].key, "movie:1");
        assert_eq!(page.entries[1].key, "tv:2");
        assert!(page.has_next_page);

        let filtered = filter_search_results(&json!({
            "results": [
                {"id": 1, "media_type": "movie", "original_language": "tl"},
                {"id": 2, "media_type": "tv", "original_language": "en"},
                {"id": 3, "media_type": "person", "original_language": "tl"}
            ]
        }));
        assert_eq!(filtered["results"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn merges_adult_catalogs_without_duplicates() {
        let merged = merge_tmdb_pages(
            &json!({"page": 1, "total_pages": 2, "results": [
                {"id": 1, "popularity": 2.0}, {"id": 2, "popularity": 3.0}
            ]}),
            &json!({"page": 1, "total_pages": 3, "results": [
                {"id": 1, "popularity": 2.0}, {"id": 3, "popularity": 1.0}
            ]}),
            "popular",
        );
        assert_eq!(merged["results"].as_array().unwrap().len(), 3);
        assert_eq!(merged["results"][0]["id"], 2);
        assert_eq!(merged["total_pages"], 3);
    }

    #[test]
    fn recognizes_r18_certifications() {
        assert!(is_r18(
            &json!({"release_dates": {"results": [{
                "iso_3166_1": "PH", "release_dates": [{"certification": "R-18"}]
            }]}}),
            "movie"
        ));
        assert!(is_r18(
            &json!({"content_ratings": {"results": [{
                "iso_3166_1": "PH", "rating": "R-18"
            }]}}),
            "tv"
        ));
    }

    #[test]
    fn resolves_owned_item_and_episode_urls() {
        assert_eq!(
            parse_silip_url("https://silip.pages.dev/movie/123").unwrap(),
            Some(("movie", 123, None, None))
        );
        assert_eq!(
            parse_silip_url("https://silip.pages.dev/watch/456?type=tv&season=2&episode=3")
                .unwrap(),
            Some(("tv", 456, Some(2), Some(3)))
        );
        assert!(parse_silip_url("https://example.com/movie/123")
            .unwrap()
            .is_none());
    }

    #[test]
    fn keeps_only_root_video_manifests() {
        let candidates = [
            (
                "https://media.example/audio.m3u8",
                "#EXTM3U\n#EXTINF:6,\na.m4s\n",
            ),
            (
                "https://media.example/master.m3u8",
                "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=6000000\nvideo.m3u8\n",
            ),
            (
                "https://media.example/video.m3u8",
                "#EXTM3U\n#EXTINF:6,\nv.m4s\n",
            ),
        ]
        .into_iter()
        .map(|(url, body)| {
            (
                VideoStream {
                    url: url.to_string(),
                    is_hls: true,
                    ..VideoStream::default()
                },
                body.to_string(),
            )
        })
        .collect();
        let streams = root_manifest_streams(candidates, None);
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].url, "https://media.example/master.m3u8");
    }

    #[test]
    fn preserves_vidcore_manifest_headers_and_rejects_other_hosts() {
        let response = WebViewExtractResponse {
            captured_requests: vec![WebViewCapturedRequest {
                url: "https://4565565.edgeone.dev/px.php?url=movie.m3u8".to_string(),
                method: "GET".to_string(),
                headers: vec![
                    (
                        "referer".to_string(),
                        "https://4565565.edgeone.dev/embed/movie/593961".to_string(),
                    ),
                    ("user-agent".to_string(), "Silip test browser".to_string()),
                    ("x-ignored".to_string(), "not forwarded".to_string()),
                ],
                ..WebViewCapturedRequest::default()
            }],
            ..WebViewExtractResponse::default()
        };
        let streams = streams_from_capture(&response, "https://vidcore.org/embed/movie/593961")
            .expect("VidCore capture should produce a stream");
        assert_eq!(streams.len(), 1);
        assert_eq!(
            streams[0].headers.get("Referer").map(String::as_str),
            Some("https://4565565.edgeone.dev/embed/movie/593961")
        );
        assert_eq!(
            streams[0].headers.get("User-Agent").map(String::as_str),
            Some("Silip test browser")
        );
        assert!(!streams[0].headers.contains_key("x-ignored"));

        assert!(validate_vidcore_media_url("https://other.example/movie.m3u8").is_err());
    }
}
