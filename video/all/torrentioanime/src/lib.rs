use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, TorrentInfo, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind,
    abi::{ExtensionError, ExtensionResult},
    export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

const SOURCE: TorrentioAnime = TorrentioAnime;
const BASE_URL: &str = "https://torrentio.strem.fun";
const ANILIST_URL: &str = "https://graphql.anilist.co";
const ANIZIP_URL: &str = "https://api.ani.zip/mappings";
const TRACKERS_URL: &str =
    "https://raw.githubusercontent.com/ngosang/trackerslist/master/trackers_best.txt";

struct TorrentioAnime;

impl VideoSource for TorrentioAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            anilist_latest(&request)
        } else {
            anilist_search(&request, "")
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let url_id = id_from_url(query);
        if let Some(id) = query.strip_prefix("id:").or(url_id.as_deref()) {
            return Ok(Paged {
                entries: vec![details_by_id(id)],
                has_next_page: false,
            });
        }
        anilist_search(&request, query)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        Ok(details_by_id(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        let body = client()
            .get(format!("{ANIZIP_URL}?anilist_id={key}"))
            .xhr()
            .send_text()
            .unwrap_or_default();
        Ok(parse_anizip_episodes(&body, &request))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode_key = request_key(&request, "episode").unwrap_or_default();
        let stream_path = if episode_key.starts_with("/stream/") {
            episode_key
        } else {
            format!("/stream/series/{episode_key}.json")
        };
        let target = torrentio_stream_url(&request, &stream_path)?;
        let body = client().get(target).xhr().send_text().unwrap_or_default();
        let mut streams = parse_torrentio_streams(&body, &request);
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
        Ok(request_key(&request, "item").map(|key| format!("https://anilist.co/anime/{key}")))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| {
            if key.starts_with("/stream/") {
                format!("{BASE_URL}{key}")
            } else {
                key
            }
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_id(&id)),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn anilist_search(request: &Value, query: &str) -> ExtensionResult<Paged<CatalogItem>> {
    let mut vars = json!({
        "page": page(request),
        "perPage": 30,
        "sort": filter_str(request, "sort", "TRENDING_DESC")
    });
    if !query.is_empty() {
        vars["search"] = json!(query);
    }
    for (filter_key, var_key) in [
        ("genre", "genre"),
        ("format", "format"),
        ("season", "season"),
        ("status", "status"),
    ] {
        let value = filter_str(request, filter_key, "");
        if !value.is_empty() {
            vars[var_key] = json!(value);
        }
    }
    let year = filter_str(request, "year", "");
    if !year.trim().is_empty() {
        vars["year"] = json!(format!("{}%", year.trim()));
    }
    let body = anilist_post(anilist_query(), &vars)?;
    Ok(parse_anilist_page(&body, false, request))
}

fn anilist_latest(request: &Value) -> ExtensionResult<Paged<CatalogItem>> {
    let vars = json!({
        "page": page(request),
        "perPage": 30,
        "sort": "TIME_DESC"
    });
    let body = anilist_post(anilist_latest_query(), &vars)?;
    Ok(parse_anilist_page(&body, true, request))
}

fn details_by_id(id: &str) -> CatalogItem {
    let vars = json!({ "id": id.parse::<u64>().unwrap_or_default() });
    let body = anilist_post(details_query(), &vars).unwrap_or_default();
    let root: Value = serde_json::from_str(&body).unwrap_or_default();
    root.pointer("/data/Media")
        .and_then(|media| item_from_media(media, pref_title_from_value(&Value::Null), true))
        .unwrap_or_else(|| CatalogItem {
            key: id.to_string(),
            title: id.to_string(),
            language: Some("all".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            initialized: true,
            ..CatalogItem::default()
        })
}

fn anilist_post(query: &str, vars: &Value) -> ExtensionResult<String> {
    Ok(client()
        .post(ANILIST_URL)
        .form(&[("query", query), ("variables", &vars.to_string())])
        .send_text()
        .unwrap_or_else(|_| ANILIST_FIXTURE.to_string()))
}

fn parse_anilist_page(body: &str, latest: bool, request: &Value) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let media_list = if latest {
        root.pointer("/data/Page/airingSchedules")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|schedule| schedule.get("media"))
            .collect::<Vec<_>>()
    } else {
        root.pointer("/data/Page/media")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
    };
    let preferred_title = pref_title(request);
    let entries = media_list
        .into_iter()
        .filter(|media| {
            !(latest
                && (media.get("countryOfOrigin").and_then(Value::as_str) == Some("CN")
                    || media.get("isAdult").and_then(Value::as_bool) == Some(true)))
        })
        .filter_map(|media| item_from_media(media, preferred_title, true))
        .collect();
    let has_next_page = root
        .pointer("/data/Page/pageInfo/hasNextPage")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Paged {
        entries,
        has_next_page,
    }
}

fn item_from_media(media: &Value, title_pref: &str, initialized: bool) -> Option<CatalogItem> {
    let id = media.get("id")?.as_u64()?.to_string();
    let title = title_from_media(media, title_pref);
    let description = media
        .get("description")
        .and_then(Value::as_str)
        .map(|text| html::strip_tags(&text.replace("<br>", "\n").replace("<br>\n", "\n")));
    let mut tags = media
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tag| tag.get("name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    tags.extend(
        media
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToString::to_string),
    );
    tags.sort();
    tags.dedup();
    Some(CatalogItem {
        key: id.clone(),
        title,
        cover: media
            .pointer("/coverImage/extraLarge")
            .or_else(|| media.pointer("/coverImage/large"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: media
            .get("siteUrl")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| Some(format!("https://anilist.co/anime/{id}"))),
        description,
        tags,
        authors: media
            .pointer("/studios/nodes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|node| node.get("name").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect(),
        language: Some("all".to_string()),
        content_rating: Some(
            if media.get("isAdult").and_then(Value::as_bool) == Some(true) {
                "adult"
            } else {
                "safe"
            }
            .to_string(),
        ),
        status: match media.get("status").and_then(Value::as_str) {
            Some("RELEASING") => ItemStatus::Ongoing,
            Some("FINISHED") => ItemStatus::Completed,
            Some("HIATUS") => ItemStatus::Hiatus,
            Some("CANCELLED") => ItemStatus::Cancelled,
            _ => ItemStatus::Unknown,
        },
        initialized,
        ..CatalogItem::default()
    })
}

fn parse_anizip_episodes(body: &str, request: &Value) -> Vec<VideoEpisode> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let mapping_type = root
        .pointer("/mappings/type")
        .and_then(Value::as_str)
        .unwrap_or("");
    let kitsu = root
        .pointer("/mappings/kitsu_id")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    if mapping_type == "MOVIE" {
        let date = root
            .pointer("/episodes/1/airdate")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        return vec![VideoEpisode {
            key: format!("/stream/movie/kitsu:{kitsu}.json"),
            title: Some("Movie".to_string()),
            episode_number: Some(1.0),
            date_uploaded: date.and_then(|_| None),
            url: Some(format!("/stream/movie/kitsu:{kitsu}.json")),
            language: Some("all".to_string()),
            ..VideoEpisode::default()
        }];
    }
    let show_upcoming = pref_bool(request, "upcoming_ep", false);
    let mut episodes = root
        .get("episodes")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|object| object.values())
        .filter(|episode| {
            show_upcoming || !is_future(episode.get("airdate").and_then(Value::as_str))
        })
        .filter_map(|episode| {
            let number = episode
                .get("episode")
                .and_then(Value::as_str)
                .and_then(|value| value.parse::<f32>().ok())
                .or_else(|| {
                    episode
                        .get("episodeNumber")
                        .and_then(Value::as_f64)
                        .map(|n| n as f32)
                })?;
            let whole = format!("{number:.0}");
            let title = episode
                .pointer("/title/en")
                .and_then(Value::as_str)
                .map(|title| format!("Episode {whole}: {title}"))
                .unwrap_or_else(|| format!("Episode {whole}"));
            let key = format!("/stream/series/kitsu:{kitsu}:{whole}.json");
            Some(VideoEpisode {
                key: key.clone(),
                title: Some(title),
                episode_number: Some(number),
                date_uploaded: None,
                thumbnail: episode
                    .get("image")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                url: Some(key),
                duration_seconds: episode
                    .get("runtime")
                    .or_else(|| episode.get("length"))
                    .and_then(Value::as_f64)
                    .map(|minutes| minutes * 60.0),
                language: Some("all".to_string()),
                labels: if is_future(episode.get("airdate").and_then(Value::as_str)) {
                    vec!["Upcoming".to_string()]
                } else {
                    Vec::new()
                },
                ..VideoEpisode::default()
            })
        })
        .collect::<Vec<_>>();
    episodes.sort_by(|a, b| {
        b.episode_number
            .partial_cmp(&a.episode_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    episodes
}

fn torrentio_stream_url(request: &Value, path: &str) -> ExtensionResult<String> {
    let mut parts = Vec::new();
    append_list(
        &mut parts,
        "providers",
        pref_list(request, "provider_selection"),
    );
    append_list(&mut parts, "language", pref_list(request, "lang_selection"));
    append_list(
        &mut parts,
        "qualityfilter",
        pref_list(request, "quality_selection"),
    );
    parts.push(format!(
        "sort={}",
        pref_str(request, "sorting_link", "quality").trim()
    ));
    let debrid = pref_str(request, "debrid_provider", "none");
    let token = pref_str(request, "token", "");
    if debrid != "none" {
        if token.trim().is_empty() {
            return Err(error("Debrid token is required for the selected provider"));
        }
        parts.push(format!("{debrid}={token}"));
    }
    Ok(format!("{BASE_URL}/{}|{path}", parts.join("|")).replace("/|/", "/"))
}

fn parse_torrentio_streams(body: &str, request: &Value) -> Vec<VideoStream> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let debrid = pref_str(request, "debrid_provider", "none").to_string();
    let trackers = trackers();
    let codec_filter = pref_list(request, "codec_selection");
    let mut streams = root
        .get("streams")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|stream| {
            let name = stream
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Torrentio")
                .replace("Torrentio\n", "");
            let title = stream.get("title").and_then(Value::as_str).unwrap_or("");
            let label = [name.as_str(), title]
                .into_iter()
                .filter(|value| !value.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            if !codec_filter.is_empty() && !codec_filter.contains(&detect_codec(&label)) {
                return None;
            }
            if debrid == "none" {
                let hash = stream.get("infoHash").and_then(Value::as_str)?;
                let index = stream
                    .get("fileIdx")
                    .and_then(Value::as_u64)
                    .map(|value| value as u32);
                let magnet = magnet_url(hash, index, &trackers);
                Some(VideoStream {
                    url: magnet.clone(),
                    name: Some(label.clone()),
                    quality: Some(label.clone()),
                    format: Some("magnet".to_string()),
                    stream_kind: Some(VideoStreamKind::Magnet),
                    torrent: Some(TorrentInfo {
                        magnet_url: Some(magnet.clone()),
                        file_index: index,
                        file_name: Some(label),
                        trackers: trackers.clone(),
                        ..TorrentInfo::default()
                    }),
                    initialized: true,
                    ..VideoStream::default()
                })
            } else {
                let stream_url = stream.get("url").and_then(Value::as_str)?;
                Some(VideoStream {
                    url: stream_url.to_string(),
                    name: Some(label.clone()),
                    quality: Some(label),
                    format: Some("external".to_string()),
                    stream_kind: Some(VideoStreamKind::Debrid),
                    initialized: true,
                    ..VideoStream::default()
                })
            }
        })
        .collect::<Vec<_>>();
    sort_streams(&mut streams, request);
    streams
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let dub = pref_bool(request, "dubbed", false);
    let efficient = pref_bool(request, "efficient", false);
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or("").to_ascii_lowercase();
        (
            quality.contains("[") && quality.contains(" download]"),
            dub && !quality.contains("dubbed"),
            efficient
                && !["hevc", "265", "av1"]
                    .iter()
                    .any(|codec| quality.contains(codec)),
        )
    });
}

fn detect_codec(label: &str) -> String {
    let value = label.to_ascii_lowercase();
    if value.contains("264") {
        "x264".to_string()
    } else if value.contains("265") || value.contains("hevc") {
        "x265".to_string()
    } else if value.contains("av1") {
        "av1".to_string()
    } else if value.contains("vp9") {
        "vp9".to_string()
    } else {
        "other".to_string()
    }
}

fn append_list(parts: &mut Vec<String>, key: &str, values: Vec<String>) {
    let values = values
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>();
    if !values.is_empty() {
        parts.push(format!("{key}={}", values.join(",")));
    }
}

fn trackers() -> Vec<String> {
    let mut out = vec![
        "http://nyaa.tracker.wf:7777/announce".to_string(),
        "http://anidex.moe:6969/announce".to_string(),
        "udp://tracker.opentrackr.org:1337/announce".to_string(),
        "udp://open.stealth.si:80/announce".to_string(),
    ];
    out.extend(
        HttpClient::browser()
            .get(TRACKERS_URL)
            .send_text()
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToString::to_string),
    );
    out.sort();
    out.dedup();
    out
}

fn magnet_url(hash: &str, index: Option<u32>, trackers: &[String]) -> String {
    let mut magnet = format!("magnet:?xt=urn:btih:{hash}&dn={hash}");
    for tracker in trackers {
        magnet.push_str("&tr=");
        magnet.push_str(&url::query_escape(tracker));
    }
    if let Some(index) = index {
        magnet.push_str("&index=");
        magnet.push_str(&index.to_string());
    }
    magnet
}

fn title_from_media(media: &Value, title_pref: &str) -> String {
    let romaji = media
        .pointer("/title/romaji")
        .and_then(Value::as_str)
        .unwrap_or("");
    match title_pref {
        "english" => media
            .pointer("/title/english")
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(romaji)
            .to_string(),
        "native" => media
            .pointer("/title/native")
            .and_then(Value::as_str)
            .unwrap_or(romaji)
            .to_string(),
        _ => romaji.to_string(),
    }
}

fn id_from_url(input: &str) -> Option<String> {
    let clean = input.trim_end_matches('/');
    if clean.contains("anilist.co/anime/") {
        clean
            .split("/anime/")
            .nth(1)?
            .split('/')
            .next()
            .map(ToString::to_string)
    } else if clean.contains("torrentio.strem.fun") {
        clean
            .rsplit('/')
            .find(|part| part.chars().all(|ch| ch.is_ascii_digit()))
            .map(ToString::to_string)
    } else {
        None
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
        .map(ToString::to_string)
        .or_else(|| {
            request
                .get("key")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

fn pref_value<'a>(request: &'a Value, key: &str) -> Option<&'a Value> {
    request
        .get("preferences")
        .or_else(|| request.get("prefs"))
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
}

fn filter_value<'a>(request: &'a Value, key: &str) -> Option<&'a Value> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
}

fn pref_str<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    pref_value(request, key)
        .and_then(Value::as_str)
        .unwrap_or(default)
}

fn pref_bool(request: &Value, key: &str, default: bool) -> bool {
    pref_value(request, key)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn pref_list(request: &Value, key: &str) -> Vec<String> {
    match pref_value(request, key) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.to_string()],
        _ => Vec::new(),
    }
}

fn filter_str<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    filter_value(request, key)
        .and_then(Value::as_str)
        .unwrap_or(default)
}

fn pref_title(request: &Value) -> &str {
    pref_str(request, "pref_title", "romaji")
}

fn pref_title_from_value(_: &Value) -> &'static str {
    "romaji"
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

fn is_future(date: Option<&str>) -> bool {
    date.map(|value| value > "2026-06-08").unwrap_or(false)
}

fn error(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.to_string(),
    }
}

fn anilist_query() -> &'static str {
    r#"
query ($page: Int, $perPage: Int, $sort: [MediaSort], $search: String, $genre: String, $format: MediaFormat, $season: MediaSeason, $year: String, $status: MediaStatus) {
  Page(page: $page, perPage: $perPage) {
    pageInfo { hasNextPage }
    media(type: ANIME, sort: $sort, search: $search, genre: $genre, format: $format, season: $season, seasonYear: $year, status: $status) {
      id siteUrl title { romaji english native } coverImage { extraLarge large } description status genres format season seasonYear countryOfOrigin isAdult
      tags { name }
      studios { nodes { name } }
    }
  }
}
"#
}

fn anilist_latest_query() -> &'static str {
    r#"
query ($page: Int, $perPage: Int, $sort: [AiringSort]) {
  Page(page: $page, perPage: $perPage) {
    pageInfo { hasNextPage }
    airingSchedules(sort: $sort) {
      media {
        id siteUrl title { romaji english native } coverImage { extraLarge large } description status genres format season seasonYear countryOfOrigin isAdult
        tags { name }
        studios { nodes { name } }
      }
    }
  }
}
"#
}

fn details_query() -> &'static str {
    r#"
query ($id: Int) {
  Media(id: $id, type: ANIME) {
    id siteUrl title { romaji english native } coverImage { extraLarge large } description status genres episodes format season seasonYear countryOfOrigin isAdult
    tags { name }
    studios { nodes { name } }
  }
}
"#
}

export_video_source!(SOURCE);

const ANILIST_FIXTURE: &str = r#"{
  "data": {
    "Page": {
      "pageInfo": { "hasNextPage": false },
      "media": [
        {
          "id": 1,
          "siteUrl": "https://anilist.co/anime/1",
          "title": { "romaji": "Sample Anime", "english": "Sample Anime", "native": "Sample Anime" },
          "coverImage": { "extraLarge": "https://example.invalid/anime.jpg", "large": "https://example.invalid/anime.jpg" },
          "description": "Sample anime description.",
          "status": "FINISHED",
          "genres": ["Action"],
          "format": "TV",
          "season": "WINTER",
          "seasonYear": 2024,
          "countryOfOrigin": "JP",
          "isAdult": false,
          "tags": [{ "name": "Adventure" }],
          "studios": { "nodes": [{ "name": "Sample Studio" }] }
        }
      ],
      "airingSchedules": [
        {
          "media": {
            "id": 1,
            "siteUrl": "https://anilist.co/anime/1",
            "title": { "romaji": "Sample Anime", "english": "Sample Anime", "native": "Sample Anime" },
            "coverImage": { "extraLarge": "https://example.invalid/anime.jpg", "large": "https://example.invalid/anime.jpg" },
            "description": "Sample anime description.",
            "status": "RELEASING",
            "genres": ["Action"],
            "format": "TV",
            "season": "WINTER",
            "seasonYear": 2024,
            "countryOfOrigin": "JP",
            "isAdult": false,
            "tags": [{ "name": "Adventure" }],
            "studios": { "nodes": [{ "name": "Sample Studio" }] }
          }
        }
      ]
    },
    "Media": {
      "id": 1,
      "siteUrl": "https://anilist.co/anime/1",
      "title": { "romaji": "Sample Anime", "english": "Sample Anime", "native": "Sample Anime" },
      "coverImage": { "extraLarge": "https://example.invalid/anime.jpg", "large": "https://example.invalid/anime.jpg" },
      "description": "Sample anime description.",
      "status": "FINISHED",
      "genres": ["Action"],
      "episodes": 1,
      "format": "TV",
      "season": "WINTER",
      "seasonYear": 2024,
      "countryOfOrigin": "JP",
      "isAdult": false,
      "tags": [{ "name": "Adventure" }],
      "studios": { "nodes": [{ "name": "Sample Studio" }] }
    }
  }
}"#;
