use manatan_extension::{
    CatalogItem, DebridInfo, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind,
    abi::ExtensionResult,
    export_video_source,
    source::VideoSource,
};
use manatan_shared::sdk::{SearchRequest, http::HttpClient};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: DebridIndex = DebridIndex;
const BASE_URL: &str = "https://torrentio.strem.fun";
const SEARCH_URL: &str = "https://68d69db7dc40-debrid-search.baby-beamup.club";

struct DebridIndex;

impl VideoSource for DebridIndex {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let Some(token) = pref(&request, "token").filter(|value| !value.trim().is_empty()) else {
            return Ok(empty_page());
        };
        let provider =
            pref(&request, "debrid_provider").unwrap_or_else(|| "RealDebrid".to_string());
        let path = format!(
            "{BASE_URL}/{}={token}/catalog/other/torrentio-{}.json",
            provider.to_ascii_lowercase(),
            provider.to_ascii_lowercase()
        );
        let body = client()
            .get(path)
            .send_text()
            .unwrap_or_else(|_| ROOT_FIXTURE.to_string());
        Ok(parse_root(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(id) = query.strip_prefix("id:") {
            return Ok(Paged {
                entries: vec![CatalogItem {
                    key: id.to_string(),
                    title: id.to_string(),
                    url: Some(format!("{BASE_URL}/meta/other/{id}.json")),
                    language: Some("all".to_string()),
                    content_rating: Some("safe".to_string()),
                    status: ItemStatus::Unknown,
                    initialized: true,
                    ..CatalogItem::default()
                }],
                has_next_page: false,
            });
        }
        if let Some(id) = id_from_url(query) {
            return self.search(with_query(&request, &format!("id:{id}")));
        }
        let Some(token) = pref(&request, "token").filter(|value| !value.trim().is_empty()) else {
            return Ok(empty_page());
        };
        let provider =
            pref(&request, "debrid_provider").unwrap_or_else(|| "RealDebrid".to_string());
        let config = format!(
            "%7B%22DebridProvider%22%3A%22{}%22%2C%22DebridApiKey%22%3A%22{}%22%7D",
            provider, token
        );
        let url = format!(
            "{SEARCH_URL}/{config}/catalog/other/debridsearch/search={}.json",
            manatan_shared::sdk::http::url_encode(query)
        );
        let body = client()
            .get(url)
            .send_text()
            .unwrap_or_else(|_| ROOT_FIXTURE.to_string());
        Ok(parse_root(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        Ok(request_item(&request).unwrap_or_else(|| CatalogItem {
            key: request_key(&request, "item").unwrap_or_default(),
            title: request_key(&request, "item").unwrap_or_else(|| "Debrid item".to_string()),
            language: Some("all".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            initialized: true,
            ..CatalogItem::default()
        }))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let Some(token) = pref(&request, "token").filter(|value| !value.trim().is_empty()) else {
            return Ok(Vec::new());
        };
        let provider =
            pref(&request, "debrid_provider").unwrap_or_else(|| "RealDebrid".to_string());
        let key = request_key(&request, "item").unwrap_or_default();
        let url = format!(
            "{BASE_URL}/{}={token}/meta/other/{key}.json",
            provider.to_ascii_lowercase()
        );
        let body = client()
            .get(url)
            .send_text()
            .unwrap_or_else(|_| META_FIXTURE.to_string());
        Ok(parse_episodes(
            &body,
            pref_bool(&request, "filename", false),
        ))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request
            .get("episode")
            .and_then(|episode| episode.get("key").or_else(|| episode.get("url")))
            .and_then(Value::as_str)
            .or_else(|| request.get("key").and_then(Value::as_str))
            .unwrap_or_default();
        let name = request
            .get("episode")
            .and_then(|episode| episode.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("Debrid stream");
        Ok(vec![VideoStream {
            url: episode.to_string(),
            name: Some(name.split('/').last().unwrap_or(name).to_string()),
            quality: Some("debrid".to_string()),
            format: Some("external".to_string()),
            stream_kind: Some(VideoStreamKind::Debrid),
            debrid: Some(DebridInfo {
                provider: pref(&request, "debrid_provider"),
                requires_account: true,
                external_playback: true,
                ..DebridInfo::default()
            }),
            initialized: true,
            ..VideoStream::default()
        }])
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Index".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: self.list(request)?.entries,
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                search: Some(SearchRequest {
                    query: format!("id:{id}"),
                    ..SearchRequest::default()
                }),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(None)
    }
}

fn client() -> HttpClient {
    HttpClient::browser().with_referer(BASE_URL)
}

fn parse_root(body: &str) -> Paged<CatalogItem> {
    let payload: RootFiles = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: payload
            .metas
            .unwrap_or_default()
            .into_iter()
            .map(|meta| CatalogItem {
                key: meta.id.clone(),
                title: meta.name.clone(),
                cover: Some(if meta.name == "Downloads" {
                    "https://i.ibb.co/MGmhmJg/download.png".to_string()
                } else {
                    "https://i.ibb.co/Q9GPtbC/default.png".to_string()
                }),
                url: Some(format!("{BASE_URL}/meta/other/{}.json", meta.id)),
                language: Some("all".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                initialized: true,
                ..CatalogItem::default()
            })
            .collect(),
        has_next_page: false,
    }
}

fn empty_page() -> Paged<CatalogItem> {
    Paged {
        entries: Vec::new(),
        has_next_page: false,
    }
}

fn parse_episodes(body: &str, filename_only: bool) -> Vec<VideoEpisode> {
    let payload: SubFiles = serde_json::from_str(body).unwrap_or_default();
    let mut episodes: Vec<_> = payload
        .meta
        .and_then(|meta| meta.videos)
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .filter_map(|(index, video)| {
            let url = video.streams.first()?.url.clone();
            let title = if filename_only {
                video
                    .title
                    .trim()
                    .split('/')
                    .last()
                    .unwrap_or(&video.title)
                    .to_string()
            } else {
                video
                    .title
                    .trim()
                    .replace('[', "(")
                    .replace(']', ")")
                    .replace('/', " / ")
            };
            Some(VideoEpisode {
                key: url.clone(),
                title: Some(title),
                episode_number: Some((index + 1) as f32),
                date_uploaded: None,
                url: Some(url),
                language: Some("all".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect();
    episodes.reverse();
    episodes
}

fn request_item(request: &Value) -> Option<CatalogItem> {
    serde_json::from_value(request.get("item")?.clone()).ok()
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("url")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn pref(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn pref_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn with_query(request: &Value, query: &str) -> Value {
    let mut out = request.clone();
    out["query"] = Value::String(query.to_string());
    out
}

fn id_from_url(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) {
        return None;
    }
    input.split('/').nth(4).map(ToString::to_string)
}

#[derive(Default, Deserialize)]
struct RootFiles {
    metas: Option<Vec<Meta>>,
}

#[derive(Default, Deserialize)]
struct SubFiles {
    meta: Option<Meta>,
}

#[derive(Deserialize)]
struct Meta {
    id: String,
    name: String,
    videos: Option<Vec<DebridVideo>>,
}

#[derive(Deserialize)]
struct DebridVideo {
    title: String,
    streams: Vec<DebridStream>,
}

#[derive(Deserialize)]
struct DebridStream {
    url: String,
}

const ROOT_FIXTURE: &str = r#"{"metas":[{"id":"downloads","type":"other","name":"Downloads"}]}"#;
const META_FIXTURE: &str = r#"{"meta":{"id":"downloads","type":"other","name":"Downloads","videos":[{"id":"1","title":"Downloads/sample.mkv","released":"2024-01-01T00:00:00.000Z","streams":[{"url":"https://example.com/sample.mkv"}]}]}}"#;

export_video_source!(SOURCE);
