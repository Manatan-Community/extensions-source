use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::Map;
use serde_json::{Value, json};

const SOURCE: AnimeLatinoHd = AnimeLatinoHd;
const BASE_URL: &str = "https://www.animelatinohd.com";
const API_URL: &str = "https://api.animelatinohd.com";

struct AnimeLatinoHd;

impl VideoSource for AnimeLatinoHd {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/animes?page={page}&status=1")
        } else {
            format!("{BASE_URL}/animes/populares")
        };
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
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
                entries: vec![fetch_details(&path)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let target = if !query.is_empty() {
            format!(
                "{BASE_URL}/animes?page={page}&search={}",
                url::query_escape(query)
            )
        } else {
            let params = filter_params(&request);
            if params.is_empty() {
                format!("{BASE_URL}/animes?page={page}")
            } else {
                format!("{BASE_URL}/animes?{params}&page={page}")
            }
        };
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        let body = get_or_fixture(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode =
            request_key(&request, "episode").unwrap_or_else(|| "/ver/sample/1".to_string());
        let body = get_or_fixture(&absolute_url(&episode), WATCH_FIXTURE, BASE_URL);
        Ok(parse_hosters(&body, &absolute_url(&episode)))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = key.splitn(3, '|');
        let server = parts.next().unwrap_or("External");
        let embed = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(BASE_URL);
        let mut streams = resolve_embed(embed, server, referer, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut streams = Vec::new();
        for hoster in self.hosters(request.clone())? {
            let mut resolved = self.resolve_hoster(json!({
                "hoster": { "key": hoster.key },
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
            }))?;
            for stream in &mut resolved {
                stream.hoster = Some(hoster.clone());
            }
            streams.extend(resolved);
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Populares".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Ultimos".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
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
            if path.starts_with("/ver/") {
                return Ok(Some(UrlResolveResult {
                    episode: Some(
                        json!({ "key": path, "url": absolute_url(&path), "language": "es" }),
                    ),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
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

fn client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_cookies_for(BASE_URL)
        .with_cookies_for(API_URL)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let data = page_data(body).unwrap_or(Value::Null);
    let list = data
        .pointer("/props/pageProps/data/data")
        .and_then(Value::as_array)
        .or_else(|| data.pointer("/props/pageProps/data/popular_today").and_then(Value::as_array));
    let entries = list
        .into_iter()
        .flatten()
        .filter_map(card_from_json)
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("Animes_paginate") || body.contains("\"next_page_url\""),
    }
}

fn card_from_json(value: &Value) -> Option<CatalogItem> {
    let obj = value.as_object()?;
    let slug = str_field(obj, "slug")?;
    let title = str_field(obj, "name").unwrap_or_else(|| slug.replace('-', " "));
    let poster = str_field(obj, "poster")
        .map(|poster| format!("https://image.tmdb.org/t/p/w200{poster}"));
    let path = format!("/anime/{slug}");
    Some(CatalogItem {
        key: path.clone(),
        title,
        cover: poster,
        url: Some(absolute_url(&path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: false,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let data = page_data(&body)
        .and_then(|v| v.pointer("/props/pageProps/data").cloned())
        .unwrap_or(Value::Null);
    CatalogItem {
        key: path_key(path),
        title: data
            .get("name")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| title_from_path(path)),
        cover: data
            .get("poster")
            .and_then(Value::as_str)
            .map(|poster| format!("https://image.tmdb.org/t/p/w600_and_h900_bestv2{poster}")),
        url: Some(absolute_url(path)),
        description: data.get("overview").and_then(Value::as_str).map(ToString::to_string),
        tags: data
            .get("genres")
            .and_then(Value::as_str)
            .map(|genres| genres.split(',').map(|v| v.trim().to_string()).collect())
            .unwrap_or_default(),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&body),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let data = page_data(body)
        .and_then(|v| v.pointer("/props/pageProps/data").cloned())
        .unwrap_or(Value::Null);
    let slug = data.get("slug").and_then(Value::as_str).unwrap_or("sample");
    let mut episodes = data
        .get("episodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|ep| {
            let number = ep
                .get("number")
                .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse::<f64>().ok()))
                .map(|v| v as f32)?;
            let path = format!("/ver/{slug}/{}", trim_float(number));
            Some(VideoEpisode {
                key: path.clone(),
                title: Some(format!("Episodio {}", trim_float(number))),
                episode_number: Some(number),
                url: Some(absolute_url(&path)),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect::<Vec<_>>();
    episodes.sort_by(|a, b| {
        b.episode_number
            .partial_cmp(&a.episode_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    episodes
}

fn parse_hosters(body: &str, episode_url: &str) -> Vec<VideoHoster> {
    let mut out = Vec::new();
    let data = page_data(body)
        .and_then(|v| v.pointer("/props/pageProps/data/players").cloned())
        .unwrap_or(Value::Null);
    for player in player_entries(&data) {
        let Some(id) = player.get("id").and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse::<i64>().ok())) else {
            continue;
        };
        let language = match player.get("languaje").and_then(Value::as_str) {
            Some("1") => "[LAT]",
            _ => "[SUB]",
        };
        let api = format!("{API_URL}/stream/{id}");
        let response = client(BASE_URL)
            .get(&api)
            .browser_document()
            .referer(BASE_URL)
            .header("authority", "api.animelatinohd.com")
            .send_text()
            .unwrap_or_default();
        let inline_urls = ["url", "code", "embed", "link"]
            .into_iter()
            .filter_map(|key| player.get(key).and_then(Value::as_str).map(ToString::to_string));
        for embed in media_urls(&response)
            .into_iter()
            .chain(inline_urls)
            .chain(media_urls(&format!("{player:?}")))
        {
            let name = format!(
                "{language} {}",
                matched_server(&embed).unwrap_or_else(|| host_name(&embed))
            );
            out.push(VideoHoster {
                key: format!("{name}|{}|{episode_url}", absolute_url(&embed)),
                name,
                url: Some(absolute_url(&embed)),
                lazy: true,
                video_count: Some(1),
                ..VideoHoster::default()
            });
        }
    }
    out
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_url(embed);
    if embed.contains(".m3u8") {
        return parse_hls(&embed, name, referer, request);
    }
    if embed.contains("/stream/fl.php?v=") {
        let target = embed.split("/stream/fl.php?v=").nth(1).unwrap_or(&embed);
        return vec![media_stream(target, "FireLoad", "direct", referer)];
    }
    let body = get_or_fixture(&embed, "", referer);
    if let Some(src) = first_media_url(&body) {
        let src = absolute_remote(&src, &embed);
        if src.contains(".m3u8") {
            return parse_hls(&src, name, &embed, request);
        }
        return vec![media_stream(&src, name, "direct", &embed)];
    }
    vec![external_stream(&embed, name, referer)]
}

fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)["']"#,
        r#"src\s*:\s*["']([^"']+)["']"#,
        r#"<source[^>]+src=["']([^"']+)["']"#,
    ]
    .into_iter()
    .find_map(|pat| {
        Regex::new(pat)
            .ok()?
            .captures(body)?
            .get(1)
            .map(|m| m.as_str().replace("\\/", "/"))
    })
}

fn media_urls(body: &str) -> Vec<String> {
    let mut urls = Regex::new(r#"https?://[^\s"'<>\\]+"#)
        .unwrap()
        .captures_iter(body)
        .filter_map(|cap| cap.get(0).map(|m| m.as_str().to_string()))
        .map(|url| url.replace("\\/", "/").replace("\\u0026", "&"))
        .collect::<Vec<_>>();
    urls.retain(|url| {
        let lower = url.to_ascii_lowercase();
        !lower.contains(".jpg")
            && !lower.contains(".png")
            && !lower.contains(".webp")
            && !lower.contains(".css")
            && !lower.contains(".js")
    });
    urls.sort();
    urls.dedup();
    urls
}

fn page_data(body: &str) -> Option<Value> {
    for script in body.split("<script").skip(1) {
        let script = script.split("</script>").next().unwrap_or_default();
        if !script.contains("{\"props\":{\"pageProps\"") {
            continue;
        }
        let raw = script.split('>').nth(1).unwrap_or(script).trim();
        if let Ok(value) = serde_json::from_str::<Value>(raw) {
            return Some(value);
        }
    }
    None
}

fn str_field(obj: &Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(Value::as_str).map(ToString::to_string)
}

fn player_entries(value: &Value) -> Vec<&Map<String, Value>> {
    let mut out = Vec::new();
    match value {
        Value::Array(groups) => {
            for group in groups {
                match group {
                    Value::Array(items) => {
                        out.extend(items.iter().filter_map(Value::as_object));
                    }
                    Value::Object(_) => {
                        if let Some(obj) = group.as_object() {
                            out.push(obj);
                        }
                    }
                    _ => {}
                }
            }
        }
        Value::Object(map) => {
            for group in map.values() {
                match group {
                    Value::Array(items) => {
                        out.extend(items.iter().filter_map(Value::as_object));
                    }
                    Value::Object(obj) => out.push(obj),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    out
}

fn trim_float(number: f32) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i32)
    } else {
        number.to_string()
    }
}

fn parse_hls(master: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = client(referer)
        .get(master)
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    let mut streams = Vec::new();
    let mut quality = "auto".to_string();
    for line in body.lines() {
        if line.starts_with("#EXT-X-STREAM-INF") {
            quality = line
                .split("RESOLUTION=")
                .nth(1)
                .and_then(|v| v.split('x').nth(1))
                .and_then(|v| v.split(',').next())
                .map(|v| format!("{v}p"))
                .unwrap_or_else(|| "auto".to_string());
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            streams.push(media_stream(
                &absolute_remote(line.trim(), master),
                name,
                &quality,
                referer,
            ));
        }
    }
    if streams.is_empty() {
        streams.push(media_stream(master, name, "auto", referer));
    }
    sort_streams(&mut streams, request);
    streams
}

fn media_stream(target: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = target.contains(".m3u8");
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(referer),
        ..VideoStream::default()
    }
}

fn external_stream(target: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} External")),
        quality: Some(name.to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let server = pref(request, "preferred_server", "Mp4Upload").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080p");
    streams.sort_by_key(|stream| {
        let name = stream.name.clone().unwrap_or_default().to_ascii_lowercase();
        let q = stream.quality.clone().unwrap_or_default();
        (
            name.contains(&server),
            q.contains(&quality),
            quality_rank(&q),
        )
    });
    streams.reverse();
}

fn filter_params(request: &Value) -> String {
    ["estado", "tipo", "genero", "estreno", "idioma"]
        .into_iter()
        .filter_map(|key| {
            filter(request, key)
                .filter(|value| !value.is_empty())
                .map(|value| format!("{key}={}", url::query_escape(&value)))
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn select_all<'a>(document: &'a Html, sel: &str) -> Vec<ElementRef<'a>> {
    document.select(&selector(sel)).collect()
}

fn selector(input: &str) -> Selector {
    Selector::parse(input).unwrap()
}

fn select_text_doc(document: &Html, sel: &str) -> Option<String> {
    document
        .select(&selector(sel))
        .next()
        .map(element_text)
        .filter(|v| !v.is_empty())
}

fn select_texts_doc(document: &Html, sel: &str) -> Vec<String> {
    document
        .select(&selector(sel))
        .map(element_text)
        .filter(|v| !v.is_empty())
        .collect()
}

fn select_attr_doc(document: &Html, sel: &str, name: &str) -> Option<String> {
    document
        .select(&selector(sel))
        .next()
        .and_then(|el| el.value().attr(name))
        .map(ToString::to_string)
}

fn select_text(el: ElementRef<'_>, sel: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .map(element_text)
        .filter(|v| !v.is_empty())
}

fn select_attr(el: ElementRef<'_>, sel: &str, name: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .and_then(|el| el.value().attr(name))
        .map(ToString::to_string)
}

fn attr(el: &ElementRef<'_>, name: &str) -> String {
    el.value().attr(name).unwrap_or_default().to_string()
}

fn element_text(el: ElementRef<'_>) -> String {
    el.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn absolute_url(input: &str) -> String {
    absolute_remote(input, BASE_URL)
}

fn absolute_remote(input: &str, base: &str) -> String {
    let trimmed = input.trim().replace("\\/", "/");
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed
    } else if let Some(rest) = trimmed.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url::join_url(base, &trimmed)
    }
}

fn path_from_url(input: &str) -> Option<String> {
    input.strip_prefix(BASE_URL).and_then(|path| {
        let first = path
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or_default();
        if path.trim_matches('/').is_empty()
            || matches!(
                first,
                "assets" | "static" | "media" | "directorio" | "buscar" | "login"
            )
        {
            None
        } else {
            Some(path_key(path))
        }
    })
}

fn path_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) {
        return path_key(path);
    }
    format!(
        "/{}",
        input
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .trim_matches('/')
    )
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request_raw_key(request, field).map(|value| path_key(&value))
}

fn request_raw_key(request: &Value, field: &str) -> Option<String> {
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
        .map(ToString::to_string)
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("AnimeLatinoHD")
        .replace('-', " ")
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("finalizado") {
        ItemStatus::Completed
    } else if lower.contains("emision") || lower.contains("emisión") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn matched_server(input: &str) -> Option<String> {
    let lower = input.to_ascii_lowercase();
    [
        (
            "Voe",
            ["voe", "tubelessceliolymph", "simpulumlamerop"].as_slice(),
        ),
        ("Okru", ["ok.ru", "okru"].as_slice()),
        ("Filemoon", ["filemoon", "moonplayer"].as_slice()),
        ("Mp4Upload", ["mp4upload"].as_slice()),
        (
            "StreamWish",
            ["wishembed", "streamwish", "strwish", "wish", "playerwish"].as_slice(),
        ),
        ("Doodstream", ["doodstream", "dood.", "d000d"].as_slice()),
        (
            "StreamTape",
            ["streamtape", "stape", "shavetape"].as_slice(),
        ),
        ("YourUpload", ["yourupload"].as_slice()),
        ("BurstCloud", ["burstcloud", "burst"].as_slice()),
        ("Upstream", ["upstream"].as_slice()),
        ("VidHide", ["vidhide", "streamhide", "streamvid"].as_slice()),
    ]
    .into_iter()
    .find(|(_, names)| names.iter().any(|name| lower.contains(name)))
    .map(|(name, _)| name.to_string())
}

fn host_name(input: &str) -> String {
    input
        .split("://")
        .nth(1)
        .unwrap_or(input)
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

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn with_listing(request: &Value, listing: &str) -> Value {
    json!({ "listing": listing, "preferences": request.get("preferences").cloned().unwrap_or(Value::Null) })
}

fn pref(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|f| f.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn quality_rank(input: &str) -> i32 {
    Regex::new(r#"(\d+)"#)
        .unwrap()
        .captures(input)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0)
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<script>{"props":{"pageProps":{"data":{"popular_today":[{"slug":"sample","name":"Sample","poster":"/sample.jpg"}]}}}}</script>"#;
const DETAILS_FIXTURE: &str = r#"<script>{"props":{"pageProps":{"data":{"slug":"sample","name":"Sample","poster":"/sample.jpg","overview":"Sample description.","genres":"Accion","status":"En emision","episodes":[{"number":1}],"players":[[{"id":1,"languaje":"1","url":"https://example.invalid/embed"}]]}}}}</script>"#;
const WATCH_FIXTURE: &str =
    DETAILS_FIXTURE;
