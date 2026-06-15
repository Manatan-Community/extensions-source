use base64::{engine::general_purpose::STANDARD, Engine};
use manatan_extension::{
    abi::{ExtensionError, ExtensionResult},
    export_video_source,
    source::VideoSource,
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind,
};
use manatan_shared::{
    html,
    sdk::{http::HttpClient, Context, SearchRequest},
    url,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: Anime4Up = Anime4Up;
const BASE_URL: &str = "https://w1.anime4up.rest";
const VIDYARD_URL: &str = "https://play.vidyard.com";

struct Anime4Up;

impl VideoSource for Anime4Up {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = client()
            .get(format!("{BASE_URL}/anime-list-3/page/{}/", page(&request)))
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged {
                entries: vec![details_for(&path)],
                has_next_page: false,
            });
        }
        let target = if !query.is_empty() {
            format!(
                "{BASE_URL}/?search_param=animes&s={}",
                url::query_escape(query)
            )
        } else if let Some(path) = selected_filter_path(&request) {
            url::join_url(BASE_URL, &path)
        } else {
            return Err(error("Select a filter or enter a search query"));
        };
        let body = client()
            .get(target)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        Ok(details_for(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        let body = client()
            .get(item_url(&key))
            .browser_document()
            .send_text()
            .unwrap_or_default();
        let mut episodes = parse_episodes(&body);
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode").unwrap_or_default();
        let page_url = item_url(&key);
        let body = client()
            .get(&page_url)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        let mut links = BTreeSet::new();
        for input in ["watch_fhd", "watch_hd", "watch_SD"] {
            links.extend(decode_watch_servers(&body, input));
        }
        let mut streams = links
            .into_iter()
            .flat_map(|link| streams_from_hoster(&link, &page_url))
            .collect::<Vec<_>>();
        prefer_quality(
            &mut streams,
            pref_str(&request, "preferred_quality", "1080p"),
        );
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Anime List".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: page.entries,
            has_more: page.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| item_url(&key)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| item_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input) {
            let is_episode = path.contains("/episode/");
            return Ok(Some(UrlResolveResult {
                item: (!is_episode).then(|| details_for(&path)),
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

#[derive(Debug, Deserialize)]
struct WatchServerData {
    name: String,
    link: String,
    #[serde(default)]
    order: String,
    #[serde(default)]
    icon: bool,
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("anime-card-poster")
        .skip(1)
        .filter_map(parse_card)
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("pagination") && body.contains("next"),
    }
}

fn parse_card(block: &str) -> Option<CatalogItem> {
    let href = html::attr_after(block, "<a", "href")?;
    let key = path_from_url(&href).unwrap_or(href);
    let title = html::attr_after(block, "<img", "alt")
        .or_else(|| html::text_between(block, "<h", "</h>").map(|text| html::strip_tags(&text)))
        .unwrap_or_else(|| key.replace('/', " "));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(block, "<img", "src"),
        url: Some(item_url(&key)),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn details_for(key: &str) -> CatalogItem {
    let key = path_from_url(key).unwrap_or_else(|| key.to_string());
    let body = client()
        .get(item_url(&key))
        .browser_document()
        .send_text()
        .unwrap_or_default();
    let info = body
        .split("anime-info")
        .skip(1)
        .map(|block| html::strip_tags(block.split("</div>").next().unwrap_or_default()))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    let status_text = info.join(" ");
    CatalogItem {
        key: key.clone(),
        title: html::text_between(&body, "anime-details-title", "</h1>")
            .map(|text| html::strip_tags(&text))
            .filter(|text| !text.is_empty())
            .or_else(|| html::attr_after(&body, "<meta property=\"og:title\"", "content"))
            .unwrap_or_else(|| key.replace('/', " ")),
        cover: html::attr_after(&body, "img.thumbnail", "src")
            .or_else(|| html::attr_after(&body, "<meta property=\"og:image\"", "content"))
            .or_else(|| html::attr_after(&body, "<img", "src")),
        url: Some(item_url(&key)),
        description: {
            let mut text = info.join("\n");
            if let Some(story) = html::text_between(&body, "anime-story", "</p>") {
                let story = html::strip_tags(&story);
                if !story.is_empty() {
                    if !text.is_empty() {
                        text.push_str("\n\n");
                    }
                    text.push_str(&story);
                }
            }
            (!text.is_empty()).then_some(text)
        },
        tags: parse_tags(&body),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: if status_text.contains("يعرض الان") {
            ItemStatus::Ongoing
        } else if status_text.contains("مكتمل") {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        },
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let mut out = Vec::new();
    for marker in ["episodes-card-title", "all-episodes-list"] {
        for block in body.split(marker).skip(1) {
            let Some(href) = html::attr_after(block, "<a", "href") else {
                continue;
            };
            let title = html::text_between(block, "<a", "</a>")
                .map(|text| html::strip_tags(&text))
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| href.clone());
            let key = path_from_url(&href).unwrap_or(href);
            if out.iter().any(|episode: &VideoEpisode| episode.key == key) {
                continue;
            }
            out.push(VideoEpisode {
                key: key.clone(),
                title: Some(title.clone()),
                episode_number: title
                    .rsplit(' ')
                    .next()
                    .and_then(|value| value.parse::<f32>().ok()),
                url: Some(item_url(&key)),
                language: Some("ar".to_string()),
                ..VideoEpisode::default()
            });
        }
    }
    out
}

fn decode_watch_servers(body: &str, input_name: &str) -> Vec<String> {
    let marker = format!("name='{input_name}'");
    let marker_alt = format!("name=\"{input_name}\"");
    let encoded = html::attr_after(body, &marker, "value")
        .or_else(|| html::attr_after(body, &marker_alt, "value"))
        .unwrap_or_else(|| "W10=".to_string());
    let bytes = STANDARD.decode(encoded).unwrap_or_default();
    let decoded = String::from_utf8(bytes).unwrap_or_else(|_| "[]".to_string());
    serde_json::from_str::<Vec<WatchServerData>>(&decoded)
        .unwrap_or_default()
        .into_iter()
        .map(|server| {
            let _ = (&server.name, &server.order, server.icon);
            server.link
        })
        .filter(|link| !link.trim().is_empty())
        .collect()
}

fn streams_from_hoster(target: &str, page_url: &str) -> Vec<VideoStream> {
    if target.contains("shared") {
        if let Some(stream) = shared_stream(target, page_url) {
            return vec![stream];
        }
    }
    if target.contains("vidyard") {
        let streams = vidyard_streams(target);
        if !streams.is_empty() {
            return streams;
        }
    }
    vec![external_stream(target, hoster_name(target), page_url)]
}

fn shared_stream(target: &str, page_url: &str) -> Option<VideoStream> {
    let body = client()
        .get(target)
        .referer(page_url)
        .browser_document()
        .send_text()
        .ok()?;
    let src = html::attr_after(&body, "<source", "src")?;
    Some(VideoStream {
        url: src.clone(),
        name: Some("4Shared".to_string()),
        quality: Some("mirror".to_string()),
        format: Some(if src.contains(".m3u8") { "hls" } else { "mp4" }.to_string()),
        is_hls: src.contains(".m3u8"),
        stream_kind: Some(if src.contains(".m3u8") {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(page_url),
        initialized: true,
        ..VideoStream::default()
    })
}

fn vidyard_streams(target: &str) -> Vec<VideoStream> {
    let id = target
        .split("vidyard.com/")
        .nth(1)
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default();
    if id.is_empty() {
        return Vec::new();
    }
    let body = HttpClient::browser()
        .with_referer(VIDYARD_URL)
        .get(format!("{VIDYARD_URL}/player/{id}.json"))
        .xhr()
        .send_text()
        .unwrap_or_default();
    let root: Value = serde_json::from_str(&body).unwrap_or_default();
    let mut streams = Vec::new();
    collect_vidyard_urls(&root, &mut streams);
    streams
        .into_iter()
        .map(|(quality, stream_url)| VideoStream {
            url: stream_url.clone(),
            name: Some("Vidyard".to_string()),
            quality: Some(quality),
            format: Some(
                if stream_url.contains(".m3u8") {
                    "hls"
                } else {
                    "mp4"
                }
                .to_string(),
            ),
            is_hls: stream_url.contains(".m3u8"),
            stream_kind: Some(if stream_url.contains(".m3u8") {
                VideoStreamKind::Hls
            } else {
                VideoStreamKind::Direct
            }),
            headers: referer_headers(VIDYARD_URL),
            initialized: true,
            ..VideoStream::default()
        })
        .collect()
}

fn collect_vidyard_urls(value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::Object(object) => {
            if let Some(url_value) = object.get("url").and_then(Value::as_str) {
                let quality = object
                    .get("profile")
                    .or_else(|| object.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("Vidyard")
                    .to_string();
                out.push((quality, url_value.to_string()));
            }
            for child in object.values() {
                collect_vidyard_urls(child, out);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_vidyard_urls(child, out);
            }
        }
        _ => {}
    }
}

fn external_stream(target: &str, name: String, referer: &str) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(name.clone()),
        quality: Some(name),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        requires_proxy: false,
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn parse_tags(body: &str) -> Vec<String> {
    body.split("anime-genres")
        .nth(1)
        .unwrap_or_default()
        .split("</ul>")
        .next()
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .map(html::strip_tags)
        .filter(|tag| !tag.is_empty())
        .collect()
}

fn selected_filter_path(request: &Value) -> Option<String> {
    let genre = filter_str(request, "genre", "");
    let anime_type = filter_str(request, "type", "");
    let status = filter_str(request, "status", "");
    if !genre.is_empty() {
        Some(format!("/anime-genre/{genre}"))
    } else if !anime_type.is_empty() {
        Some(format!("/anime-type/{anime_type}"))
    } else if !status.is_empty() {
        Some(format!("/anime-status/{status}"))
    } else {
        None
    }
}

fn prefer_quality(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by_key(|stream| {
        !stream
            .quality
            .as_deref()
            .unwrap_or_default()
            .contains(preferred)
    });
}

fn hoster_name(target: &str) -> String {
    target
        .split("://")
        .nth(1)
        .unwrap_or(target)
        .split('/')
        .next()
        .unwrap_or("External")
        .replace("www.", "")
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn item_url(key: &str) -> String {
    let path = path_from_url(key).unwrap_or_else(|| key.to_string());
    url::join_url(BASE_URL, &path)
}

fn path_from_url(input: &str) -> Option<String> {
    if input.contains("/anime/") || input.contains("/episode/") || input.starts_with("/anime-") {
        let path = if let Some(index) = input.find("/anime/") {
            &input[index..]
        } else if let Some(index) = input.find("/episode/") {
            &input[index..]
        } else {
            input
        };
        Some(path.trim_end_matches('/').to_string())
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

fn pref_str<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .or_else(|| request.get("prefs"))
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
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

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn error(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.to_string(),
    }
}

export_video_source!(SOURCE);
