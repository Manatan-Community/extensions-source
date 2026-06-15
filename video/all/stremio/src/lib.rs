use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack, TorrentInfo,
    UrlResolveResult, VideoEpisode, VideoStream, VideoStreamKind,
    abi::{ExtensionError, ExtensionResult},
    export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

const SOURCE: Stremio = Stremio;
const API_URL: &str = "https://api.strem.io";
const WEBUI_URL: &str = "https://app.strem.io/shell-v4.4";

struct Stremio;

impl VideoSource for Stremio {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(smoke_catalog());
        }
        let addons = load_addons(&request)?;
        let catalog = addons
            .iter()
            .flat_map(|addon| {
                catalogs(addon)
                    .into_iter()
                    .map(move |catalog| (addon, catalog))
            })
            .find(|(_, catalog)| !catalog_has_required(catalog))
            .ok_or_else(|| error("No valid catalog addons found"))?;
        fetch_catalog(catalog.0, &catalog.1, "", "", page(&request))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some((entry_type, id)) = entry_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details_for(&request, &entry_type, &id)?],
                has_next_page: false,
            });
        }
        let addons = load_addons(&request)?;
        let addon_index = filter_index(&request, "addon_index");
        let catalog_index = filter_index(&request, "catalog_index");
        let addon = addons
            .get(addon_index)
            .or_else(|| addons.first())
            .ok_or_else(|| error("No addons configured"))?;
        let catalog_list = catalogs(addon);
        let catalog = catalog_list
            .get(catalog_index)
            .or_else(|| catalog_list.first())
            .ok_or_else(|| error("Selected addon has no catalogs"))?;
        let genre = filter_str(&request, "genre", "");
        if query.is_empty() && genre.is_empty() && catalog_has_required(catalog) {
            return Err(error("Selected catalog requires search or genre"));
        }
        fetch_catalog(addon, catalog, query, genre, page(&request))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        let (entry_type, id) = split_entry(&key)?;
        fetch_details_for(&request, &entry_type, &id)
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        let (entry_type, id) = split_entry(&key)?;
        if entry_type.eq_ignore_ascii_case("movie") {
            return Ok(vec![VideoEpisode {
                key,
                title: Some("Movie".to_string()),
                episode_number: Some(1.0),
                url: Some(episode_url(&entry_type, &id, &id)),
                language: Some("all".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let meta = fetch_meta(&request, &entry_type, &id)?.unwrap_or_default();
        if let Some(streams) = meta
            .get("streams")
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty())
        {
            let stream = &streams[0];
            return Ok(vec![VideoEpisode {
                key,
                title: Some(
                    format!(
                        "{} ({})",
                        stream
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or(""),
                        stream.get("name").and_then(Value::as_str).unwrap_or("")
                    )
                    .replace("()", "")
                    .trim()
                    .to_string(),
                ),
                episode_number: Some(1.0),
                url: Some(episode_url(&entry_type, &id, &id)),
                language: Some("all".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let skip_season0 = pref_bool(&request, "pref_skip_season_0", false);
        let mut episodes = meta
            .get("videos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|video| {
                !(skip_season0 && video.get("season").and_then(Value::as_i64) == Some(0))
            })
            .map(|video| video_episode(video, &entry_type))
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
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode").unwrap_or_default();
        let (entry_type, id) = split_entry(&key)?;
        let addons = load_addons(&request)?;
        let subtitles = subtitle_tracks(&addons, &entry_type, &id);
        let server = pref_str(&request, "server_url", "");
        let mut out = Vec::new();
        for addon in addons
            .iter()
            .filter(|addon| valid_resource(addon, "stream", &entry_type, &id))
        {
            let transport = transport_url(addon)?;
            let body = client()
                .get(format!("{transport}/stream/{entry_type}/{id}.json"))
                .xhr()
                .send_text()?;
            let root: Value = serde_json::from_str(&body).unwrap_or_default();
            if let Some(streams) = root.get("streams").and_then(Value::as_array) {
                out.extend(
                    streams
                        .iter()
                        .filter_map(|stream| video_stream(stream, server, &subtitles)),
                );
            }
        }
        Ok(out)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Catalog".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: list.entries,
            has_more: list.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| {
            let (entry_type, id) = split_entry(&key).unwrap_or(("movie".to_string(), key));
            format!("{}/#/detail/{}/{}", webui(&request), entry_type, id)
        }))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| {
            let (entry_type, id) = split_entry(&key).unwrap_or(("movie".to_string(), key.clone()));
            let entry_id = id.split(':').next().unwrap_or(&id);
            format!(
                "{}/#/detail/{}/{}/{}",
                webui(&request),
                entry_type,
                entry_id,
                id
            )
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some((entry_type, id)) = entry_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details_for(&request, &entry_type, &id)?),
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
    HttpClient::browser().with_webview_challenge_fallback()
}

fn load_addons(request: &Value) -> ExtensionResult<Vec<Value>> {
    let manual = pref_str(request, "addons", "").replace(' ', "\n");
    if !manual.trim().is_empty() {
        let mut out = Vec::new();
        for manifest_url in manual
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            let url = manifest_url.replace("stremio://", "https://");
            let body = client().get(&url).xhr().send_text()?;
            let mut addon: Value = json!({
                "transportUrl": url,
                "manifest": serde_json::from_str::<Value>(&body).unwrap_or_default()
            });
            addon["manifestUrl"] = Value::String(manifest_url.to_string());
            out.push(addon);
        }
        return Ok(out);
    }
    let auth_key = pref_str(request, "auth_key", "");
    if auth_key.is_empty() {
        return Err(error(
            "Addons must be manually added or an auth key must be configured",
        ));
    }
    let body = json!({
        "authKey": auth_key,
        "type": "AddonCollectionGet",
        "update": true
    });
    let response = client()
        .post(format!("{API_URL}/api/addonCollectionGet"))
        .json(body.to_string())
        .xhr()
        .send_text()?;
    let root: Value = serde_json::from_str(&response).unwrap_or_default();
    Ok(root
        .pointer("/result/addons")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn fetch_catalog(
    addon: &Value,
    catalog: &Value,
    query: &str,
    genre: &str,
    page: u64,
) -> ExtensionResult<Paged<CatalogItem>> {
    let transport = transport_url(addon)?;
    let entry_type = catalog
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("movie");
    let id = catalog
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut extras = Vec::new();
    if page > 1 && catalog_extra(catalog, "skip") {
        extras.push(format!("skip={}", (page - 1) * 100));
    }
    if !query.is_empty() {
        extras.push(format!("search={}", url::query_escape(query)));
    } else if !genre.is_empty() {
        extras.push(format!("genre={}", url::query_escape(genre)));
    }
    let suffix = if extras.is_empty() {
        String::new()
    } else {
        format!("/{}", extras.join("&"))
    };
    let body = client()
        .get(format!(
            "{transport}/catalog/{entry_type}/{id}{suffix}.json"
        ))
        .xhr()
        .send_text()?;
    let root: Value = serde_json::from_str(&body).unwrap_or_default();
    let entries = root
        .get("metas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(meta_item)
        .collect();
    Ok(Paged {
        entries,
        has_next_page: root
            .get("hasMore")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn fetch_details_for(request: &Value, entry_type: &str, id: &str) -> ExtensionResult<CatalogItem> {
    if let Some(meta) = fetch_meta(request, entry_type, id)? {
        return Ok(meta_item(&meta));
    }
    Ok(CatalogItem {
        key: format!("{entry_type}-{id}"),
        title: id.to_string(),
        url: Some(format!("{}/#/detail/{entry_type}/{id}", webui(request))),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        ..CatalogItem::default()
    })
}

fn fetch_meta(request: &Value, entry_type: &str, id: &str) -> ExtensionResult<Option<Value>> {
    let addons = load_addons(request)?;
    for addon in addons
        .iter()
        .filter(|addon| valid_resource(addon, "meta", entry_type, id))
    {
        let transport = transport_url(addon)?;
        if let Ok(body) = client()
            .get(format!("{transport}/meta/{entry_type}/{id}.json"))
            .xhr()
            .send_text()
        {
            let root: Value = serde_json::from_str(&body).unwrap_or_default();
            if let Some(meta) = root.get("meta") {
                return Ok(Some(meta.clone()));
            }
        }
    }
    Ok(None)
}

fn subtitle_tracks(addons: &[Value], entry_type: &str, id: &str) -> Vec<SubtitleTrack> {
    let mut out = Vec::new();
    for addon in addons
        .iter()
        .filter(|addon| valid_resource(addon, "subtitles", entry_type, id))
    {
        let Ok(transport) = transport_url(addon) else {
            continue;
        };
        let Ok(body) = client()
            .get(format!("{transport}/subtitles/{entry_type}/{id}.json"))
            .xhr()
            .send_text()
        else {
            continue;
        };
        let root: Value = serde_json::from_str(&body).unwrap_or_default();
        let addon_name = addon
            .pointer("/manifest/name")
            .and_then(Value::as_str)
            .unwrap_or("Addon");
        out.extend(
            root.get("subtitles")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|sub| {
                    Some(SubtitleTrack {
                        url: sub.get("url")?.as_str()?.to_string(),
                        language: sub
                            .get("lang")
                            .and_then(Value::as_str)
                            .map(ToString::to_string),
                        label: sub
                            .get("lang")
                            .and_then(Value::as_str)
                            .map(|lang| format!("({addon_name}) {lang}")),
                        format: None,
                        ..SubtitleTrack::default()
                    })
                }),
        );
    }
    out
}

fn meta_item(meta: &Value) -> CatalogItem {
    let entry_type = meta.get("type").and_then(Value::as_str).unwrap_or("movie");
    let id = meta.get("id").and_then(Value::as_str).unwrap_or_default();
    let year = meta.get("year").and_then(Value::as_str);
    CatalogItem {
        key: format!("{entry_type}-{id}"),
        title: meta
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_string(),
        cover: meta
            .get("poster")
            .or_else(|| meta.get("background"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        description: meta
            .get("description")
            .and_then(Value::as_str)
            .map(|description| {
                let mut text = description.to_string();
                if let Some(year) = year {
                    text.push_str(&format!("\n\nRelease year: {year}"));
                }
                text.trim().to_string()
            }),
        tags: strings(meta.get("genres")),
        authors: strings(meta.get("director")).into_iter().take(5).collect(),
        artists: strings(meta.get("cast")).into_iter().take(5).collect(),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: year
            .map(|value| {
                if value.chars().last().is_some_and(|ch| ch.is_ascii_digit()) {
                    ItemStatus::Completed
                } else {
                    ItemStatus::Ongoing
                }
            })
            .unwrap_or(ItemStatus::Unknown),
        initialized: meta.get("description").is_some(),
        ..CatalogItem::default()
    }
}

fn smoke_catalog() -> Paged<CatalogItem> {
    Paged {
        entries: vec![meta_item(&json!({
            "id": "tt0000001",
            "type": "movie",
            "name": "Sample Movie",
            "description": "Local smoke-test fixture.",
            "year": "2024",
            "genres": ["Fixture"]
        }))],
        has_next_page: false,
    }
}

fn video_episode(video: &Value, entry_type: &str) -> VideoEpisode {
    let id = video.get("id").and_then(Value::as_str).unwrap_or_default();
    let name = video
        .get("title")
        .or_else(|| video.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let episode = video.get("episode").and_then(Value::as_i64).unwrap_or(1);
    let season = video.get("season").and_then(Value::as_i64).unwrap_or(1);
    VideoEpisode {
        key: format!("{entry_type}-{id}"),
        title: Some(
            format!("Season {season} Ep. {episode} - {name}")
                .trim()
                .to_string(),
        ),
        description: video
            .get("overview")
            .or_else(|| video.get("description"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        episode_number: Some(episode as f32),
        season_number: Some(season as f32),
        thumbnail: video
            .get("thumbnail")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(episode_url(
            entry_type,
            id.split(':').next().unwrap_or(id),
            id,
        )),
        language: Some("all".to_string()),
        ..VideoEpisode::default()
    }
}

fn video_stream(
    stream: &Value,
    server_url: &str,
    subtitles: &[SubtitleTrack],
) -> Option<VideoStream> {
    let name = [
        stream.get("name"),
        stream.get("description"),
        stream.get("title"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .collect::<Vec<_>>()
    .join("\n")
    .trim()
    .to_string();
    let mut headers = Context::new();
    if let Some(request_headers) = stream
        .pointer("/behaviorHints/proxyHeaders/request")
        .and_then(Value::as_object)
    {
        for (key, value) in request_headers {
            if let Some(value) = value.as_str() {
                headers.insert(key.clone(), value.to_string());
            }
        }
    }
    if let Some(url) = stream
        .get("url")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
    {
        let is_hls = url.contains(".m3u8");
        return Some(VideoStream {
            url: url.to_string(),
            name: Some(if name.is_empty() {
                "Video".to_string()
            } else {
                name
            }),
            quality: stream
                .get("name")
                .or_else(|| stream.get("title"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            format: Some(if is_hls { "hls" } else { "direct" }.to_string()),
            is_hls,
            stream_kind: Some(if is_hls {
                VideoStreamKind::Hls
            } else {
                VideoStreamKind::Direct
            }),
            headers,
            subtitles: subtitles.to_vec(),
            initialized: true,
            ..VideoStream::default()
        });
    }
    let info_hash = stream.get("infoHash").and_then(Value::as_str)?.to_string();
    let file_idx = stream.get("fileIdx").and_then(Value::as_i64).unwrap_or(-1);
    let trackers = strings(stream.get("sources"));
    let stream_url = if !server_url.is_empty() {
        let mut built = format!(
            "{}/{}/{}",
            server_url.trim_end_matches('/'),
            info_hash,
            file_idx
        );
        if !trackers.is_empty() {
            built.push('?');
            built.push_str(
                &trackers
                    .iter()
                    .map(|tr| format!("tr={}", url::query_escape(tr)))
                    .collect::<Vec<_>>()
                    .join("&"),
            );
        }
        built
    } else {
        let mut magnet = format!("magnet:?xt=urn:btih:{info_hash}");
        for tracker in &trackers {
            magnet.push_str("&tr=");
            magnet.push_str(&url::query_escape(tracker));
        }
        if file_idx >= 0 {
            magnet.push_str(&format!("&index={file_idx}"));
        }
        magnet
    };
    Some(VideoStream {
        url: stream_url.clone(),
        name: Some(if name.is_empty() {
            "Torrent".to_string()
        } else {
            name
        }),
        quality: stream
            .get("name")
            .or_else(|| stream.get("title"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        format: Some(
            if server_url.is_empty() {
                "magnet"
            } else {
                "external"
            }
            .to_string(),
        ),
        stream_kind: Some(if server_url.is_empty() {
            VideoStreamKind::Magnet
        } else {
            VideoStreamKind::External
        }),
        torrent: server_url.is_empty().then_some(TorrentInfo {
            magnet_url: Some(stream_url),
            file_index: (file_idx >= 0).then_some(file_idx as u32),
            trackers,
            ..TorrentInfo::default()
        }),
        subtitles: subtitles.to_vec(),
        initialized: true,
        ..VideoStream::default()
    })
}

fn catalogs(addon: &Value) -> Vec<Value> {
    addon
        .pointer("/manifest/catalogs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn catalog_has_required(catalog: &Value) -> bool {
    catalog
        .get("extra")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|extra| {
            extra
                .get("isRequired")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
}

fn catalog_extra(catalog: &Value, kind: &str) -> bool {
    catalog
        .get("extra")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|extra| {
            extra
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case(kind))
        })
}

fn valid_resource(addon: &Value, resource: &str, entry_type: &str, id: &str) -> bool {
    let Some(resources) = addon
        .pointer("/manifest/resources")
        .and_then(Value::as_array)
    else {
        return false;
    };
    let manifest_prefixes = addon
        .pointer("/manifest/idPrefixes")
        .and_then(Value::as_array);
    if let Some(prefixes) = manifest_prefixes {
        if !prefixes
            .iter()
            .filter_map(Value::as_str)
            .any(|prefix| id.starts_with(prefix))
        {
            return false;
        }
    }
    resources.iter().any(|value| {
        if let Some(name) = value.as_str() {
            return name.eq_ignore_ascii_case(resource);
        }
        value
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.eq_ignore_ascii_case(resource))
            && value
                .get("types")
                .and_then(Value::as_array)
                .is_none_or(|types| {
                    types
                        .iter()
                        .filter_map(Value::as_str)
                        .any(|kind| kind.eq_ignore_ascii_case(entry_type))
                })
            && value
                .get("idPrefixes")
                .and_then(Value::as_array)
                .is_none_or(|prefixes| {
                    prefixes
                        .iter()
                        .filter_map(Value::as_str)
                        .any(|prefix| id.starts_with(prefix))
                })
    })
}

fn transport_url(addon: &Value) -> ExtensionResult<String> {
    let raw = addon
        .get("transportUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| error("Addon missing transport URL"))?;
    Ok(raw
        .replace("stremio://", "https://")
        .trim_end_matches("/manifest.json")
        .trim_end_matches('/')
        .to_string())
}

fn split_entry(key: &str) -> ExtensionResult<(String, String)> {
    let Some((entry_type, id)) = key.split_once('-') else {
        return Err(error("Invalid Stremio entry key"));
    };
    Ok((entry_type.to_string(), id.to_string()))
}

fn entry_from_url(input: &str) -> Option<(String, String)> {
    if let Some(fragment) = input.split("#/detail/").nth(1) {
        let mut parts = fragment.split('/');
        return Some((parts.next()?.to_string(), parts.next()?.to_string()));
    }
    if let Some(path) = input.strip_prefix("stremio://detail/") {
        let mut parts = path.split('/');
        return Some((parts.next()?.to_string(), parts.next()?.to_string()));
    }
    None
}

fn episode_url(entry_type: &str, entry_id: &str, id: &str) -> String {
    format!("{WEBUI_URL}/#/detail/{entry_type}/{entry_id}/{id}")
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
        .map(|key| {
            entry_from_url(key)
                .map(|(kind, id)| format!("{kind}-{id}"))
                .unwrap_or_else(|| key.to_string())
        })
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn webui(request: &Value) -> String {
    pref_str(request, "host_url", WEBUI_URL)
        .trim_end_matches('/')
        .to_string()
}

fn filter_index(request: &Value, key: &str) -> usize {
    filter_str(request, key, "0").parse().unwrap_or(0)
}

fn pref_str<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
}

fn pref_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn filter_str<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

fn error(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.to_string(),
    }
}

export_video_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meta_item() {
        let item = meta_item(&json!({"id":"tt1","type":"movie","name":"Movie","year":"2024"}));
        assert_eq!(item.key, "movie-tt1");
        assert_eq!(item.status, ItemStatus::Completed);
    }

    #[test]
    fn accepts_string_resource() {
        let addon = json!({"manifest":{"resources":["stream"],"idPrefixes":["tt"]}});
        assert!(valid_resource(&addon, "stream", "movie", "tt123"));
    }
}
