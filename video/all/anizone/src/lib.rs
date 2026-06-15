use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind,
    abi::{ExtensionError, ExtensionResult},
    export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: AniZone = AniZone;
const BASE_URL: &str = "https://anizone.to";

struct AniZone;

impl VideoSource for AniZone {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort = if listing == "latest" {
            "release-desc"
        } else {
            "title-asc"
        };
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let html = livewire_html("/anime", json!({ "sort": sort }), Vec::new(), page)?;
        Ok(Paged {
            entries: parse_list(&html),
            has_next_page: html.contains("x-intersect") && html.contains("loadMore"),
        })
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
        let mut updates = json!({ "sort": "title-asc" });
        if !query.is_empty() {
            updates["search"] = Value::String(query.to_string());
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let html = livewire_html("/anime", updates, Vec::new(), page)?;
        Ok(Paged {
            entries: parse_list(&html),
            has_next_page: html.contains("x-intersect") && html.contains("loadMore"),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        let mut html = livewire_html(&path, json!({ "sort": "release-desc" }), Vec::new(), 1)?;
        let mut out = parse_episodes(&html);
        let mut guard = 0;
        while html.contains("x-intersect") && html.contains("loadMore") && guard < 10 {
            guard += 1;
            html = livewire_html(
                &path,
                json!({}),
                vec![json!({"path":"","method":"loadMore","params":[]})],
                1,
            )?;
            let loaded = out.len();
            out.extend(parse_episodes(&html).into_iter().skip(loaded));
        }
        Ok(out)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/watch/sample".to_string());
        let body = get_or_fixture(&absolute_url(&path), STREAM_FIXTURE);
        let mut streams = parse_stream_page(&body, &path);
        let buttons = parse_server_buttons(&body);
        for (id, name) in buttons.into_iter().skip(1) {
            let html = livewire_html(
                &path,
                json!({}),
                vec![json!({"path":"","method":"setVideo","params":[id]})],
                1,
            )?;
            streams.extend(parse_stream_page_with_name(&html, &path, &name));
        }
        sort_streams(
            &mut streams,
            pref(&request, "preferred_quality")
                .as_deref()
                .unwrap_or("1080"),
        );
        Ok(streams)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "A-Z".to_string(),
                entries: self.list(json!({"listing":"popular","page":1}))?.entries,
                has_more: true,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: self.list(json!({"listing":"latest","page":1}))?.entries,
                has_more: true,
                ..HomeSection::default()
            },
        ])
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
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn livewire_html(
    initial_slug: &str,
    updates: Value,
    calls: Vec<Value>,
    page: u64,
) -> ExtensionResult<String> {
    let first = get_or_fixture(&absolute_url(initial_slug), DETAILS_FIXTURE);
    let mut snapshot = snapshot_from_doc(&first).unwrap_or_default();
    let token = html::attr_after(&first, "script", "data-csrf").unwrap_or_default();
    let mut html_out = first;
    let iterations = page.max(1);
    for index in 0..iterations {
        let active_calls = if index == 0 {
            calls.clone()
        } else {
            vec![json!({"path":"","method":"loadMore","params":[]})]
        };
        let payload = json!({
            "_token": token,
            "components": [{
                "calls": active_calls,
                "snapshot": snapshot,
                "updates": if index == 0 { updates.clone() } else { json!({}) }
            }]
        });
        let body = client()
            .post(format!("{BASE_URL}/livewire/update"))
            .json(payload.to_string())
            .header("X-Livewire", "")
            .xhr()
            .send_text()
            .unwrap_or_else(|_| LIVEWIRE_FIXTURE.to_string());
        let dto: LivewireDto = serde_json::from_str(&body)
            .map_err(|error| error_with(format!("invalid Livewire response: {error}")))?;
        let Some(component) = dto.components.into_iter().next() else {
            return Err(error_with("missing Livewire component"));
        };
        snapshot = component.snapshot.replace("\\\"", "\"");
        html_out = unescape_livewire_html(&component.effects.html);
    }
    Ok(html_out)
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
    let info = body.split("flex items-start").nth(1).unwrap_or(&body);
    CatalogItem {
        key: path_key(path),
        title: html::text_between(&body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_else(|| path.trim_matches('/').replace('-', " ")),
        cover: html::attr_after(&body, "<img", "src").map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description: html::text_between(&body, "Synopsis", "</div>")
            .map(|value| html::strip_tags(&value)),
        tags: info
            .split("<a")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .collect(),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&body),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_list(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("inline") && chunk.contains("href="))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(CatalogItem {
                key: path_key(&href),
                title,
                cover: preceding_or_following_img(body, chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&href)),
                language: Some("all".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    body.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title =
                html::text_between(chunk, "<h3", "</h3>").map(|value| html::strip_tags(&value));
            Some(VideoEpisode {
                key: path_key(&href),
                title,
                episode_number: html::text_between(chunk, "<h3", "</h3>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| {
                        value
                            .chars()
                            .filter(|ch| ch.is_ascii_digit() || *ch == '.')
                            .collect::<String>()
                            .parse()
                            .ok()
                    }),
                date_uploaded: None,
                url: Some(absolute_url(&href)),
                language: Some("all".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_stream_page(body: &str, referer_path: &str) -> Vec<VideoStream> {
    let name = parse_server_buttons(body)
        .first()
        .map(|(_, name)| name.clone())
        .unwrap_or_else(|| "Server".to_string());
    parse_stream_page_with_name(body, referer_path, &name)
}

fn parse_stream_page_with_name(body: &str, referer_path: &str, name: &str) -> Vec<VideoStream> {
    let Some(master) = html::attr_after(body, "<media-player", "src") else {
        return Vec::new();
    };
    let subtitles = body
        .split("<track")
        .skip(1)
        .filter(|chunk| chunk.contains("subtitles"))
        .filter_map(|chunk| {
            Some(SubtitleTrack {
                url: absolute_url(&html::attr(chunk, "src")?),
                label: html::attr(chunk, "label"),
                language: None,
                format: Some("vtt".to_string()),
                ..SubtitleTrack::default()
            })
        })
        .collect::<Vec<_>>();
    let playlist = client().get(&master).send_text().unwrap_or_default();
    if playlist.contains("#EXT-X-STREAM-INF") {
        parse_hls_playlist(
            &playlist,
            &master,
            name,
            &absolute_url(referer_path),
            subtitles,
        )
    } else {
        vec![hls_stream(
            &master,
            name,
            "HLS",
            &absolute_url(referer_path),
            subtitles,
        )]
    }
}

fn parse_hls_playlist(
    body: &str,
    master: &str,
    name: &str,
    referer: &str,
    subtitles: Vec<SubtitleTrack>,
) -> Vec<VideoStream> {
    body.split("#EXT-X-STREAM-INF:")
        .skip(1)
        .filter_map(|block| {
            let quality = block
                .split("RESOLUTION=")
                .nth(1)
                .and_then(|part| part.split('x').nth(1))
                .and_then(|part| part.split([',', '\n']).next())
                .map(|height| format!("{height}p"))
                .unwrap_or_else(|| "auto".to_string());
            let line = block
                .lines()
                .find(|line| !line.trim().is_empty() && !line.starts_with('#'))?;
            let stream_url = if line.starts_with("http") {
                line.to_string()
            } else {
                format!(
                    "{}/{}",
                    master
                        .rsplit_once('/')
                        .map(|(base, _)| base)
                        .unwrap_or(master),
                    line
                )
            };
            Some(hls_stream(
                &stream_url,
                name,
                &quality,
                referer,
                subtitles.clone(),
            ))
        })
        .collect()
}

fn hls_stream(
    stream_url: &str,
    name: &str,
    quality: &str,
    referer: &str,
    subtitles: Vec<SubtitleTrack>,
) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(format!("{name} - {quality}")),
        quality: Some(quality.to_string()),
        format: Some("hls".to_string()),
        is_hls: true,
        stream_kind: Some(VideoStreamKind::Hls),
        headers: referer_headers(referer),
        subtitles,
        ..VideoStream::default()
    }
}

fn parse_server_buttons(body: &str) -> Vec<(u64, String)> {
    body.split("<button")
        .skip(1)
        .filter(|chunk| chunk.contains("setVideo"))
        .map(|chunk| {
            let id = chunk
                .split("setVideo('")
                .nth(1)
                .and_then(|part| part.split('\'').next())
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            (id, html::strip_tags(chunk))
        })
        .collect()
}

fn snapshot_from_doc(body: &str) -> Option<String> {
    html::attr_after(body, "wire:snapshot", "wire:snapshot").or_else(|| {
        body.split("wire:snapshot=")
            .nth(1)
            .and_then(|part| {
                part.trim_start_matches(['"', '\''])
                    .split(['"', '\''])
                    .next()
            })
            .map(|value| value.replace("&quot;", "\""))
    })
}

fn unescape_livewire_html(input: &str) -> String {
    input
        .replace("\\\"", "\"")
        .replace("\\n", "")
        .replace("\\/", "/")
}

fn preceding_or_following_img(body: &str, chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "src").or_else(|| {
        let index = body.find(chunk)?;
        let start = index.saturating_sub(300);
        html::attr_after(&body[start..index], "<img", "src")
    })
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn sort_streams(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by_key(|stream| {
        stream
            .quality
            .as_deref()
            .unwrap_or_default()
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse::<i32>()
            .unwrap_or(0)
    });
    streams.reverse();
    for stream in streams {
        stream.preferred = stream
            .quality
            .as_deref()
            .map(|q| q.contains(preferred))
            .unwrap_or(false);
    }
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

fn pref(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
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

fn error_with(message: impl Into<String>) -> ExtensionError {
    ExtensionError {
        message: message.into(),
    }
}

#[derive(Deserialize)]
struct LivewireDto {
    components: Vec<ComponentDto>,
}

#[derive(Deserialize)]
struct ComponentDto {
    snapshot: String,
    effects: EffectsDto,
}

#[derive(Deserialize)]
struct EffectsDto {
    html: String,
}

const DETAILS_FIXTURE: &str = r#"
<main><div wire:snapshot="{&quot;data&quot;:{},&quot;memo&quot;:{},&quot;checksum&quot;:&quot;x&quot;}"></div></main>
<script data-csrf="fixture"></script>
<div class="grid"><div><img src="/sample.jpg"><a class="inline" href="/anime/sample">Sample Anime</a></div></div>
<h1>Sample Anime</h1><div>Synopsis</div><span class="flex">Ongoing</span>
"#;
const STREAM_FIXTURE: &str = r#"
<media-player src="https://example.com/master.m3u8"></media-player>
<button wire:click="setVideo('0')">Fixture</button>
"#;
const LIVEWIRE_FIXTURE: &str = r#"{"components":[{"snapshot":"{}","effects":{"html":"<div class=\"grid\"><div><img src=\"/sample.jpg\"><a class=\"inline\" href=\"/anime/sample\">Sample Anime</a></div></div>"}}]}"#;

export_video_source!(SOURCE);
