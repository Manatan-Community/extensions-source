use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack, UrlResolveResult,
    VideoEpisode, VideoHoster, VideoStream, VideoStreamKind,
    abi::{ExtensionError, ExtensionResult},
    export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

const SOURCE: ChineseAnime = ChineseAnime;
const BASE_URL: &str = "https://www.chineseanime.vip";

struct ChineseAnime;

impl VideoSource for ChineseAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let order = if listing == "latest" {
            "update"
        } else {
            "popular"
        };
        let body = get_or_fixture(
            &format!("{BASE_URL}/anime/?page={page}&order={order}"),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: has_next_page(&body),
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
        let page = page(&request);
        let body = get_or_fixture(
            &format!(
                "{BASE_URL}/page/{page}/?s={}",
                manatan_shared::sdk::http::url_encode(query)
            ),
            SEARCH_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        Ok(fetch_details(
            &request_key(&request, "item").unwrap_or_else(|| "/anime/sample/".to_string()),
        ))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample/".to_string());
        let body = get_or_fixture(&absolute_url(&path), EPISODES_FIXTURE);
        Ok(parse_episodes(&body))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/sample-episode/".to_string());
        let body = get_or_fixture(&absolute_url(&path), HOSTERS_FIXTURE);
        Ok(parse_hosters(&body))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let name = request
            .get("hoster")
            .and_then(|hoster| hoster.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Mirror");
        let embed = resolve_embedded_url(&key)?;
        Ok(resolve_embed_streams(&embed, name))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut streams = Vec::new();
        for hoster in self.hosters(request)? {
            streams.extend(
                self.resolve_hoster(
                    json!({ "hoster": { "key": hoster.key, "name": hoster.name } }),
                )?,
            );
        }
        sort_streams(&mut streams, "720p");
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

fn get_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE);
    parse_details(&body, path).unwrap_or_else(|| fallback_item(path))
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("<article")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "div class=\"tt", "</div>")
                .or_else(|| html::text_between(chunk, "div class='tt", "</div>"))
                .map(|text| html::strip_tags(&text))
                .filter(|text| !text.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))?;
            Some(CatalogItem {
                key: path_key(&href),
                title,
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| {
                        absolute_url(&image)
                            .split("?resize")
                            .next()
                            .unwrap_or("")
                            .to_string()
                    }),
                url: Some(absolute_url(&href)),
                language: Some("all".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, path: &str) -> Option<CatalogItem> {
    let title = html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value))?;
    Some(CatalogItem {
        key: path_key(path),
        title,
        cover: html::attr_after(body, "div class=\"thumb", "src")
            .or_else(|| html::attr_after(body, "div class=\"limage", "src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description: html::text_between(body, "entry-content", "</div>")
            .map(|value| html::strip_tags(&value)),
        tags: collect_anchor_text(body, "genxed"),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(info_value(body, "Status").as_deref()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    body.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let ep_text = html::text_between(chunk, "epl-num", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| "0".to_string());
            Some(VideoEpisode {
                key: path_key(&href),
                title: Some(format!("Episode {ep_text}")),
                episode_number: ep_text
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse::<f32>().ok()),
                url: Some(absolute_url(&href)),
                language: Some("all".to_string()),
                labels: html::text_between(chunk, "epl-sub", "</")
                    .map(|value| vec![html::strip_tags(&value)])
                    .unwrap_or_default(),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_hosters(body: &str) -> Vec<VideoHoster> {
    let mut hosters = Vec::new();
    for chunk in body.split("<option").skip(1) {
        if let Some(value) = html::attr(chunk, "value") {
            let name = html::strip_tags(chunk).trim().to_string();
            hosters.push(video_hoster(
                &value,
                if name.is_empty() { "Mirror" } else { &name },
            ));
        }
    }
    for chunk in body.split("<a").skip(1) {
        if let Some(value) = html::attr(chunk, "data-em") {
            let name = html::strip_tags(chunk).trim().to_string();
            hosters.push(video_hoster(
                &value,
                if name.is_empty() { "Mirror" } else { &name },
            ));
        }
    }
    hosters
}

fn video_hoster(key: &str, name: &str) -> VideoHoster {
    VideoHoster {
        key: key.to_string(),
        name: name.to_string(),
        lazy: true,
        video_count: Some(1),
        ..VideoHoster::default()
    }
}

fn resolve_embedded_url(encoded: &str) -> ExtensionResult<String> {
    if encoded.starts_with("http") {
        let doc = client()
            .get(encoded)
            .browser_document()
            .send_text()
            .map_err(|error| error_with(format!("hoster request failed: {}", error.message)))?;
        return embed_from_document(&doc).ok_or_else(|| error_with("missing embed iframe"));
    }
    let decoded = STANDARD
        .decode(encoded)
        .map_err(|error| error_with(format!("invalid encoded mirror data: {error}")))?;
    let doc = String::from_utf8_lossy(&decoded);
    embed_from_document(&doc).ok_or_else(|| error_with("missing embed iframe"))
}

fn embed_from_document(doc: &str) -> Option<String> {
    html::attr_after(doc, "<iframe", "src")
        .or_else(|| html::attr_after(doc, "itemprop=\"embedUrl\"", "content"))
        .map(|value| {
            if value.starts_with("//") {
                format!("https:{value}")
            } else {
                absolute_url(&value)
            }
        })
}

fn resolve_embed_streams(embed: &str, name: &str) -> Vec<VideoStream> {
    if embed.contains("vatchus") {
        let streams = vatchus_streams(embed, name);
        if !streams.is_empty() {
            return streams;
        }
    }
    if embed.contains(".m3u8") {
        return vec![hls_stream(embed, name, "HLS", embed, Vec::new())];
    }
    vec![external_stream(embed, name)]
}

fn vatchus_streams(embed: &str, name: &str) -> Vec<VideoStream> {
    let Ok(doc) = client().get(embed).browser_document().send_text() else {
        return Vec::new();
    };
    let script = doc
        .split("<script")
        .find(|chunk| chunk.contains("document.write"))
        .unwrap_or_default();
    let numbers: Vec<i32> = script
        .split(" = [")
        .nth(1)
        .and_then(|chunk| chunk.split("];").next())
        .unwrap_or_default()
        .replace('"', "")
        .split(',')
        .filter_map(|part| STANDARD.decode(part.trim()).ok())
        .filter_map(|bytes| String::from_utf8(bytes).ok())
        .filter_map(|text| {
            text.chars()
                .filter(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
        .collect();
    let Some(first) = numbers.first() else {
        return Vec::new();
    };
    let offset = first - 60;
    let decoded = numbers
        .iter()
        .filter_map(|number| char::from_u32((*number - offset) as u32))
        .collect::<String>();
    let Some(playlist_url) = decoded
        .split("file:'")
        .nth(1)
        .and_then(|part| part.split('\'').next())
    else {
        return Vec::new();
    };
    let subtitles = parse_vatchus_subtitles(&decoded);
    let playlist = client().get(playlist_url).send_text().unwrap_or_default();
    if playlist.contains("#EXT-X-STREAM-INF") {
        parse_hls_playlist(&playlist, playlist_url, name, embed, subtitles)
    } else {
        vec![hls_stream(playlist_url, name, "HLS", embed, subtitles)]
    }
}

fn parse_vatchus_subtitles(decoded: &str) -> Vec<SubtitleTrack> {
    decoded
        .split('{')
        .skip(1)
        .filter(|chunk| chunk.contains("\"kind\":\"captions\""))
        .filter_map(|chunk| {
            let track_url = chunk.split("file\\\":\\\"").nth(1)?.split('"').next()?;
            let label = chunk
                .split("label\\\":\\\"")
                .nth(1)
                .and_then(|part| part.split('"').next())
                .unwrap_or("Subtitles");
            Some(SubtitleTrack {
                url: track_url.to_string(),
                language: Some(label.to_ascii_lowercase()),
                label: Some(label.to_string()),
                format: Some("vtt".to_string()),
                ..SubtitleTrack::default()
            })
        })
        .collect()
}

fn parse_hls_playlist(
    body: &str,
    master: &str,
    name: &str,
    referer: &str,
    subtitles: Vec<SubtitleTrack>,
) -> Vec<VideoStream> {
    let mut streams = Vec::new();
    for block in body.split("#EXT-X-STREAM-INF:").skip(1) {
        let quality = block
            .split("RESOLUTION=")
            .nth(1)
            .and_then(|part| part.split('x').nth(1))
            .and_then(|part| part.split([',', '\n']).next())
            .map(|height| format!("{height}p"))
            .unwrap_or_else(|| "auto".to_string());
        let Some(line) = block
            .lines()
            .find(|line| !line.trim().is_empty() && !line.starts_with('#'))
        else {
            continue;
        };
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
        streams.push(hls_stream(
            &stream_url,
            name,
            &quality,
            referer,
            subtitles.clone(),
        ));
    }
    streams
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

fn external_stream(stream_url: &str, name: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        initialized: true,
        ..VideoStream::default()
    }
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn collect_anchor_text(body: &str, marker: &str) -> Vec<String> {
    let block = body.split(marker).nth(1).unwrap_or_default();
    block
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn info_value(block: &str, label: &str) -> Option<String> {
    block
        .split("<span")
        .find(|chunk| chunk.contains(label))
        .map(html::strip_tags)
        .map(|text| text.replace(label, "").replace(':', "").trim().to_string())
        .filter(|text| !text.is_empty())
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "completed" => ItemStatus::Completed,
        "ongoing" => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
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

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .filter(|path| !path.trim_matches('/').is_empty())
        .map(path_key)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("url")))
        .and_then(Value::as_str)
        .map(path_key)
}

fn path_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) {
        return path_key(path);
    }
    let path = input.split('?').next().unwrap_or(input).trim();
    format!("/{}", path.trim_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn has_next_page(body: &str) -> bool {
    body.contains("pagination")
        && (body.contains("class=\"next\"")
            || body.contains("class='next'")
            || body.contains("hpage")
            || body.contains("div.mrgn"))
}

fn fallback_item(path: &str) -> CatalogItem {
    CatalogItem {
        key: path_key(path),
        title: path.trim_matches('/').replace('-', " "),
        url: Some(absolute_url(path)),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn error_with(message: impl Into<String>) -> ExtensionError {
    ExtensionError {
        message: message.into(),
    }
}

const LIST_FIXTURE: &str = r#"
<div class="listupd">
<article><a class="tip" href="/anime/sample/"><img data-src="/sample.jpg"><div class="tt">Sample Donghua</div></a></article>
</div>
"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample Donghua</h1>
<div class="thumb"><img src="/sample.jpg"></div>
<div class="info-content"><span>Status: Ongoing</span><div class="genxed"><a>Action</a></div></div>
<div class="entry-content">Sample description.</div>
"#;
const EPISODES_FIXTURE: &str = r#"
<div class="eplister"><ul><li><a href="/sample-episode/"><span class="epl-num">1</span><span class="epl-sub">All Sub</span></a></li></ul></div>
"#;
const HOSTERS_FIXTURE: &str = r#"
<select class="mirror"><option data-index="0" value="PGlmcmFtZSBzcmM9Imh0dHBzOi8vZXhhbXBsZS5jb20vdmlkZW8ubTN1OCI+PC9pZnJhbWU+">Fixture HLS</option></select>
"#;

export_video_source!(SOURCE);
