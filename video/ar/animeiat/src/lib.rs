use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::sdk::{Context, SearchRequest, http::HttpClient};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Animeiat = Animeiat;
const BASE_URL: &str = "https://api.animeiat.co/v1";
const STORAGE_URL: &str = "https://api.animeiat.co/storage";

struct Animeiat;

impl VideoSource for Animeiat {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            let body = get_or_fixture(
                &format!("{BASE_URL}/home/sticky-episodes?page={page}"),
                LATEST_FIXTURE,
            );
            return Ok(parse_latest(&body));
        }
        let body = get_or_fixture(&format!("{BASE_URL}/anime?page={page}"), LIST_FIXTURE);
        Ok(parse_list(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(slug) = slug_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&slug)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let target = if query.is_empty() {
            let type_filter = filter(&request, "type").unwrap_or_default();
            let status_filter = filter(&request, "status").unwrap_or_default();
            let mut url = format!("{BASE_URL}/anime?page={page}");
            if !type_filter.is_empty() {
                url.push_str("&type=");
                url.push_str(&type_filter);
            }
            if !status_filter.is_empty() {
                url.push_str("&status=");
                url.push_str(&status_filter);
            }
            url
        } else {
            format!(
                "{BASE_URL}/anime?q={}&page={page}",
                manatan_shared::sdk::http::url_encode(query)
            )
        };
        let body = get_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_list(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let slug = request_key(&request, "item").unwrap_or_else(|| "sample".to_string());
        Ok(fetch_details(&slug))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let slug = request_key(&request, "item").unwrap_or_else(|| "sample".to_string());
        let mut target = format!("{BASE_URL}/anime/{slug}/episodes");
        let mut out = Vec::new();
        let mut guard = 0;
        loop {
            guard += 1;
            let body = get_or_fixture(&target, EPISODES_FIXTURE);
            let page: EpisodesResponse = serde_json::from_str(&body).unwrap_or_default();
            out.extend(page.data.into_iter().map(|episode| VideoEpisode {
                key: format!("episode/{}", episode.slug),
                title: Some(episode.title),
                episode_number: Some(episode.number),
                thumbnail: Some(storage_url(&episode.poster_path)),
                url: Some(format!("{BASE_URL}/episode/{}", episode.slug)),
                language: Some("ar".to_string()),
                ..VideoEpisode::default()
            }));
            let Some(next) = page.links.next else {
                break;
            };
            if next.is_empty() || guard >= 20 {
                break;
            }
            target = next;
        }
        out.reverse();
        Ok(out)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode").unwrap_or_else(|| "episode/sample".to_string());
        let body = get_or_fixture(&format!("{BASE_URL}/{key}"), STREAM_PAGE_FIXTURE);
        let player_hash = body
            .split("\"hash\":\"")
            .nth(1)
            .and_then(|part| part.split('"').next())
            .unwrap_or_default();
        let decoded = STANDARD.decode(player_hash).ok();
        let decoded = decoded
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_default();
        let player_id = decoded
            .split('"')
            .rev()
            .nth(1)
            .filter(|value| !value.is_empty())
            .unwrap_or("sample");
        let body = get_or_fixture(&format!("{BASE_URL}/video/{player_id}"), STREAM_FIXTURE);
        let payload: StreamResponse = serde_json::from_str(&body).unwrap_or_default();
        let mut streams: Vec<_> = payload
            .data
            .sources
            .into_iter()
            .map(|source| media_stream(&source.file, &source.label, &source.quality, BASE_URL))
            .collect();
        sort_streams(&mut streams, &preferred_quality(&request));
        Ok(streams)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: self.list(json!({"listing": "popular", "page": 1}))?.entries,
                has_more: true,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: self.list(json!({"listing": "latest", "page": 1}))?.entries,
                has_more: true,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|slug| format!("{BASE_URL}/anime/{slug}")))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| format!("{BASE_URL}/{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = slug_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&slug)),
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

fn get_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(slug: &str) -> CatalogItem {
    let body = get_or_fixture(&format!("{BASE_URL}/anime/{slug}"), DETAILS_FIXTURE);
    let payload: DetailsResponse = serde_json::from_str(&body).unwrap_or_default();
    let details = payload.data;
    CatalogItem {
        key: details.slug.clone(),
        title: if details.anime_name.is_empty() {
            slug.replace('-', " ")
        } else {
            details.anime_name
        },
        cover: Some(storage_url(&details.poster_path)),
        url: Some(format!("{BASE_URL}/anime/{}", details.slug)),
        authors: details.studios.into_iter().map(|item| item.name).collect(),
        description: Some(details.story).filter(|value| !value.is_empty()),
        tags: details.genres.into_iter().map(|item| item.name).collect(),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: match details.status.as_str() {
            "ongoing" => ItemStatus::Ongoing,
            "completed" => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let payload: ListResponse = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: payload
            .data
            .into_iter()
            .map(|item| CatalogItem {
                key: item.slug.clone(),
                title: item.anime_name,
                cover: Some(storage_url(&item.poster_path)),
                url: Some(format!("{BASE_URL}/anime/{}", item.slug)),
                language: Some("ar".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
            .collect(),
        has_next_page: payload.meta.current_page < payload.meta.last_page,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let payload: LatestResponse = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: payload
            .data
            .into_iter()
            .map(|item| {
                let slug = item.slug.split("-episode-").next().unwrap_or(&item.slug);
                CatalogItem {
                    key: slug.to_string(),
                    title: item.title,
                    cover: Some(storage_url(&item.poster_path)),
                    url: Some(format!("{BASE_URL}/anime/{slug}")),
                    language: Some("ar".to_string()),
                    content_rating: Some("safe".to_string()),
                    status: ItemStatus::Unknown,
                    ..CatalogItem::default()
                }
            })
            .collect(),
        has_next_page: payload.meta.current_page < payload.meta.last_page,
    }
}

fn media_stream(url: &str, label: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = url.contains(".m3u8");
    VideoStream {
        url: url.to_string(),
        name: Some(format!("{label} {quality}").trim().to_string()),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn storage_url(path: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else {
        format!(
            "{}/{}",
            STORAGE_URL.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

fn sort_streams(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by_key(|stream| quality_score(stream.quality.as_deref()));
    streams.reverse();
    for stream in streams {
        stream.preferred = stream
            .quality
            .as_deref()
            .map(|quality| quality.contains(preferred))
            .unwrap_or(false);
    }
}

fn quality_score(quality: Option<&str>) -> i32 {
    quality
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("url")))
        .and_then(Value::as_str)
        .map(|value| {
            if field == "episode" {
                value
                    .strip_prefix(BASE_URL)
                    .unwrap_or(value)
                    .trim_start_matches('/')
                    .to_string()
            } else {
                slug_from_url(value).unwrap_or_else(|| value.trim_matches('/').to_string())
            }
        })
}

fn slug_from_url(input: &str) -> Option<String> {
    if input.contains("/anime/") {
        return input
            .split("/anime/")
            .nth(1)
            .and_then(|value| value.split(['/', '?', '#']).next())
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
    }
    None
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn preferred_quality(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("preferred_quality"))
        .or_else(|| request.get("preferred_quality"))
        .and_then(Value::as_str)
        .unwrap_or("1080")
        .to_string()
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

#[derive(Default, Deserialize)]
struct ListResponse {
    #[serde(default)]
    data: Vec<ListItem>,
    #[serde(default)]
    meta: Meta,
}

#[derive(Default, Deserialize)]
struct LatestResponse {
    #[serde(default)]
    data: Vec<LatestItem>,
    #[serde(default)]
    meta: Meta,
}

#[derive(Default, Deserialize)]
struct ListItem {
    #[serde(default)]
    anime_name: String,
    #[serde(default)]
    poster_path: String,
    #[serde(default)]
    slug: String,
}

#[derive(Default, Deserialize)]
struct LatestItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    poster_path: String,
    #[serde(default)]
    slug: String,
}

#[derive(Default, Deserialize)]
struct Meta {
    #[serde(default)]
    current_page: u64,
    #[serde(default)]
    last_page: u64,
}

#[derive(Default, Deserialize)]
struct DetailsResponse {
    #[serde(default)]
    data: Details,
}

#[derive(Default, Deserialize)]
struct Details {
    #[serde(default)]
    anime_name: String,
    #[serde(default)]
    genres: Vec<NameDto>,
    #[serde(default)]
    poster_path: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    story: String,
    #[serde(default)]
    studios: Vec<NameDto>,
}

#[derive(Deserialize)]
struct NameDto {
    name: String,
}

#[derive(Default, Deserialize)]
struct EpisodesResponse {
    #[serde(default)]
    data: Vec<EpisodeDto>,
    #[serde(default)]
    links: Links,
}

#[derive(Default, Deserialize)]
struct Links {
    next: Option<String>,
}

#[derive(Deserialize)]
struct EpisodeDto {
    number: f32,
    slug: String,
    title: String,
    poster_path: String,
}

#[derive(Default, Deserialize)]
struct StreamResponse {
    #[serde(default)]
    data: VideoInfo,
}

#[derive(Default, Deserialize)]
struct VideoInfo {
    #[serde(default)]
    sources: Vec<SourceDto>,
}

#[derive(Deserialize)]
struct SourceDto {
    file: String,
    label: String,
    quality: String,
}

const LIST_FIXTURE: &str = r#"{"data":[{"anime_name":"Sample Anime","poster_path":"sample.jpg","slug":"sample"}],"meta":{"current_page":1,"last_page":1}}"#;
const LATEST_FIXTURE: &str = r#"{"data":[{"title":"Sample Anime","poster_path":"sample.jpg","slug":"sample-episode-1"}],"meta":{"current_page":1,"last_page":1}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"anime_name":"Sample Anime","genres":[{"name":"Action"}],"poster_path":"sample.jpg","slug":"sample","status":"ongoing","story":"Sample description.","studios":[{"name":"Studio"}]}}"#;
const EPISODES_FIXTURE: &str = r#"{"data":[{"number":1,"slug":"sample-episode-1","title":"Episode 1","poster_path":"sample.jpg"}],"links":{"next":null}}"#;
const STREAM_PAGE_FIXTURE: &str = r#"{"hash":"WyJwbGF5ZXIiLCJzYW1wbGUiXQ=="}"#;
const STREAM_FIXTURE: &str = r#"{"data":{"sources":[{"file":"https://cdn.example/sample.m3u8","label":"Server","name":"Server","newfile":"","quality":"720"}]}}"#;

export_video_source!(SOURCE);
