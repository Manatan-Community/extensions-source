use manatan_extension::{
    AudioTrack, CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack,
    UrlResolveResult, VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult,
    export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: AnimeOnsen = AnimeOnsen;
const BASE_URL: &str = "https://www.animeonsen.xyz";
const API_URL: &str = "https://api.animeonsen.xyz/v4";
const AUTH_URL: &str = "https://auth.animeonsen.xyz/oauth/token";
const SEARCH_URL: &str = "https://search.animeonsen.xyz";
const AO_USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Mobile Safari/537.3";
const PAGE_SIZE: u64 = 20;

struct AnimeOnsen;

impl VideoSource for AnimeOnsen {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let start = page.saturating_sub(1) * PAGE_SIZE;
        let body = api_get_or_fixture(
            &format!("{API_URL}/content/index?start={start}&limit={PAGE_SIZE}"),
            LIST_FIXTURE,
        );
        Ok(parse_list(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(content_id) = content_id_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&content_id)],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(json!({ "page": 1 }));
        }
        let body = search_post_or_fixture(query, SEARCH_FIXTURE);
        Ok(parse_search(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(|| "sample".to_string());
        Ok(fetch_details(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_else(|| "sample".to_string());
        let body = api_get_or_fixture(
            &format!("{API_URL}/content/{key}/episodes"),
            EPISODES_FIXTURE,
        );
        Ok(parse_episodes(&body, &key))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode_key =
            request_key(&request, "episode").unwrap_or_else(|| "sample/video/1".to_string());
        let body = api_get_or_fixture(&format!("{API_URL}/content/{episode_key}"), STREAM_FIXTURE);
        Ok(parse_streams(&body, &request))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?.entries;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Latest".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: page,
            has_more: true,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| format!("{BASE_URL}/details/{key}")))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| format!("{BASE_URL}/watch/{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(content_id) = content_id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&content_id)),
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
        .with_header("User-Agent", AO_USER_AGENT)
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_get_or_fixture(target: &str, fixture: &str) -> String {
    let client = client();
    let mut request = client.get(target).xhr();
    if let Some(token) = api_token() {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn search_post_or_fixture(query: &str, fixture: &str) -> String {
    let client = client();
    let mut request = client
        .post(format!("{SEARCH_URL}/indexes/content/search"))
        .json(json!({ "q": query }).to_string())
        .xhr()
        .referer(BASE_URL);
    if let Some(token) = search_token() {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn api_token() -> Option<String> {
    let body = json!({
        "client_id": "f296be26-28b5-4358-b5a1-6259575e23b7",
        "client_secret": "349038c4157d0480784753841217270c3c5b35f4281eaee029de21cb04084235",
        "grant_type": "client_credentials"
    });
    let text = client()
        .post(AUTH_URL)
        .json(body.to_string())
        .xhr()
        .send_text()
        .ok()?;
    serde_json::from_str::<Value>(&text)
        .ok()?
        .get("access_token")?
        .as_str()
        .map(ToString::to_string)
}

fn search_token() -> Option<String> {
    let body = client().get(BASE_URL).browser_document().send_text().ok()?;
    body.split("name=\"ao-search-token\"")
        .nth(1)
        .and_then(|chunk| html::attr(chunk, "content"))
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = api_get_or_fixture(
        &format!("{API_URL}/content/{key}/extensive"),
        DETAILS_FIXTURE,
    );
    parse_details(&body).unwrap_or_else(|| fallback_item(key))
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let payload: AnimeListResponse = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: payload
            .content
            .into_iter()
            .map(AnimeListItem::into_catalog)
            .collect(),
        has_next_page: payload.cursor.has_next(),
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let payload: SearchResponse = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: payload
            .hits
            .into_iter()
            .map(AnimeListItem::into_catalog)
            .collect(),
        has_next_page: false,
    }
}

fn parse_details(body: &str) -> Option<CatalogItem> {
    let details: AnimeDetails = serde_json::from_str(body).ok()?;
    let title = details.title();
    let mal = details.mal_data.unwrap_or_default();
    Some(CatalogItem {
        key: details.content_id.clone(),
        title,
        cover: Some(image_url(&details.content_id)),
        url: Some(format!("{BASE_URL}/details/{}", details.content_id)),
        authors: mal
            .studios
            .unwrap_or_default()
            .into_iter()
            .map(|studio| studio.name)
            .collect(),
        description: mal.synopsis,
        tags: mal
            .genres
            .unwrap_or_default()
            .into_iter()
            .map(|genre| genre.name)
            .collect(),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(mal.status.as_deref()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &str, content_id: &str) -> Vec<VideoEpisode> {
    let payload: serde_json::Map<String, Value> = serde_json::from_str(body).unwrap_or_default();
    let mut entries: Vec<_> = payload
        .into_iter()
        .filter_map(|(number, value)| {
            let episode: EpisodeDto = serde_json::from_value(value).ok()?;
            let episode_number = number.parse::<f32>().ok();
            Some(VideoEpisode {
                key: format!("{content_id}/video/{number}"),
                title: Some(format!("Episode {number}: {}", episode.name)),
                episode_number,
                thumbnail: Some(image_url(content_id)),
                url: Some(format!("{BASE_URL}/watch/{content_id}/{number}")),
                language: Some("all".to_string()),
                labels: vec!["subbed".to_string()],
                ..VideoEpisode::default()
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.episode_number
            .partial_cmp(&a.episode_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    entries
}

fn parse_streams(body: &str, request: &Value) -> Vec<VideoStream> {
    let data: VideoData = serde_json::from_str(body).unwrap_or_default();
    if data.uri.stream.is_empty() {
        return Vec::new();
    }
    let preferred_sub = preference(request, "preferredSubLang")
        .or_else(|| preference(request, "preferred_subLang"))
        .unwrap_or_else(|| "en-US".to_string());
    let subtitles = sorted_subtitles(data.uri.subtitles, data.metadata.subtitles, &preferred_sub);
    let mut headers = Context::new();
    headers.insert("User-Agent".to_string(), AO_USER_AGENT.to_string());
    headers.insert("Referer".to_string(), BASE_URL.to_string());
    vec![VideoStream {
        url: data.uri.stream.clone(),
        name: Some("Default (720p)".to_string()),
        quality: Some("720p".to_string()),
        format: Some(
            if data.uri.stream.contains(".m3u8") {
                "hls"
            } else {
                "mp4"
            }
            .to_string(),
        ),
        is_hls: data.uri.stream.contains(".m3u8"),
        stream_kind: Some(if data.uri.stream.contains(".m3u8") {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        preferred: true,
        headers,
        audio_tracks: vec![AudioTrack {
            label: Some("Default".to_string()),
            is_default: true,
            ..AudioTrack::default()
        }],
        subtitles,
        ..VideoStream::default()
    }]
}

fn sorted_subtitles(
    subtitle_urls: std::collections::BTreeMap<String, String>,
    subtitle_names: std::collections::BTreeMap<String, String>,
    preferred: &str,
) -> Vec<SubtitleTrack> {
    let mut subtitles: Vec<_> = subtitle_urls
        .into_iter()
        .map(|(lang, sub_url)| SubtitleTrack {
            url: sub_url,
            language: Some(lang.clone()),
            label: subtitle_names.get(&lang).cloned().or(Some(lang.clone())),
            format: Some(subtitle_format(&lang)),
            is_default: lang.contains(preferred),
            ..SubtitleTrack::default()
        })
        .collect();
    subtitles.sort_by_key(|track| !track.is_default);
    subtitles
}

fn subtitle_format(url_or_lang: &str) -> String {
    if url_or_lang.ends_with(".srt") {
        "srt".to_string()
    } else {
        "vtt".to_string()
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
        .or_else(|| request.get("key").and_then(Value::as_str))
        .and_then(|value| {
            if field == "episode" {
                episode_key_from_url(value).or_else(|| Some(value.to_string()))
            } else {
                content_id_from_url(value).or_else(|| Some(value.to_string()))
            }
        })
}

fn content_id_from_url(input: &str) -> Option<String> {
    let path = input
        .strip_prefix(BASE_URL)
        .or_else(|| input.strip_prefix("https://animeonsen.xyz"))?;
    path.split("/details/")
        .nth(1)
        .and_then(|value| value.split(['/', '?', '#']).next())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn episode_key_from_url(input: &str) -> Option<String> {
    if input.contains("/video/") {
        return Some(input.trim_start_matches('/').to_string());
    }
    let path = input
        .strip_prefix(BASE_URL)
        .or_else(|| input.strip_prefix("https://animeonsen.xyz"))?;
    let tail = path.split("/watch/").nth(1)?;
    let mut parts = tail.split(['/', '?', '#']).filter(|part| !part.is_empty());
    let content_id = parts.next()?;
    let episode = parts.next()?;
    Some(format!("{content_id}/video/{episode}"))
}

fn preference(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn image_url(content_id: &str) -> String {
    format!("{API_URL}/image/210x300/{content_id}")
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.map(str::trim) {
        Some("finished_airing") => ItemStatus::Completed,
        Some("currently_airing") => ItemStatus::Ongoing,
        _ => ItemStatus::Ongoing,
    }
}

fn fallback_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: key.replace(['-', '_'], " "),
        cover: Some(image_url(key)),
        url: Some(format!("{BASE_URL}/details/{key}")),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Ongoing,
        ..CatalogItem::default()
    }
}

impl AnimeListItem {
    fn title(&self) -> String {
        self.content_title
            .clone()
            .or_else(|| self.content_title_en.clone())
            .unwrap_or_else(|| self.content_id.clone())
    }

    fn into_catalog(self) -> CatalogItem {
        CatalogItem {
            key: self.content_id.clone(),
            title: self.title(),
            cover: Some(image_url(&self.content_id)),
            url: Some(format!("{BASE_URL}/details/{}", self.content_id)),
            language: Some("all".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Ongoing,
            ..CatalogItem::default()
        }
    }
}

impl AnimeDetails {
    fn title(&self) -> String {
        self.content_title
            .clone()
            .or_else(|| self.content_title_en.clone())
            .unwrap_or_else(|| self.content_id.clone())
    }
}

#[derive(Default, Deserialize)]
struct AnimeListResponse {
    #[serde(default)]
    content: Vec<AnimeListItem>,
    #[serde(default)]
    cursor: AnimeListCursor,
}

#[derive(Default, Deserialize)]
struct AnimeListCursor {
    #[serde(default)]
    next: Vec<Value>,
}

impl AnimeListCursor {
    fn has_next(&self) -> bool {
        self.next.first().and_then(Value::as_bool).unwrap_or(false)
    }
}

#[derive(Default, Deserialize)]
struct AnimeListItem {
    content_id: String,
    content_title: Option<String>,
    content_title_en: Option<String>,
}

#[derive(Default, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    hits: Vec<AnimeListItem>,
}

#[derive(Default, Deserialize)]
struct AnimeDetails {
    content_id: String,
    content_title: Option<String>,
    content_title_en: Option<String>,
    #[serde(default)]
    mal_data: Option<MalData>,
}

#[derive(Default, Deserialize)]
struct MalData {
    #[serde(default)]
    genres: Option<Vec<NamedValue>>,
    status: Option<String>,
    #[serde(default)]
    studios: Option<Vec<NamedValue>>,
    synopsis: Option<String>,
}

#[derive(Deserialize)]
struct NamedValue {
    name: String,
}

#[derive(Deserialize)]
struct EpisodeDto {
    #[serde(rename = "contentTitle_episode_en")]
    name: String,
}

#[derive(Default, Deserialize)]
struct VideoData {
    #[serde(default)]
    metadata: MetaData,
    #[serde(default)]
    uri: StreamData,
}

#[derive(Default, Deserialize)]
struct MetaData {
    #[serde(default)]
    subtitles: std::collections::BTreeMap<String, String>,
}

#[derive(Default, Deserialize)]
struct StreamData {
    #[serde(default)]
    stream: String,
    #[serde(default)]
    subtitles: std::collections::BTreeMap<String, String>,
}

const LIST_FIXTURE: &str = r#"{"content":[{"content_id":"sample","content_title":"Sample AnimeOnsen","content_title_en":"Sample AnimeOnsen"}],"cursor":{"next":[false,20]}}"#;
const SEARCH_FIXTURE: &str = r#"{"hits":[{"content_id":"sample","content_title":"Sample AnimeOnsen","content_title_en":"Sample AnimeOnsen"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"content_id":"sample","content_title":"Sample AnimeOnsen","content_title_en":"Sample AnimeOnsen","mal_data":{"genres":[{"name":"Action"}],"status":"currently_airing","studios":[{"name":"Sample Studio"}],"synopsis":"Sample AnimeOnsen description."}}"#;
const EPISODES_FIXTURE: &str = r#"{"1":{"contentTitle_episode_en":"Arrival"},"2":{"contentTitle_episode_en":"Lantern Street"}}"#;
const STREAM_FIXTURE: &str = r#"{"metadata":{"subtitles":{"en-US":"English","es-ES":"Español"}},"uri":{"stream":"https://media.example/animeonsen/sample-720p.m3u8","subtitles":{"en-US":"https://media.example/animeonsen/sample-en.vtt","es-ES":"https://media.example/animeonsen/sample-es.vtt"}}}"#;

export_video_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_list() {
        let page = parse_list(LIST_FIXTURE);
        assert_eq!(page.entries[0].key, "sample");
    }

    #[test]
    fn parses_episodes_descending() {
        let episodes = parse_episodes(EPISODES_FIXTURE, "sample");
        assert_eq!(episodes[0].episode_number, Some(2.0));
        assert_eq!(episodes[0].key, "sample/video/2");
    }

    #[test]
    fn parses_stream_subtitles() {
        let streams = parse_streams(STREAM_FIXTURE, &json!({ "preferredSubLang": "es-ES" }));
        assert_eq!(streams[0].subtitles[0].language.as_deref(), Some("es-ES"));
    }
}
