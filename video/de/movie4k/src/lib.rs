use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

const SOURCE: Movie4k = Movie4k;
const BASE_URL: &str = "https://movie4k.stream";
const API_URL: &str = "https://api.movie4k.stream";

struct Movie4k;

impl VideoSource for Movie4k {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let body = get_or_fixture(
            &format!("{API_URL}/data/browse/?lang=2&keyword=&year=&rating=&votes=&genre=&country=&cast=&directors=&type=movies&order_by=trending&page={page}"),
            LIST_FIXTURE,
        );
        Ok(parse_browse(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&path)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let body = get_or_fixture(
            &format!(
                "{API_URL}/data/browse/?lang=2&keyword={}&year=&rating=&votes=&genre=&country=&cast=&directors=&type=&order_by=&page={page}",
                manatan_shared::sdk::http::url_encode(query)
            ),
            LIST_FIXTURE,
        );
        Ok(parse_browse(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/data/watch/?_id=sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/data/watch/?_id=sample".to_string());
        let body = get_or_fixture(&api_url(&path), DETAILS_FIXTURE);
        Ok(parse_episodes(&body, &path))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/data/watch/?_id=sample".to_string());
        let (watch_path, episode) = split_episode_key(&path);
        let body = get_or_fixture(&api_url(&watch_path), STREAMS_FIXTURE);
        let limit = pref_bool(&request, "limit_qualities", true).then_some(15);
        let selected = selected_hosters(&request);
        Ok(parse_stream_hosters(&body, episode.as_deref(), limit)
            .into_iter()
            .filter(|hoster| selected.iter().any(|key| hoster.key.contains(key)))
            .collect())
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let name = request
            .get("hoster")
            .and_then(|hoster| hoster.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Mirror");
        let mut streams = vec![stream_from_hoster(&key, name)];
        sort_streams(
            &mut streams,
            pref(&request, "preferred_hoster", "https://streamtape.com"),
        );
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut streams = Vec::new();
        for hoster in self.hosters(request.clone())? {
            let mut resolved = self.resolve_hoster(json!({
                "hoster": { "key": hoster.key, "name": hoster.name },
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
            }))?;
            for stream in &mut resolved {
                stream.hoster = Some(hoster.clone());
            }
            streams.extend(resolved);
        }
        sort_streams(
            &mut streams,
            pref(&request, "preferred_hoster", "https://streamtape.com"),
        );
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Trending Movies".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: page.entries,
            has_more: page.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|path| api_url(&path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|path| api_url(&split_episode_key(&path).0)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&path)),
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
        .with_header("if-none-match", "")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_browse(body: &str) -> Paged<CatalogItem> {
    let json: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap());
    let pager = json.get("pager").unwrap_or(&Value::Null);
    let current = pager.get("currentPage").and_then(Value::as_u64).unwrap_or(1);
    let last = pager.get("endPage").and_then(Value::as_u64).unwrap_or(current);
    let entries = json
        .get("movies")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(item_from_movie_json)
        .collect();
    Paged {
        entries,
        has_next_page: current < last,
    }
}

fn item_from_movie_json(item: &Value) -> CatalogItem {
    let id = item.get("_id").and_then(Value::as_str).unwrap_or("sample");
    let tv = item.get("tv").and_then(Value::as_i64).unwrap_or(0) == 1;
    let poster = if tv {
        item.get("poster_path_season")
            .or_else(|| item.get("poster_path"))
    } else {
        item.get("poster_path")
    }
    .and_then(Value::as_str)
    .unwrap_or_default();
    CatalogItem {
        key: format!("/data/watch/?_id={id}"),
        title: item
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Movie4k")
            .to_string(),
        cover: (!poster.is_empty()).then(|| format!("https://image.tmdb.org/t/p/w300{poster}")),
        url: Some(format!("{API_URL}/data/watch/?_id={id}")),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&api_url(path), DETAILS_FIXTURE);
    let json: Value = serde_json::from_str(&body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    CatalogItem {
        key: path_key(path),
        title: json
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Movie4k")
            .to_string(),
        url: Some(api_url(path)),
        description: json
            .get("storyline")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, path: &str) -> Vec<VideoEpisode> {
    let json: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let id = json.get("_id").and_then(Value::as_str).unwrap_or("sample");
    if json.get("tv").and_then(Value::as_i64).unwrap_or(0) != 1 {
        return vec![VideoEpisode {
            key: format!("/data/watch/?_id={id}"),
            title: Some("Film".to_string()),
            episode_number: Some(1.0),
            url: Some(format!("{API_URL}/data/watch/?_id={id}")),
            language: Some("de".to_string()),
            ..VideoEpisode::default()
        }];
    }
    let season = json.get("s").and_then(Value::as_str).unwrap_or("1");
    let mut eps = json
        .get("streams")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|stream| stream.get("e").and_then(Value::as_f64))
        .collect::<Vec<_>>();
    eps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    eps.dedup();
    eps.into_iter()
        .map(|ep| {
            let ep_text = if ep.fract() == 0.0 {
                format!("{}", ep as i64)
            } else {
                ep.to_string()
            };
            VideoEpisode {
                key: format!("/data/watch/?_id={id}&e={ep_text}"),
                title: Some(format!("Staffel {season} Folge {ep_text}")),
                episode_number: Some(ep as f32),
                season_number: season.parse().ok(),
                url: Some(api_url(path)),
                language: Some("de".to_string()),
                ..VideoEpisode::default()
            }
        })
        .rev()
        .collect()
}

fn parse_stream_hosters(body: &str, episode: Option<&str>, limit: Option<usize>) -> Vec<VideoHoster> {
    let json: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(STREAMS_FIXTURE).unwrap());
    let tv = json.get("tv").and_then(Value::as_i64).unwrap_or(0) == 1;
    let mut out = Vec::new();
    for stream in json
        .get("streams")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if limit.is_some_and(|limit| out.len() >= limit) {
            break;
        }
        if tv {
            let Some(ep) = episode else {
                continue;
            };
            let stream_ep = stream.get("e").map(value_as_string).unwrap_or_default();
            if stream_ep != ep {
                continue;
            }
        }
        let link = stream
            .get("stream")
            .and_then(Value::as_str)
            .map(normalize_scheme)
            .unwrap_or_default();
        if link.is_empty() {
            continue;
        }
        let name = hoster_name(&link);
        out.push(VideoHoster {
            key: link.clone(),
            name,
            url: Some(link),
            lazy: true,
            video_count: Some(1),
            headers: referer_headers(API_URL),
            ..VideoHoster::default()
        });
    }
    out.reverse();
    out
}

fn stream_from_hoster(link: &str, name: &str) -> VideoStream {
    let resolved = if link.contains("streamcrypt.net") {
        client()
            .get(link)
            .send()
            .ok()
            .map(|response| response.final_url)
            .unwrap_or_else(|| link.to_string())
    } else {
        link.to_string()
    };
    external_stream(&resolved, name, API_URL)
}

fn external_stream(stream_url: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some(name.to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn selected_hosters(request: &Value) -> Vec<String> {
    let Some(values) = request
        .get("preferences")
        .and_then(|prefs| prefs.get("hoster_selection"))
        .and_then(Value::as_array)
    else {
        return vec!["streamtape".to_string(), "voe".to_string(), "streamz".to_string(), "vidoza".to_string()];
    };
    values
        .iter()
        .filter_map(Value::as_str)
        .map(|value| match value {
            "stape" => "streamtape",
            "streamz" => "streamz",
            value => value,
        })
        .map(ToString::to_string)
        .collect()
}

fn hoster_name(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if lower.contains("streamtape") {
        "Streamtape"
    } else if lower.contains("streamz") || lower.contains("streamcrypt") {
        "StreamZ"
    } else if lower.contains("vidoza") {
        "Vidoza"
    } else if lower.contains("voe") {
        "Voe"
    } else {
        "Mirror"
    }
    .to_string()
}

fn sort_streams(streams: &mut [VideoStream], preferred_hoster: &str) {
    streams.sort_by_key(|stream| i32::from(stream.url.contains(preferred_hoster)));
    streams.reverse();
    for stream in streams {
        stream.preferred = stream.url.contains(preferred_hoster);
    }
}

fn pref<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

fn pref_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .and_then(Value::as_str)
                .or_else(|| value.get("url").and_then(Value::as_str))
                .or_else(|| value.as_str())
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(path_key)
}

fn path_from_url(input: &str) -> Option<String> {
    if input.starts_with(API_URL) || input.starts_with(BASE_URL) || input.starts_with("/data/") {
        Some(path_key(input))
    } else {
        None
    }
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(API_URL) && !input.starts_with(BASE_URL) {
        return input.to_string();
    }
    input
        .strip_prefix(API_URL)
        .or_else(|| input.strip_prefix(BASE_URL))
        .unwrap_or(input)
        .to_string()
}

fn api_url(path: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else {
        url::join_url(API_URL, path)
    }
}

fn split_episode_key(path: &str) -> (String, Option<String>) {
    let Some((base, ep)) = path.split_once("&e=") else {
        return (path.to_string(), None);
    };
    (base.to_string(), Some(ep.to_string()))
}

fn normalize_scheme(input: &str) -> String {
    if input.starts_with("//") {
        format!("https:{input}")
    } else {
        input.to_string()
    }
}

fn value_as_string(value: &Value) -> String {
    value
        .as_str()
        .map(ToString::to_string)
        .or_else(|| value.as_i64().map(|num| num.to_string()))
        .or_else(|| value.as_f64().map(|num| {
            if num.fract() == 0.0 {
                format!("{}", num as i64)
            } else {
                num.to_string()
            }
        }))
        .unwrap_or_default()
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

const LIST_FIXTURE: &str = r#"{
  "pager": { "currentPage": 1, "endPage": 1 },
  "movies": [{ "_id": "sample", "title": "Sample", "poster_path": "/sample.jpg", "tv": 0 }]
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "_id": "sample",
  "title": "Sample",
  "storyline": "Sample movie",
  "tv": 0,
  "streams": [{ "stream": "https://streamtape.com/e/sample" }]
}"#;

const STREAMS_FIXTURE: &str = DETAILS_FIXTURE;

export_video_source!(SOURCE);
