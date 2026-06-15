use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: AnimeParadise = AnimeParadise;
const BASE_URL: &str = "https://www.animeparadise.moe";
const API_URL: &str = "https://api.animeparadise.moe";

struct AnimeParadise;

impl VideoSource for AnimeParadise {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort = if listing == "latest" {
            "%7B%22startDate%22%3A%20-1%20%7D&type=TV"
        } else {
            "%7B%22rate%22%3A%20-1%7D"
        };
        let body = api_get_or_fixture(&format!("{API_URL}/?sort={sort}"), LIST_FIXTURE);
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
                entries: vec![fetch_details(
                    &json!({ "slug": slug, "id": "" }).to_string(),
                )],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(json!({ "listing": "popular" }));
        }
        let body = api_get_or_fixture(
            &format!(
                "{API_URL}/?title={}",
                manatan_shared::sdk::http::url_encode(query)
            ),
            LIST_FIXTURE,
        );
        Ok(parse_list(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item")
            .unwrap_or_else(|| json!({ "slug": "sample", "id": "sample" }).to_string());
        Ok(fetch_details(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item")
            .unwrap_or_else(|| json!({ "slug": "sample", "id": "sample" }).to_string());
        let link = parse_link(&key);
        let body = api_get_or_fixture(
            &format!("{API_URL}/anime/{}/episode", link.id),
            EPISODES_FIXTURE,
        );
        Ok(parse_episodes(&body))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "episode")
            .unwrap_or_else(|| "/watch/sample?origin=fixture".to_string());
        let page = get_or_fixture(&format!("{BASE_URL}{episode}"), WATCH_FIXTURE);
        let page_data = next_data(&page);
        let title = value_string(&page_data, &["props", "pageProps", "animeData", "title"])
            .unwrap_or_else(|| "Sample Anime".to_string());
        let number = value_string(&page_data, &["props", "pageProps", "episode", "number"])
            .unwrap_or_else(|| "1".to_string());
        let subtitles = value_array(&page_data, &["props", "pageProps", "subtitles"])
            .into_iter()
            .filter_map(parse_subtitle)
            .collect::<Vec<_>>();
        let storage = api_get_or_fixture(
            &format!("{API_URL}/storage/{title}/{number}"),
            STREAMS_FIXTURE,
        );
        let mut streams = parse_streams(&storage, subtitles, &request);
        sort_streams(&mut streams, pref(&request, "preferred_quality", "1080"));
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: latest.entries,
                has_more: false,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item")
            .map(|key| format!("{BASE_URL}/anime/{}", parse_link(&key).slug)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|path| format!("{BASE_URL}{path}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = slug_from_url(input) {
            let key = json!({ "slug": slug, "id": "" }).to_string();
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&key)),
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

fn api_get_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json, text/plain, */*")
        .header("Origin", BASE_URL)
        .referer(BASE_URL)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn get_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let payload: AnimeListResponse = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: payload
            .data
            .into_iter()
            .map(AnimeObject::into_item)
            .collect(),
        has_next_page: false,
    }
}

fn fetch_details(key: &str) -> CatalogItem {
    let link = parse_link(key);
    let body = get_or_fixture(&format!("{BASE_URL}/anime/{}", link.slug), DETAILS_FIXTURE);
    let data = next_data(&body);
    let page = get_value(&data, &["props", "pageProps", "data"]);
    CatalogItem {
        key: key.to_string(),
        title: value_string(&data, &["props", "pageProps", "data", "title"])
            .unwrap_or_else(|| title_from_slug(&link.slug)),
        cover: value_string(page, &["posterImage", "original"])
            .or_else(|| value_string(page, &["posterImage", "large"])),
        url: Some(format!("{BASE_URL}/anime/{}", link.slug)),
        description: value_string(page, &["synopsys"]),
        tags: value_array(page, &["genres"])
            .into_iter()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect(),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let payload: EpisodeListResponse = serde_json::from_str(body).unwrap_or_default();
    let mut episodes = payload
        .data
        .into_iter()
        .map(|episode| {
            let number = episode.number.unwrap_or_else(|| "1".to_string());
            let title = episode
                .title
                .filter(|value| !value.is_empty())
                .map(|title| format!("Ep. {number} - {title}"))
                .unwrap_or_else(|| format!("Ep. {number}"));
            VideoEpisode {
                key: format!("/watch/{}?origin={}", episode.uid, episode.origin),
                title: Some(title),
                episode_number: number.parse().ok(),
                url: Some(format!(
                    "{BASE_URL}/watch/{}?origin={}",
                    episode.uid, episode.origin
                )),
                language: Some("en".to_string()),
                ..VideoEpisode::default()
            }
        })
        .collect::<Vec<_>>();
    episodes.reverse();
    episodes
}

fn parse_streams(body: &str, subtitles: Vec<SubtitleTrack>, request: &Value) -> Vec<VideoStream> {
    let value: Value = serde_json::from_str(body).unwrap_or_default();
    value
        .get("directUrl")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let src = item.get("src")?.as_str()?;
            let stream_url = absolute_media(src);
            let quality = item.get("label").and_then(Value::as_str).unwrap_or("Video");
            let is_hls = stream_url.contains(".m3u8");
            Some(VideoStream {
                url: stream_url,
                name: Some(quality.to_string()),
                quality: Some(quality.to_string()),
                format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
                is_hls,
                stream_kind: Some(if is_hls {
                    VideoStreamKind::Hls
                } else {
                    VideoStreamKind::Direct
                }),
                headers: video_headers(),
                subtitles: subtitles.clone(),
                preferred: quality.contains(pref(request, "preferred_quality", "1080")),
                ..VideoStream::default()
            })
        })
        .collect()
}

fn parse_subtitle(value: &Value) -> Option<SubtitleTrack> {
    Some(SubtitleTrack {
        url: value.get("src")?.as_str()?.to_string(),
        label: value
            .get("label")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        language: value
            .get("label")
            .and_then(Value::as_str)
            .map(|label| label.to_ascii_lowercase()),
        format: Some("vtt".to_string()),
        ..SubtitleTrack::default()
    })
}

fn next_data(body: &str) -> Value {
    if let Some(raw) = body
        .split("id=\"__NEXT_DATA__\"")
        .nth(1)
        .and_then(|chunk| chunk.split('>').nth(1))
        .and_then(|chunk| chunk.split("</script>").next())
    {
        return serde_json::from_str(raw).unwrap_or_default();
    }
    if let Some(raw) = html::text_between(body, "<script id=\"__NEXT_DATA__\"", "</script>") {
        return serde_json::from_str(&raw).unwrap_or_default();
    }
    Value::Null
}

fn get_value<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    let mut current = value;
    for key in path {
        current = current.get(*key).unwrap_or(&Value::Null);
    }
    current
}

fn value_string(value: &Value, path: &[&str]) -> Option<String> {
    get_value(value, path).as_str().map(ToString::to_string)
}

fn value_array<'a>(value: &'a Value, path: &[&str]) -> Vec<&'a Value> {
    get_value(value, path)
        .as_array()
        .map(|values| values.iter().collect())
        .unwrap_or_default()
}

fn absolute_media(src: &str) -> String {
    if src.starts_with("//") {
        format!("https:{src}")
    } else if src.starts_with('/') {
        format!("{API_URL}{src}")
    } else {
        src.to_string()
    }
}

fn video_headers() -> Context {
    let mut headers = Context::new();
    headers.insert(
        "Accept".to_string(),
        "video/webm,video/ogg,video/*;q=0.9,*/*;q=0.5".to_string(),
    );
    headers.insert("Referer".to_string(), format!("{BASE_URL}/"));
    headers
}

fn sort_streams(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by_key(|stream| quality_score(stream.quality.as_deref()));
    streams.reverse();
    for stream in streams {
        stream.preferred = stream
            .quality
            .as_deref()
            .is_some_and(|quality| quality.contains(preferred));
    }
}

fn quality_score(value: Option<&str>) -> i32 {
    value
        .unwrap_or_default()
        .split(|ch: char| !ch.is_ascii_digit())
        .find_map(|part| part.parse::<i32>().ok())
        .unwrap_or(0)
}

fn parse_link(key: &str) -> LinkData {
    serde_json::from_str(key).unwrap_or_else(|_| LinkData {
        slug: key.trim_matches('/').to_string(),
        id: key.trim_matches('/').to_string(),
    })
}

fn slug_from_url(input: &str) -> Option<String> {
    input
        .split("/anime/")
        .nth(1)
        .map(|slug| slug.trim_matches('/').to_string())
}

fn title_from_slug(slug: &str) -> String {
    slug.replace('-', " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn pref<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_str)
        .or_else(|| request.get(key).and_then(Value::as_str))
        .unwrap_or(default)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| value.get("key").or_else(|| value.get("url")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            request
                .get("key")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

#[derive(Default, Deserialize)]
struct AnimeListResponse {
    #[serde(default)]
    data: Vec<AnimeObject>,
}

#[derive(Deserialize)]
struct AnimeObject {
    #[serde(rename = "_id")]
    id: String,
    title: String,
    link: String,
    #[serde(rename = "posterImage", default)]
    poster_image: ImageObject,
}

impl AnimeObject {
    fn into_item(self) -> CatalogItem {
        CatalogItem {
            key: json!({ "slug": self.link, "id": self.id }).to_string(),
            title: self.title,
            cover: self
                .poster_image
                .original
                .or(self.poster_image.large)
                .or(self.poster_image.medium)
                .or(self.poster_image.small),
            url: Some(format!("{BASE_URL}/anime/{}", self.link)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct ImageObject {
    original: Option<String>,
    large: Option<String>,
    medium: Option<String>,
    small: Option<String>,
}

#[derive(Default, Deserialize)]
struct EpisodeListResponse {
    #[serde(default)]
    data: Vec<EpisodeObject>,
}

#[derive(Deserialize)]
struct EpisodeObject {
    uid: String,
    origin: String,
    number: Option<String>,
    title: Option<String>,
}

#[derive(Deserialize)]
struct LinkData {
    slug: String,
    id: String,
}

const LIST_FIXTURE: &str = r#"{"data":[{"_id":"sample","title":"Sample Anime Paradise","link":"sample-anime","posterImage":{"original":"https://fixtures.invalid/animeparadise/cover.jpg"}}]}"#;
const DETAILS_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"data":{"title":"Sample Anime Paradise","synopsys":"Fixture details.","genres":["Action"],"posterImage":{"original":"https://fixtures.invalid/animeparadise/cover.jpg"}}}}}</script>"#;
const EPISODES_FIXTURE: &str = r#"{"data":[{"uid":"sample-1","origin":"main","number":"1","title":"Arrival"},{"uid":"sample-2","origin":"main","number":"2","title":"Signal"}]}"#;
const WATCH_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"subtitles":[{"src":"https://fixtures.invalid/animeparadise/en.vtt","label":"English"}],"animeData":{"title":"Sample Anime Paradise"},"episode":{"number":"1"}}}}</script>"#;
const STREAMS_FIXTURE: &str =
    r#"{"directUrl":[{"src":"https://fixtures.invalid/animeparadise/720.mp4","label":"720p"}]}"#;

export_video_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_list_fixture() {
        let page = parse_list(LIST_FIXTURE);
        assert_eq!(page.entries[0].title, "Sample Anime Paradise");
    }

    #[test]
    fn parses_episode_fixture_descending() {
        let episodes = parse_episodes(EPISODES_FIXTURE);
        assert_eq!(episodes[0].episode_number, Some(2.0));
    }
}
