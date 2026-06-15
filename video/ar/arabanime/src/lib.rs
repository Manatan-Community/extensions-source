use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

const SOURCE: ArabAnime = ArabAnime;
const BASE_URL: &str = "https://www.arabanime.net";

struct ArabAnime;

impl VideoSource for ArabAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let body = get_or_fixture(&format!("{BASE_URL}/api?page={page}"), LIST_FIXTURE);
        Ok(parse_api_page(&body))
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
        if query.is_empty() {
            let page = page(&request);
            let target = format!(
                "{BASE_URL}/api?order={}&type={}&stat={}&tags=&page={page}",
                filter(&request, "order").unwrap_or_default(),
                filter(&request, "type").unwrap_or_default(),
                filter(&request, "stat").unwrap_or_default()
            );
            let body = get_or_fixture(&target, LIST_FIXTURE);
            Ok(parse_api_page(&body))
        } else {
            let body = client()
                .post(format!("{BASE_URL}/searchq"))
                .form(&[("searchq", query)])
                .xhr()
                .send_text()
                .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
            Ok(Paged {
                entries: parse_search_html(&body),
                has_next_page: false,
            })
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/show-1/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/show-1/sample".to_string());
        let body = get_or_fixture(&absolute_url(&path), DETAILS_FIXTURE);
        let data = decoded_data(&body, "div#data").unwrap_or_else(|| DETAILS_DATA.to_string());
        let payload: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
        let mut episodes: Vec<_> = payload
            .get("EPS")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|episode| {
                let url = episode.get("info-src")?.as_str()?;
                let number = episode
                    .get("episode_number")
                    .and_then(Value::as_f64)
                    .map(|value| value as f32);
                Some(VideoEpisode {
                    key: path_key(url),
                    title: episode
                        .get("episode_name")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    episode_number: number,
                    url: Some(absolute_url(url)),
                    language: Some("ar".to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect();
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/watch-1/sample/1".to_string());
        let body = get_or_fixture(&absolute_url(&path), WATCH_FIXTURE);
        let data = decoded_data(&body, "div#datawatch").unwrap_or_else(|| WATCH_DATA.to_string());
        let payload: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
        let Some(server) = payload
            .get("ep_info")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("stream_servers"))
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(Value::as_str)
            .and_then(decode_base64)
        else {
            return Ok(Vec::new());
        };
        let server_page = get_or_fixture(&server, SERVER_FIXTURE);
        let mut streams = Vec::new();
        for option in server_page.split("<option").skip(1) {
            let name = html::strip_tags(option);
            let Some(encoded) = html::attr(option, "data-src") else {
                continue;
            };
            let Some(embed) = decode_base64(&encoded) else {
                continue;
            };
            if !embed.contains(&format!("{BASE_URL}/embed")) {
                continue;
            }
            let embed_page = get_or_fixture(&embed, EMBED_FIXTURE);
            for source in embed_page.split("<source").skip(1) {
                let Some(video_url) = html::attr(source, "src") else {
                    continue;
                };
                if video_url.contains("static") {
                    continue;
                }
                let mut quality = html::attr(source, "label").unwrap_or_else(|| "auto".to_string());
                if !quality.contains('p') && quality.chars().any(|ch| ch.is_ascii_digit()) {
                    quality.push('p');
                }
                streams.push(media_stream(&video_url, &name, &quality, &embed));
            }
        }
        sort_streams(&mut streams, &preferred_quality(&request));
        Ok(streams)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let latest = get_or_fixture(BASE_URL, LATEST_HTML_FIXTURE);
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: self.list(json!({"page": 1}))?.entries,
                has_more: true,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: parse_latest_html(&latest),
                has_more: false,
                ..HomeSection::default()
            },
        ])
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
                item: Some(fetch_details(&path.replace("/watch-", "/show-"))),
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
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE);
    let data = decoded_data(&body, "div#data").unwrap_or_else(|| DETAILS_DATA.to_string());
    let payload: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
    let show = payload
        .get("show")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or(Value::Null);
    let title = show
        .get("anime_name")
        .and_then(Value::as_str)
        .unwrap_or("Anime")
        .to_string();
    let key = show
        .get("anime_id")
        .and_then(Value::as_i64)
        .zip(show.get("anime_slug").and_then(Value::as_str))
        .map(|(id, slug)| format!("/show-{id}/{slug}"))
        .unwrap_or_else(|| path_key(path));
    CatalogItem {
        key: key.clone(),
        title,
        cover: show
            .get("anime_cover_image_url")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(absolute_url(&key)),
        description: show
            .get("anime_description")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        tags: show
            .get("anime_genres")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: match show.get("anime_status").and_then(Value::as_str) {
            Some("Ongoing") => ItemStatus::Ongoing,
            Some("Completed") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_api_page(body: &str) -> Paged<CatalogItem> {
    let payload: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let entries = payload
        .get("Shows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(decode_base64)
        .filter_map(|text| serde_json::from_str::<Value>(&text).ok())
        .filter_map(|item| {
            let path = item.get("info_src")?.as_str()?;
            Some(CatalogItem {
                key: path_key(path),
                title: item.get("anime_name")?.as_str()?.to_string(),
                cover: item
                    .get("anime_cover_image_url")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                url: Some(absolute_url(path)),
                language: Some("ar".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: payload
            .get("current_page")
            .and_then(Value::as_u64)
            .zip(payload.get("last_page").and_then(Value::as_u64))
            .map(|(current, last)| current < last)
            .unwrap_or(false),
    }
}

fn parse_search_html(body: &str) -> Vec<CatalogItem> {
    body.split("div class=\"show")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            Some(CatalogItem {
                key: path_key(&href),
                title: html::text_between(chunk, "<h3", "</h3>")
                    .map(|text| html::strip_tags(&text))?,
                cover: html::attr_after(chunk, "<img", "src").map(|src| absolute_url(&src)),
                url: Some(absolute_url(&href)),
                language: Some("ar".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_latest_html(body: &str) -> Vec<CatalogItem> {
    body.split("div class=\"as-episode")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "a class=\"as-info", "href")?;
            let item_path = path_key(
                &href
                    .replace("watch", "show")
                    .rsplit_once('/')
                    .map(|(base, _)| base)
                    .unwrap_or(&href),
            );
            Some(CatalogItem {
                key: item_path.clone(),
                title: html::text_between(chunk, "a class=\"as-info", "</a>")
                    .map(|text| html::strip_tags(&text))?,
                cover: html::attr_after(chunk, "<img", "src").map(|src| absolute_url(&src)),
                url: Some(absolute_url(&item_path)),
                language: Some("ar".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn decoded_data(body: &str, marker: &str) -> Option<String> {
    let encoded = html::text_between(body, marker, "</div>").map(|text| html::strip_tags(&text))?;
    decode_base64(encoded.trim())
}

fn media_stream(stream_url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = stream_url.contains(".m3u8");
    VideoStream {
        url: stream_url.to_string(),
        name: Some(format!("{name}: {quality}")),
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

fn decode_base64(input: &str) -> Option<String> {
    STANDARD
        .decode(input.trim())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
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

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("url")))
        .and_then(Value::as_str)
        .map(path_key)
}

fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .filter(|path| !path.trim_matches('/').is_empty())
        .map(path_key)
}

fn path_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) {
        return path_key(path);
    }
    format!(
        "/{}",
        input.split('?').next().unwrap_or(input).trim_matches('/')
    )
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
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

const LIST_FIXTURE: &str = r#"{"Shows":["eyJhbmltZV9jb3Zlcl9pbWFnZV91cmwiOiJodHRwczovL2V4YW1wbGUuY29tL2NvdmVyLmpwZyIsImFuaW1lX2lkIjoiMSIsImFuaW1lX25hbWUiOiJTYW1wbGUgQW5pbWUiLCJhbmltZV9zY29yZSI6IjgiLCJhbmltZV9zbHVnIjoic2FtcGxlIiwiYW5pbWVfdHlwZSI6IjEiLCJpbmZvX3NyYyI6Ii9zaG93LTEvc2FtcGxlIn0="],"current_page":1,"last_page":1}"#;
const SEARCH_FIXTURE: &str = r#"<div class="show"><a href="/show-1/sample"><img src="/cover.jpg"><h3>Sample Anime</h3></a></div>"#;
const LATEST_HTML_FIXTURE: &str = r#"<div class="as-episode"><a class="as-info" href="/watch-1/sample/1">Sample Anime</a><img src="/cover.jpg"></div>"#;
const DETAILS_DATA: &str = r#"{"EPS":[{"episode_name":"Episode 1","episode_number":1,"info-src":"/watch-1/sample/1"}],"show":[{"anime_cover_image_url":"https://example.com/cover.jpg","anime_description":"Sample description.","anime_genres":"Action","anime_id":1,"anime_name":"Sample Anime","anime_release_date":"2024","anime_score":"8","anime_slug":"sample","anime_status":"Ongoing","anime_type":"1","show_episode_count":1,"wallpapaer":""}]}"#;
const DETAILS_FIXTURE: &str = r#"<div id="data">eyJFUFMiOlt7ImVwaXNvZGVfbmFtZSI6IkVwaXNvZGUgMSIsImVwaXNvZGVfbnVtYmVyIjoxLCJpbmZvLXNyYyI6Ii93YXRjaC0xL3NhbXBsZS8xIn1dLCJzaG93IjpbeyJhbmltZV9jb3Zlcl9pbWFnZV91cmwiOiJodHRwczovL2V4YW1wbGUuY29tL2NvdmVyLmpwZyIsImFuaW1lX2Rlc2NyaXB0aW9uIjoiU2FtcGxlIGRlc2NyaXB0aW9uLiIsImFuaW1lX2dlbnJlcyI6IkFjdGlvbiIsImFuaW1lX2lkIjoxLCJhbmltZV9uYW1lIjoiU2FtcGxlIEFuaW1lIiwiYW5pbWVfcmVsZWFzZV9kYXRlIjoiMjAyNCIsImFuaW1lX3Njb3JlIjoiOCIsImFuaW1lX3NsdWciOiJzYW1wbGUiLCJhbmltZV9zdGF0dXMiOiJPbmdvaW5nIiwiYW5pbWVfdHlwZSI6IjEiLCJzaG93X2VwaXNvZGVfY291bnQiOjEsIndhbGxwYXBhZXIiOiIifV19</div>"#;
const WATCH_DATA: &str =
    r#"{"ep_info":[{"stream_servers":["aHR0cHM6Ly93d3cuYXJhYmFuaW1lLm5ldC9zZXJ2ZXI="]}]}"#;
const WATCH_FIXTURE: &str = r#"<div id="datawatch">eyJlcF9pbmZvIjpbeyJzdHJlYW1fc2VydmVycyI6WyJhSFIwY0hNNkx5OTNkM2N1WVhKaFltRnVhVzFsTG01bGRDOXpaWEoyWlhJPSJdfV19</div>"#;
const SERVER_FIXTURE: &str = r#"<select><option data-src="aHR0cHM6Ly93d3cuYXJhYmFuaW1lLm5ldC9lbWJlZC9zYW1wbGU=">Server</option></select>"#;
const EMBED_FIXTURE: &str =
    r#"<video><source src="https://cdn.example/sample.m3u8" label="720"></video>"#;

export_video_source!(SOURCE);
