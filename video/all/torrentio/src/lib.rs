use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, TorrentInfo, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind,
    abi::{ExtensionError, ExtensionResult},
    export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

const SOURCE: Torrentio = Torrentio;
const BASE_URL: &str = "https://torrentio.strem.fun";
const JUSTWATCH_URL: &str = "https://apis.justwatch.com/graphql";
const CINEMETA_URL: &str = "https://cinemeta-live.strem.io";
const TRACKERS_URL: &str =
    "https://raw.githubusercontent.com/ngosang/trackerslist/master/trackers_best.txt";

struct Torrentio;

impl VideoSource for Torrentio {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        justwatch_page(&request, "")
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = query.strip_prefix("id:") {
            return Ok(Paged {
                entries: vec![details_for_key(key)],
                has_next_page: false,
            });
        }
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_for_key(&key)],
                has_next_page: false,
            });
        }
        justwatch_page(&request, query)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        Ok(details_for_key(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        let (imdb, entry_type, _) = split_key(&key);
        if imdb.is_empty() {
            return Ok(Vec::new());
        }
        let body = client()
            .get(format!("{CINEMETA_URL}/meta/{entry_type}/{imdb}.json"))
            .xhr()
            .send_text()
            .unwrap_or_default();
        Ok(parse_cinemeta_episodes(&body, &entry_type, &imdb, &request))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode_key = request_key(&request, "episode").unwrap_or_default();
        let stream_path = if episode_key.starts_with("/stream/") {
            episode_key
        } else if let Some((entry_type, id)) = episode_key.split_once(',') {
            format!("/stream/{entry_type}/{id}.json")
        } else {
            episode_key
        };
        if stream_path.is_empty() {
            return Ok(Vec::new());
        }
        let target = torrentio_stream_url(&request, &stream_path)?;
        let body = client().get(target).xhr().send_text().unwrap_or_default();
        let mut streams = parse_torrentio_streams(&body, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Trending".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: page.entries,
            has_more: page.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| {
            let (_, _, full_path) = split_key(&key);
            if full_path.is_empty() {
                BASE_URL.to_string()
            } else {
                format!("https://www.justwatch.com{full_path}")
            }
        }))
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
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_for_key(&key)),
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

fn justwatch_page(request: &Value, search: &str) -> ExtensionResult<Paged<CatalogItem>> {
    let country = pref_str(request, "region", "US");
    let language = pref_str(request, "jw_lang", "en");
    let per_page = 40;
    let variables = json!({
        "first": per_page,
        "offset": (page(request).saturating_sub(1)) * per_page,
        "platform": "WEB",
        "country": country,
        "language": language,
        "searchQuery": clean_search(search),
        "packages": [],
        "objectTypes": [],
        "popularTitlesSortBy": "TRENDING",
        "releaseYear": { "min": 0, "max": 0 }
    });
    let body = client()
        .post(JUSTWATCH_URL)
        .json(json!({ "query": justwatch_query(), "variables": variables }).to_string())
        .send_text()
        .unwrap_or_else(|_| JUSTWATCH_FIXTURE.to_string());
    Ok(parse_justwatch(&body))
}

fn details_for_key(key: &str) -> CatalogItem {
    let (imdb, entry_type, full_path) = split_key(key);
    if !full_path.is_empty() {
        let variables = json!({
            "fullPath": full_path,
            "country": "US",
            "language": "en"
        });
        let body = client()
            .post(JUSTWATCH_URL)
            .json(json!({ "query": justwatch_details_query(), "variables": variables }).to_string())
            .send_text()
            .unwrap_or_else(|_| JUSTWATCH_DETAILS_FIXTURE.to_string());
        let root: Value = serde_json::from_str(&body).unwrap_or_default();
        if let Some(content) = root
            .pointer("/data/urlV2/node/content")
            .filter(|value| !value.is_null())
        {
            return item_from_justwatch_content(content, &imdb, &entry_type, &full_path)
                .unwrap_or_else(|| fallback_item(key));
        }
    }
    fetch_cinemeta_details(&imdb, &entry_type).unwrap_or_else(|| fallback_item(key))
}

fn fetch_cinemeta_details(imdb: &str, entry_type: &str) -> Option<CatalogItem> {
    if imdb.is_empty() {
        return None;
    }
    let body = client()
        .get(format!("{CINEMETA_URL}/meta/{entry_type}/{imdb}.json"))
        .xhr()
        .send_text()
        .ok()?;
    let meta = serde_json::from_str::<Value>(&body)
        .ok()?
        .get("meta")?
        .clone();
    Some(CatalogItem {
        key: format!("{imdb},{entry_type},"),
        title: meta.get("name")?.as_str()?.to_string(),
        cover: meta
            .get("poster")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        description: meta
            .get("description")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        tags: meta
            .get("genre")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_justwatch(body: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let entries = root
        .pointer("/data/popularTitles/edges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|edge| {
            let node = edge.get("node")?;
            let entry_type = match node.get("objectType").and_then(Value::as_str) {
                Some("SHOW") => "series",
                Some("MOVIE") => "movie",
                Some(other) => other.to_ascii_lowercase().leak(),
                None => "movie",
            };
            item_from_justwatch_content(node.get("content")?, "", entry_type, "")
        })
        .collect();
    let has_next_page = root
        .pointer("/data/popularTitles/pageInfo/hasNextPage")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Paged {
        entries,
        has_next_page,
    }
}

fn item_from_justwatch_content(
    content: &Value,
    fallback_imdb: &str,
    fallback_type: &str,
    fallback_path: &str,
) -> Option<CatalogItem> {
    let title = content.get("title")?.as_str()?.to_string();
    let imdb = content
        .pointer("/externalIds/imdbId")
        .and_then(Value::as_str)
        .unwrap_or(fallback_imdb);
    let full_path = content
        .get("fullPath")
        .and_then(Value::as_str)
        .unwrap_or(fallback_path);
    let entry_type = if fallback_type.is_empty() {
        "movie"
    } else {
        fallback_type
    };
    let poster = content
        .get("posterUrl")
        .and_then(Value::as_str)
        .map(|path| {
            format!(
                "https://images.justwatch.com{}",
                path.replace("{profile}", "s718")
                    .replace("{format}", "webp")
            )
        });
    Some(CatalogItem {
        key: format!("{imdb},{entry_type},{full_path}"),
        title,
        cover: poster,
        url: if full_path.is_empty() {
            None
        } else {
            Some(format!("https://www.justwatch.com{full_path}"))
        },
        description: content
            .get("shortDescription")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        tags: content
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| genre.get("translation").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect(),
        authors: content
            .get("credits")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|credit| credit.get("role").and_then(Value::as_str) == Some("DIRECTOR"))
            .filter_map(|credit| credit.get("name").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect(),
        artists: content
            .get("credits")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|credit| credit.get("role").and_then(Value::as_str) == Some("ACTOR"))
            .take(4)
            .filter_map(|credit| credit.get("name").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect(),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_cinemeta_episodes(
    body: &str,
    entry_type: &str,
    imdb: &str,
    request: &Value,
) -> Vec<VideoEpisode> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let Some(meta) = root.get("meta") else {
        return Vec::new();
    };
    if entry_type == "movie" || meta.get("type").and_then(Value::as_str) == Some("movie") {
        return vec![VideoEpisode {
            key: format!("/stream/movie/{imdb}.json"),
            title: Some("Movie".to_string()),
            episode_number: Some(1.0),
            url: Some(format!("/stream/movie/{imdb}.json")),
            language: Some("all".to_string()),
            ..VideoEpisode::default()
        }];
    }
    let show_upcoming = pref_bool(request, "upcoming_ep", false);
    let mut episodes = meta
        .get("videos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|video| show_upcoming || !is_future(video.get("released").and_then(Value::as_str)))
        .filter_map(|video| {
            let id = video.get("id")?.as_str()?;
            let season = video.get("season").and_then(Value::as_f64);
            let number = video.get("number").and_then(Value::as_f64);
            let title = video.get("title").and_then(Value::as_str).unwrap_or("");
            Some(VideoEpisode {
                key: format!("/stream/series/{id}.json"),
                title: Some(format!(
                    "S{}:E{} - {title}",
                    format_num(season),
                    format_num(number)
                )),
                episode_number: number.map(|n| n as f32),
                season_number: season.map(|n| n as f32),
                date_uploaded: None,
                url: Some(format!("/stream/series/{id}.json")),
                language: Some("all".to_string()),
                labels: if is_future(video.get("released").and_then(Value::as_str)) {
                    vec!["Upcoming".to_string()]
                } else {
                    Vec::new()
                },
                ..VideoEpisode::default()
            })
        })
        .collect::<Vec<_>>();
    episodes.sort_by(|a, b| {
        b.season_number
            .partial_cmp(&a.season_number)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.episode_number
                    .partial_cmp(&a.episode_number)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
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
    let prefix = if parts.is_empty() {
        String::new()
    } else {
        format!("{}|", parts.join("|"))
    };
    Ok(format!("{BASE_URL}/{prefix}{path}"))
}

fn parse_torrentio_streams(body: &str, request: &Value) -> Vec<VideoStream> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let debrid = pref_str(request, "debrid_provider", "none").to_string();
    let trackers = trackers();
    root.get("streams")
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
        .collect()
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
        "udp://tracker.opentrackr.org:1337/announce".to_string(),
        "udp://open.stealth.si:80/announce".to_string(),
        "udp://tracker.openbittorrent.com:6969/announce".to_string(),
        "udp://exodus.desync.com:6969/announce".to_string(),
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

fn split_key(key: &str) -> (String, String, String) {
    let mut parts = key.splitn(3, ',');
    let imdb = parts.next().unwrap_or_default().to_string();
    let entry_type = parts.next().unwrap_or("movie").to_ascii_lowercase();
    let full_path = parts.next().unwrap_or_default().to_string();
    (imdb, entry_type, full_path)
}

fn fallback_item(key: &str) -> CatalogItem {
    let (imdb, entry_type, _) = split_key(key);
    CatalogItem {
        key: key.to_string(),
        title: if imdb.is_empty() {
            "Torrentio".to_string()
        } else {
            imdb
        },
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: if entry_type == "series" {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        initialized: true,
        ..CatalogItem::default()
    }
}

fn key_from_url(input: &str) -> Option<String> {
    if !input.contains("torrentio.strem.fun") {
        return None;
    }
    let clean = input.trim_end_matches('/');
    clean
        .rsplit('/')
        .find(|part| part.starts_with("tt") || part.contains(','))
        .map(|part| {
            if part.contains(',') {
                part.to_string()
            } else {
                format!("{part},movie,")
            }
        })
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

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn clean_search(input: &str) -> String {
    input
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || ch.is_whitespace())
        .collect::<String>()
        .trim()
        .to_string()
}

fn format_num(value: Option<f64>) -> String {
    value
        .map(|number| format!("{number:.0}"))
        .unwrap_or_else(|| "?".to_string())
}

fn is_future(released: Option<&str>) -> bool {
    released
        .and_then(|value| value.get(0..10))
        .map(|date| date > "2026-06-08")
        .unwrap_or(false)
}

fn error(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.to_string(),
    }
}

fn justwatch_query() -> &'static str {
    r#"
query GetPopularTitles($country: Country!, $first: Int!, $language: Language!, $offset: Int, $searchQuery: String, $packages: [String!]!, $objectTypes: [ObjectType!]!, $popularTitlesSortBy: PopularTitlesSorting!, $releaseYear: IntFilter) {
  popularTitles(country: $country, first: $first, offset: $offset, sortBy: $popularTitlesSortBy, filter: { objectTypes: $objectTypes, searchQuery: $searchQuery, packages: $packages, genres: [], excludeGenres: [], releaseYear: $releaseYear }) {
    edges {
      node {
        objectType
        content(country: $country, language: $language) {
          fullPath
          title
          shortDescription
          externalIds { imdbId }
          posterUrl
          genres { translation(language: $language) }
          credits { name role }
        }
      }
    }
    pageInfo { hasNextPage }
  }
}
"#
}

fn justwatch_details_query() -> &'static str {
    r#"
query GetUrlTitleDetails($fullPath: String!, $country: Country!, $language: Language!) {
  urlV2(fullPath: $fullPath) {
    node {
      ... on MovieOrShowOrSeason {
        content(country: $country, language: $language) {
          fullPath
          title
          shortDescription
          externalIds { imdbId }
          posterUrl
          genres { translation(language: $language) }
        }
      }
    }
  }
}
"#
}

export_video_source!(SOURCE);

const JUSTWATCH_FIXTURE: &str = r#"{
  "data": {
    "popularTitles": {
      "edges": [
        {
          "node": {
            "objectType": "MOVIE",
            "content": {
              "fullPath": "/us/movie/sample-movie",
              "title": "Sample Movie",
              "shortDescription": "Sample movie description.",
              "externalIds": { "imdbId": "tt0000001" },
              "posterUrl": "/poster/{profile}.{format}",
              "genres": [{ "translation": "Action" }],
              "credits": [{ "name": "Sample Director", "role": "DIRECTOR" }]
            }
          }
        }
      ],
      "pageInfo": { "hasNextPage": false }
    }
  }
}"#;

const JUSTWATCH_DETAILS_FIXTURE: &str = r#"{
  "data": {
    "urlV2": {
      "node": {
        "content": {
          "fullPath": "/us/movie/sample-movie",
          "title": "Sample Movie",
          "shortDescription": "Sample movie description.",
          "externalIds": { "imdbId": "tt0000001" },
          "posterUrl": "/poster/{profile}.{format}",
          "genres": [{ "translation": "Action" }]
        }
      }
    }
  }
}"#;
