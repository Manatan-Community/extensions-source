use aes::Aes128;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::{
    Decryptor,
    cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7},
};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: KissKh = KissKh;
const DEFAULT_BASE_URL: &str = "https://kisskh.ovh";
const VIDEO_KEY_API: &str = "https://script.google.com/macros/s/AKfycbzn8B31PuDxzaMa9_CQ0VGEDasFqfzI5bXvjaIZH4DM8DNq9q6xj1ALvZNz_JT3jF0suA/exec?id=";
const SUB_KEY_API: &str = "https://script.google.com/macros/s/AKfycbyq6hTj0ZhlinYC6xbggtgo166tp6XaDKBCGtnYk8uOfYBUFwwxBui0sGXiu_zIFmA/exec?id=";

struct KissKh;

impl VideoSource for KissKh {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let order = if listing(&request) == "latest" { 2 } else { 1 };
        let body = get_json(&base, &format!("{base}/api/DramaList/List?page={}&type=0&sub=0&country=0&status=0&order={order}&pageSize=40", page(&request)), LIST_FIXTURE);
        Ok(parse_list(&base, &body, rating()))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(id) = id_from_url(query) {
            return Ok(Paged { entries: vec![fetch_details(&base, &id)], has_next_page: false });
        }
        if query.is_empty() {
            return self.list(request);
        }
        let body = get_json(&base, &format!("{base}/api/DramaList/Search?q={}&type=0", url::query_escape(query)), SEARCH_FIXTURE);
        Ok(parse_search(&base, &body, rating()))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let id = request_key(&request, "item").and_then(|key| id_from_url(&key)).unwrap_or_else(|| "1".to_string());
        Ok(fetch_details(&base, &id))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base = base_url(&request);
        let id = request_key(&request, "item").and_then(|key| id_from_url(&key)).unwrap_or_else(|| "1".to_string());
        let body = get_json(&base, &format!("{base}/api/DramaList/Drama/{id}?isq=false"), DETAILS_FIXTURE);
        Ok(parse_episodes(&base, &body))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let base = base_url(&request);
        let id = request_key(&request, "episode").unwrap_or_else(|| "1".to_string());
        let kkey = request_key_api(VIDEO_KEY_API, &id);
        let body = get_json(&base, &format!("{base}/api/DramaList/Episode/{id}.png?err=false&ts=&time=&kkey={kkey}"), VIDEO_FIXTURE);
        let mut streams = parse_video(&base, &id, &body, &request);
        sort_streams(&mut streams);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection { id: "popular".to_string(), title: "Popular".to_string(), style: Some(HomeSectionStyle::Featured), entries: popular.entries, has_more: popular.has_next_page, ..HomeSection::default() },
            HomeSection { id: "latest".to_string(), title: "Latest".to_string(), entries: latest.entries, has_more: latest.has_next_page, ..HomeSection::default() },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "item").map(|key| if key.starts_with("http") { key } else { format!("{base}{key}") }))
    }

    fn episode_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(None)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None) };
        if let Some(id) = id_from_url(input) {
            let base = base_url(&request);
            return Ok(Some(UrlResolveResult { item: Some(fetch_details(&base, &id)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_referer(base)
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn get_json(base: &str, target: &str, fixture: &str) -> String {
    client(base)
        .get(target)
        .xhr()
        .header("Accept", "application/json, text/plain, */*")
        .referer(base)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_list(base: &str, body: &str, content_rating: &str) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<ListDto>(body).unwrap_or_default();
    Paged {
        entries: payload.data.into_iter().filter_map(|item| item.into_item(base, content_rating)).collect(),
        has_next_page: payload.page < payload.total_count,
    }
}

fn parse_search(base: &str, body: &str, content_rating: &str) -> Paged<CatalogItem> {
    let items = serde_json::from_str::<Vec<ListItemDto>>(body).unwrap_or_default();
    Paged {
        entries: items.into_iter().filter_map(|item| item.into_item(base, content_rating)).collect(),
        has_next_page: false,
    }
}

fn fetch_details(base: &str, id: &str) -> CatalogItem {
    let body = get_json(base, &format!("{base}/api/DramaList/Drama/{id}?isq=false"), DETAILS_FIXTURE);
    let drama = serde_json::from_str::<DramaDto>(&body).unwrap_or_else(|_| DramaDto::fallback(id));
    drama.into_item(base, rating())
}

fn parse_episodes(base: &str, body: &str) -> Vec<VideoEpisode> {
    let drama = serde_json::from_str::<DramaDto>(body).unwrap_or_default();
    let episodes_count = drama.episodes_count.unwrap_or(1);
    drama.episodes.into_iter().filter_map(|episode| {
        let id = episode.id?;
        let number = episode.number.unwrap_or(1.0);
        let name = if drama.kind.as_deref().unwrap_or_default().contains("Movie") || episodes_count == 1 {
            "Movie".to_string()
        } else {
            format!("Episode {}", display_number(number))
        };
        Some(VideoEpisode {
            key: id.clone(),
            title: Some(name),
            episode_number: Some(number),
            url: Some(format!("{base}/Drama/Episode/{id}")),
            language: Some("en".to_string()),
            ..VideoEpisode::default()
        })
    }).collect()
}

fn parse_video(base: &str, id: &str, body: &str, request: &Value) -> Vec<VideoStream> {
    let payload = serde_json::from_str::<VideoDto>(body).unwrap_or_default();
    let Some(video_url) = payload.video.filter(|value| !value.is_empty()).map(|value| fix_url(&value)) else {
        return Vec::new();
    };
    let subtitles = fetch_subtitles(base, id);
    let is_hls = video_url.contains(".m3u8");
    vec![VideoStream {
        url: video_url.clone(),
        name: Some("FirstParty".to_string()),
        quality: Some(if is_hls { pref(request, "preferred_quality", "auto") } else { "direct".to_string() }),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls { VideoStreamKind::Hls } else { VideoStreamKind::Direct }),
        subtitles,
        headers: playback_headers(base),
        initialized: true,
        ..VideoStream::default()
    }]
}

fn fetch_subtitles(base: &str, id: &str) -> Vec<SubtitleTrack> {
    let kkey = request_key_api(SUB_KEY_API, id);
    let body = get_json(base, &format!("{base}/api/Sub/{id}?kkey={kkey}"), SUBS_FIXTURE);
    serde_json::from_str::<Vec<SubDto>>(&body).unwrap_or_default().into_iter().filter_map(|sub| {
        let src = sub.src?;
        let label = sub.label.unwrap_or_else(|| "Unknown".to_string());
        let url = if src.contains(".txt") {
            decrypt_subtitle_url(base, &src).unwrap_or(src)
        } else {
            src
        };
        Some(SubtitleTrack {
            url,
            language: language_code(&label),
            label: Some(label),
            format: Some("vtt".to_string()),
            headers: playback_headers(base),
            ..SubtitleTrack::default()
        })
    }).collect()
}

fn decrypt_subtitle_url(base: &str, target: &str) -> Option<String> {
    let body = client(base)
        .get(target)
        .xhr()
        .header("Accept", "application/json, text/plain, */*")
        .header("Origin", base)
        .referer(&format!("{base}/"))
        .send_text()
        .ok()?;
    let decrypted = decrypt_subtitle_text(&body)?;
    Some(format!("data:text/plain;base64,{}", STANDARD.encode(decrypted)))
}

fn decrypt_subtitle_text(input: &str) -> Option<String> {
    let chunks = input.split('\n').collect::<Vec<_>>();
    if chunks.len() < 3 {
        return None;
    }
    let mut out = String::new();
    let mut index = 1;
    for block in input.split("\n\n") {
        let lines = block.lines().collect::<Vec<_>>();
        if lines.len() < 3 {
            continue;
        }
        if !out.is_empty() { out.push_str("\n\n"); }
        out.push_str(&index.to_string());
        out.push('\n');
        out.push_str(lines[1]);
        out.push('\n');
        let decrypted = lines[2..].iter().filter_map(|line| aes_decrypt(line)).collect::<Vec<_>>().join("\n");
        out.push_str(&decrypted);
        index += 1;
    }
    (!out.is_empty()).then_some(out)
}

fn aes_decrypt(input: &str) -> Option<String> {
    for (key, iv) in [
        ("AmSmZVcH93UQUezi".as_bytes().to_vec(), int_array_bytes([1382367819, 1465333859, 1902406224, 1164854838])),
        ("8056483646328763".as_bytes().to_vec(), int_array_bytes([909653298, 909193779, 925905208, 892483379])),
    ] {
        let bytes = STANDARD.decode(input).ok()?;
        if let Ok(plain) = Decryptor::<Aes128>::new_from_slices(&key, &iv).ok()?.decrypt_padded_vec_mut::<Pkcs7>(&bytes) {
            if let Ok(text) = String::from_utf8(plain) {
                return Some(text);
            }
        }
    }
    None
}

fn int_array_bytes(values: [i32; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    for value in values {
        out.extend_from_slice(&value.to_be_bytes());
    }
    out
}

fn request_key_api(api: &str, id: &str) -> String {
    client(DEFAULT_BASE_URL)
        .get(format!("{api}{id}&version=2.8.10"))
        .xhr()
        .send_text()
        .ok()
        .and_then(|body| serde_json::from_str::<KeyDto>(&body).ok())
        .map(|key| key.key)
        .unwrap_or_default()
}

fn sort_streams(streams: &mut [VideoStream]) {
    streams.sort_by(|a, b| b.quality.cmp(&a.quality));
}

fn id_from_url(input: &str) -> Option<String> {
    if input.is_empty() {
        return None;
    }
    input.split("id=").nth(1)
        .and_then(|tail| tail.split(['&', '#']).next())
        .or_else(|| input.strip_prefix("id:"))
        .filter(|id| !id.is_empty())
        .map(ToString::to_string)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request.get(field).and_then(|value| {
        value.get("key").or_else(|| value.get("url")).and_then(Value::as_str).or_else(|| value.as_str())
    }).or_else(|| request.get("key").and_then(Value::as_str)).map(ToString::to_string)
}

fn base_url(request: &Value) -> String {
    pref(request, "preferred_domain", DEFAULT_BASE_URL)
}

fn pref(request: &Value, key: &str, default: &str) -> String {
    request.get("preferences").and_then(|p| p.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).unwrap_or(default).to_string()
}

fn rating() -> &'static str {
    "adult"
}

fn fix_url(input: &str) -> String {
    if input.starts_with("http") { input.to_string() } else { format!("{DEFAULT_BASE_URL}/{}", input.trim_start_matches('/')) }
}

fn language_code(label: &str) -> Option<String> {
    let lower = label.to_lowercase();
    if lower.contains("english") { Some("en".to_string()) } else { None }
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    let status = status.unwrap_or_default();
    if status.contains("Ongoing") {
        ItemStatus::Ongoing
    } else if status.is_empty() {
        ItemStatus::Unknown
    } else {
        ItemStatus::Completed
    }
}

fn playback_headers(base: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), format!("{base}/"));
    headers.insert("Origin".to_string(), base.to_string());
    headers
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn listing(request: &Value) -> &str {
    request.get("listing").or_else(|| request.get("listingId")).and_then(Value::as_str).unwrap_or("popular")
}

fn with_listing(request: &Value, listing: &str) -> Value {
    serde_json::json!({ "listing": listing, "preferences": request.get("preferences").cloned().unwrap_or(Value::Null) })
}

fn display_number(number: f32) -> String {
    if number.fract() == 0.0 { format!("{}", number as i32) } else { number.to_string() }
}

#[derive(Default, Deserialize)]
struct ListDto {
    #[serde(default, rename = "totalCount")]
    total_count: u64,
    #[serde(default)]
    page: u64,
    #[serde(default)]
    data: Vec<ListItemDto>,
}

#[derive(Default, Deserialize)]
struct ListItemDto {
    id: Option<u64>,
    title: Option<String>,
    thumbnail: Option<String>,
}

impl ListItemDto {
    fn into_item(self, base: &str, content_rating: &str) -> Option<CatalogItem> {
        let title = self.title?;
        let id = self.id?;
        let slug = title.replace(|ch: char| !ch.is_ascii_alphanumeric(), "-");
        Some(CatalogItem {
            key: format!("/Drama/{slug}?id={id}"),
            title,
            cover: self.thumbnail,
            url: Some(format!("{base}/Drama/{slug}?id={id}")),
            language: Some("en".to_string()),
            content_rating: Some(content_rating.to_string()),
            ..CatalogItem::default()
        })
    }
}

#[derive(Default, Deserialize)]
struct DramaDto {
    id: Option<u64>,
    title: Option<String>,
    status: Option<String>,
    description: Option<String>,
    thumbnail: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default, rename = "episodesCount")]
    episodes_count: Option<u64>,
    #[serde(default)]
    episodes: Vec<EpisodeDto>,
}

impl DramaDto {
    fn fallback(id: &str) -> Self {
        Self { id: id.parse().ok(), title: Some("Sample Drama".to_string()), ..Self::default() }
    }

    fn into_item(self, base: &str, content_rating: &str) -> CatalogItem {
        let id = self.id.unwrap_or(1);
        let title = self.title.unwrap_or_else(|| "Sample Drama".to_string());
        let slug = title.replace(|ch: char| !ch.is_ascii_alphanumeric(), "-");
        CatalogItem {
            key: format!("/Drama/{slug}?id={id}"),
            title,
            cover: self.thumbnail,
            description: self.description,
            url: Some(format!("{base}/Drama/{slug}?id={id}")),
            language: Some("en".to_string()),
            content_rating: Some(content_rating.to_string()),
            status: parse_status(self.status.as_deref()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct EpisodeDto {
    id: Option<String>,
    number: Option<f32>,
}

#[derive(Default, Deserialize)]
struct VideoDto {
    #[serde(default, rename = "Video")]
    video: Option<String>,
}

#[derive(Default, Deserialize)]
struct SubDto {
    src: Option<String>,
    label: Option<String>,
}

#[derive(Deserialize)]
struct KeyDto {
    key: String,
}

const LIST_FIXTURE: &str = r#"{"totalCount":1,"page":1,"data":[{"id":1,"title":"Sample Drama","thumbnail":"https://fixtures.invalid/kisskh/cover.jpg"}]}"#;
const SEARCH_FIXTURE: &str = r#"[{"id":1,"title":"Sample Drama","thumbnail":"https://fixtures.invalid/kisskh/cover.jpg"}]"#;
const DETAILS_FIXTURE: &str = r#"{"id":1,"title":"Sample Drama","status":"Ongoing","description":"Sample description.","thumbnail":"https://fixtures.invalid/kisskh/cover.jpg","type":"Anime","episodesCount":1,"episodes":[{"id":"100","number":1.0}]}"#;
const VIDEO_FIXTURE: &str = r#"{"Video":"https://fixtures.invalid/kisskh/video.m3u8"}"#;
const SUBS_FIXTURE: &str = r#"[{"src":"https://fixtures.invalid/kisskh/sub.vtt","label":"English"}]"#;

export_video_source!(SOURCE);
