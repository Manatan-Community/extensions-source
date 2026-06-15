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

const SOURCE: Kool = Kool;
const BASE_URL: &str = "https://www.kool.to";

struct Kool;

impl VideoSource for Kool {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let body = post_catalog("tmdb.movie", "movie/popular", "", "popularity", page, false);
        Ok(parse_catalog(&body, false))
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
        let search_type = filter(&request, "search_type", "filme");
        let (catalog, id, cluster) = match search_type.as_str() {
            "serien" => ("tmdb.series", "tmdb.series", false),
            "tv" => ("kool-iptv", "kool-iptv", true),
            _ => ("tmdb.movie", "tmdb.movie", false),
        };
        let body = post_catalog(catalog, id, query, "", page, cluster);
        Ok(parse_catalog(&body, cluster))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/data/watch/?_id=sample&type=movie&name=Sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/data/watch/?_id=sample&type=movie&name=Sample".to_string());
        let item = parse_item_key(&path);
        if item.kind == "movie" {
            return Ok(vec![VideoEpisode {
                key: path.clone(),
                title: Some("Film".to_string()),
                episode_number: Some(1.0),
                url: Some(absolute_url(&path)),
                language: Some("de".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        if item.kind == "iptv" {
            return Ok(vec![VideoEpisode {
                key: path.clone(),
                title: Some("TV".to_string()),
                episode_number: Some(1.0),
                url: Some(absolute_url(&path)),
                language: Some("de".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let body = post_item(&item);
        Ok(parse_series_episodes(&body, &item))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/data/watch/?_id=sample&type=movie&name=Sample".to_string());
        let body = post_source(&parse_item_key(&path));
        let selected = selected_hosters(&request);
        Ok(parse_stream_hosters(&body)
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
        let mut streams = vec![external_stream(&key, name, BASE_URL)];
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
            title: "Popular Movies".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: page.entries,
            has_more: page.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|path| absolute_url(&path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|path| absolute_url(&path)))
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

#[derive(Clone)]
struct KoolItem {
    id: String,
    name: String,
    kind: String,
    url: Option<String>,
    season: Option<u64>,
    episode: Option<u64>,
    episode_id: Option<String>,
    episode_name: Option<String>,
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn mhub() -> String {
    let body = client()
        .post("https://www.dezor.net/api/app/ping")
        .json(r#"{"reason":"ping","locale":"de","theme":"dark","metadata":{"device":{"type":"Tablet","brand":"google","model":"Pixel 5","name":"Pixel 5","uniqueId":"17623a364c1eab4b"},"os":{"name":"android","version":"12"},"app":{"platform":"android","version":"1.1.2","buildId":"97245000","engine":"hbc85"}},"hasMhub":true,"package":"net.dezor.browser","version":"1.1.2"}"#)
        .send_text()
        .unwrap_or_default();
    serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|json| json.get("mhub").and_then(Value::as_str).map(ToString::to_string))
        .unwrap_or_default()
}

fn post_json(target: &str, payload: Value, cluster: bool) -> String {
    let http = client();
    let mut req = http
        .post(target)
        .header("content-type", "application/json; charset=utf-8")
        .header("mediahubmx-signature", mhub())
        .header("user-agent", "MediaHubMX/2")
        .json(payload.to_string());
    if cluster {
        req = req.referer(BASE_URL);
    }
    req.send_text().unwrap_or_else(|_| CATALOG_FIXTURE.to_string())
}

fn cursor(page: u64) -> Value {
    match page {
        0 | 1 => Value::Null,
        2 => json!(8),
        page => json!(page * 8 - (page - 1)),
    }
}

fn post_catalog(catalog: &str, id: &str, search: &str, sort: &str, page: u64, cluster: bool) -> String {
    let host = if cluster { "kool-cluster" } else { "kool" };
    post_json(
        &format!("{BASE_URL}/{host}/mediahubmx-catalog.json"),
        json!({
            "language": "de",
            "region": "DE",
            "catalogId": catalog,
            "id": id,
            "adult": false,
            "search": search,
            "sort": sort,
            "filter": {},
            "cursor": cursor(page),
            "clientVersion": "1.1.3"
        }),
        cluster,
    )
}

fn post_item(item: &KoolItem) -> String {
    post_json(
        &format!("{BASE_URL}/kool/mediahubmx-item.json"),
        json!({
            "language": "de",
            "region": "DE",
            "type": item.kind,
            "ids": { "tmdb_id": item.id },
            "name": item.name,
            "episode": {},
            "clientVersion": "1.1.3"
        }),
        false,
    )
}

fn post_source(item: &KoolItem) -> String {
    if item.kind == "iptv" {
        return post_json(
            &format!("{BASE_URL}/kool-cluster/mediahubmx-resolve.json"),
            json!({
                "language": "de",
                "region": "DE",
                "url": item.url.clone().unwrap_or_default(),
                "clientVersion": "1.1.3"
            }),
            true,
        );
    }
    let episode = if item.kind == "series" {
        json!({
            "name": item.episode_name.clone().unwrap_or_default(),
            "ids": { "tmdb_episode_id": item.episode_id.clone().unwrap_or_default() },
            "season": item.season.unwrap_or(1),
            "episode": item.episode.unwrap_or(1)
        })
    } else {
        json!({})
    };
    post_json(
        &format!("{BASE_URL}/kool-cluster/mediahubmx-source.json"),
        json!({
            "language": "de",
            "region": "DE",
            "type": item.kind,
            "ids": { "tmdb_id": item.id },
            "name": item.name,
            "episode": episode,
            "clientVersion": "1.1.3"
        }),
        true,
    )
}

fn parse_catalog(body: &str, cluster: bool) -> Paged<CatalogItem> {
    let json: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(CATALOG_FIXTURE).unwrap());
    let has_next_page = !json
        .get("nextCursor")
        .map(|value| value.is_null())
        .unwrap_or(true);
    let entries = json
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| item_from_json(item, cluster))
        .collect();
    Paged {
        entries,
        has_next_page,
    }
}

fn item_from_json(item: &Value, cluster: bool) -> CatalogItem {
    let name = item.get("name").and_then(Value::as_str).unwrap_or("Kool");
    let kind = item.get("type").and_then(Value::as_str).unwrap_or("movie");
    let ids = item.get("ids").unwrap_or(&Value::Null);
    let id = ids
        .get("urlId")
        .or_else(|| ids.get("tmdb_id"))
        .map(value_as_string)
        .unwrap_or_else(|| "sample".to_string());
    let key = if kind == "iptv" || cluster {
        let stream_url = item.get("url").and_then(Value::as_str).unwrap_or_default();
        format!("/data/watch/?url={}&type=iptv&name={}", encode_key(stream_url), encode_key(name))
    } else {
        format!("/data/watch/?_id={id}&type={kind}&name={}", encode_key(name))
    };
    CatalogItem {
        key: key.clone(),
        title: name.to_string(),
        cover: item
            .get("images")
            .and_then(|images| images.get("poster").or_else(|| images.get("backdrop")))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(absolute_url(&key)),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn fetch_details(path: &str) -> CatalogItem {
    let item = parse_item_key(path);
    let body = if item.kind == "iptv" {
        post_source(&item)
    } else {
        post_item(&item)
    };
    let json: Value = serde_json::from_str(&body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let source = json.as_array().and_then(|array| array.first()).unwrap_or(&json);
    CatalogItem {
        key: path_key(path),
        title: source
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&item.name)
            .to_string(),
        cover: source
            .get("images")
            .and_then(|images| images.get("poster").or_else(|| images.get("backdrop")))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(absolute_url(path)),
        description: source
            .get("description")
            .or_else(|| source.get("name"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_series_episodes(body: &str, item: &KoolItem) -> Vec<VideoEpisode> {
    let json: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    json.get("episodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|ep| {
            let ep_id = ep
                .get("ids")
                .and_then(|ids| ids.get("tmdb_episode_id"))
                .map(value_as_string)
                .unwrap_or_default();
            let season = ep.get("season").and_then(Value::as_u64).unwrap_or(1);
            let ep_num = ep.get("episode").and_then(Value::as_u64).unwrap_or(1);
            let name = ep.get("name").and_then(Value::as_str).unwrap_or_default();
            let key = format!(
                "/data/watch/?_id={}&type=series&name={}&epid={ep_id}&season={season}&ep={ep_num}&epname={}",
                item.id,
                encode_key(&item.name),
                encode_key(name)
            );
            VideoEpisode {
                key: key.clone(),
                title: Some(format!("Staffel {season} Folge {ep_num} : {name}")),
                episode_number: Some(ep_num as f32),
                season_number: Some(season as f32),
                url: Some(absolute_url(&key)),
                language: Some("de".to_string()),
                ..VideoEpisode::default()
            }
        })
        .rev()
        .collect()
}

fn parse_stream_hosters(body: &str) -> Vec<VideoHoster> {
    let json: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(STREAMS_FIXTURE).unwrap());
    let items = json.as_array().into_iter().flatten();
    items
        .filter_map(|item| {
            let src = item.get("url").and_then(Value::as_str)?;
            let name = hoster_name(src);
            Some(VideoHoster {
                key: src.to_string(),
                name,
                url: Some(src.to_string()),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(BASE_URL),
                ..VideoHoster::default()
            })
        })
        .collect()
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
        return vec!["streamtape".to_string(), "voe".to_string(), "vidoza".to_string(), "clipboard".to_string(), "filemoon".to_string()];
    };
    values
        .iter()
        .filter_map(Value::as_str)
        .map(|value| match value {
            "stape" => "streamtape",
            "clip" => "clipboard",
            "fmoon" => "filemoon",
            value => value,
        })
        .map(ToString::to_string)
        .collect()
}

fn hoster_name(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if lower.contains("streamtape") {
        "Streamtape"
    } else if lower.contains("vidoza") {
        "Vidoza"
    } else if lower.contains("clipboard") {
        "Clipboard"
    } else if lower.contains("filemoon") {
        "Filemoon"
    } else if lower.contains("voe") || lower.contains("scatch176duplicities") {
        "Voe"
    } else {
        "TV"
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

fn parse_item_key(path: &str) -> KoolItem {
    let query = path.split('?').nth(1).unwrap_or(path);
    KoolItem {
        id: query_param(query, "_id").unwrap_or_else(|| "sample".to_string()),
        name: query_param(query, "name").unwrap_or_else(|| "Kool".to_string()),
        kind: query_param(query, "type").unwrap_or_else(|| "movie".to_string()),
        url: query_param(query, "url"),
        season: query_param(query, "season").and_then(|value| value.parse().ok()),
        episode: query_param(query, "ep").and_then(|value| value.parse().ok()),
        episode_id: query_param(query, "epid"),
        episode_name: query_param(query, "epname"),
    }
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|part| {
        let (part_key, value) = part.split_once('=')?;
        (part_key == key).then(|| value.replace("%20", " ").replace("%2F", "/").replace("%3A", ":"))
    })
}

fn encode_key(input: &str) -> String {
    input
        .replace(':', "%3A")
        .replace('/', "%2F")
        .replace(' ', "%20")
        .replace('&', "%26")
}

fn value_as_string(value: &Value) -> String {
    value
        .as_str()
        .map(ToString::to_string)
        .or_else(|| value.as_u64().map(|num| num.to_string()))
        .or_else(|| value.as_i64().map(|num| num.to_string()))
        .unwrap_or_default()
}

fn filter(request: &Value, key: &str, default: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn pref<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
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
    if input.starts_with(BASE_URL) || input.starts_with("/data/") {
        Some(path_key(input))
    } else {
        None
    }
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(BASE_URL) {
        return input.to_string();
    }
    input.strip_prefix(BASE_URL).unwrap_or(input).to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

const CATALOG_FIXTURE: &str = r#"{
  "nextCursor": null,
  "items": [{ "name": "Sample", "type": "movie", "ids": { "tmdb_id": "sample" }, "images": { "poster": "https://www.kool.to/sample.jpg" } }]
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "name": "Sample",
  "description": "Sample",
  "images": { "poster": "https://www.kool.to/sample.jpg" },
  "episodes": [{ "name": "Pilot", "season": 1, "episode": 1, "ids": { "tmdb_episode_id": "sample-ep" } }]
}"#;

const STREAMS_FIXTURE: &str = r#"[{ "url": "https://streamtape.com/e/sample" }]"#;

export_video_source!(SOURCE);
