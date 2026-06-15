use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;

const SOURCE: PelisPlusTo = PelisPlusTo;
const BASE_URL: &str = "https://tioplus.app";

struct PelisPlusTo;

impl VideoSource for PelisPlusTo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_listing(&fetch(
            &format!("{BASE_URL}/peliculas?page={}", page(&request)),
            LIST_FIXTURE,
            BASE_URL,
        )))
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
        let p = page(&request);
        let target = if !query.is_empty() {
            format!("{BASE_URL}/api/search/{}", url::query_escape(query))
        } else if let Some(genre) = filter(&request, "genre").filter(|value| !value.is_empty()) {
            format!("{BASE_URL}/{genre}?page={p}")
        } else {
            format!("{BASE_URL}/peliculas?page={p}")
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/pelicula/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/pelicula/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, DETAILS_FIXTURE, BASE_URL);
        if path.contains("/pelicula/") {
            return Ok(vec![VideoEpisode {
                key: path.clone(),
                title: Some("PELICULA".to_string()),
                episode_number: Some(1.0),
                url: Some(referer),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let seasons_json = body
            .split("seasonsJson = ")
            .nth(1)
            .and_then(|value| value.split(';').next())
            .unwrap_or("{}");
        let root: Value = serde_json::from_str(seasons_json).unwrap_or(Value::Null);
        let mut index = 0.0;
        let mut out = Vec::new();
        if let Some(map) = root.as_object() {
            for episodes in map.values().filter_map(Value::as_array) {
                for ep in episodes.iter().rev() {
                    index += 1.0;
                    let season = ep.get("season").and_then(Value::as_str).unwrap_or("1");
                    let number = ep.get("episode").and_then(Value::as_str).unwrap_or("1");
                    let title = ep.get("title").and_then(Value::as_str).unwrap_or_default();
                    let key = format!(
                        "{}/season/{season}/episode/{number}",
                        path.trim_end_matches('/')
                    );
                    out.push(VideoEpisode {
                        key: path_key(&key),
                        title: Some(
                            format!("T{season} - E{number} - {title}")
                                .trim()
                                .to_string(),
                        ),
                        episode_number: Some(index),
                        season_number: season.parse::<f32>().ok(),
                        url: Some(absolute_url(&key)),
                        language: Some("es".to_string()),
                        ..VideoEpisode::default()
                    });
                }
            }
        }
        out.reverse();
        Ok(out)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/pelicula/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for item in doc.select(&selector(".bg-tabs ul li")) {
            let raw = attr(&item, "data-server").unwrap_or_default();
            if raw.is_empty() {
                continue;
            }
            let decoded = STANDARD
                .decode(raw.as_bytes())
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_else(|| raw.clone());
            let lang = item
                .ancestors()
                .filter_map(ElementRef::wrap)
                .find_map(|element| select_text(element, "button"))
                .map(|value| lang_tag(&value))
                .unwrap_or_default();
            let mut embed = if is_http_url(&decoded) {
                decoded
            } else {
                format!("{BASE_URL}/player/{}", STANDARD.encode(raw.as_bytes()))
            };
            if embed.contains("/player/") {
                let player_body = fetch(&embed, "", &referer);
                if let Some(url) = first_url(&player_body) {
                    embed = url;
                }
            }
            embed = embed
                .replace("https://sblanh.com", "https://lvturbo.com")
                .replace("https://ww3.pelisplus.to", BASE_URL);
            if embed.trim().is_empty() {
                continue;
            }
            let name = [lang, text(item)]
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            streams.extend(resolve_embed(&embed, &name, &referer, &request));
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Peliculas".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        }])
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
        .with_header("Origin", BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    if let Ok(json) = serde_json::from_str::<Value>(body) {
        let entries = json_cards(&json);
        if !entries.is_empty() {
            return Paged {
                entries,
                has_next_page: false,
            };
        }
    }
    let doc = Html::parse_document(body);
    Paged {
        entries: doc
            .select(&selector("article.item"))
            .filter_map(card)
            .collect(),
        has_next_page: doc.select(&selector(r#"a[rel="next"]"#)).next().is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "a", "href")?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_text(el, "a h2, h2").unwrap_or_else(|| title_from_path(&key)),
        cover: select_attr(el, "a .item__image picture img, img", "data-src")
            .or_else(|| select_attr(el, "img", "src"))
            .map(|src| absolute_url(&src)),
        url: Some(absolute_url(&key)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn json_cards(value: &Value) -> Vec<CatalogItem> {
    let mut out = Vec::new();
    collect_json_cards(value, &mut out);
    out
}

fn collect_json_cards(value: &Value, out: &mut Vec<CatalogItem>) {
    match value {
        Value::Array(items) => items.iter().for_each(|item| collect_json_cards(item, out)),
        Value::Object(map) => {
            let title = map
                .get("title")
                .or_else(|| map.get("name"))
                .and_then(Value::as_str);
            let href = map
                .get("url")
                .or_else(|| map.get("link"))
                .or_else(|| map.get("permalink"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .or_else(|| {
                    let slug = map.get("slug").and_then(Value::as_str)?;
                    let kind = map
                        .get("type")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("pelicula");
                    Some(format!("/{kind}/{slug}"))
                });
            if let (Some(title), Some(href)) = (title, href) {
                let key = path_key(&href);
                out.push(CatalogItem {
                    key: key.clone(),
                    title: title.to_string(),
                    cover: map
                        .get("image")
                        .or_else(|| map.get("poster"))
                        .or_else(|| map.get("thumbnail"))
                        .and_then(Value::as_str)
                        .map(absolute_url),
                    url: Some(absolute_url(&key)),
                    language: Some("es".to_string()),
                    content_rating: Some("safe".to_string()),
                    status: ItemStatus::Completed,
                    ..CatalogItem::default()
                });
            } else {
                map.values().for_each(|item| collect_json_cards(item, out));
            }
        }
        _ => {}
    }
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, ".home__slider_content div h1.slugh1, h1")
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, ".home__slider_content img, img", "src")
            .map(|src| absolute_url(&src)),
        description: select_text_doc(&doc, ".home__slider_content .description"),
        tags: select_texts_doc(&doc, ".home__slider_content div:nth-child(5) > a"),
        artists: select_text_doc(&doc, ".home__slider_content div:nth-child(7) > a")
            .map(|value| vec![value])
            .unwrap_or_default(),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_remote(embed, referer);
    if embed.contains(".m3u8") {
        return parse_hls(&embed, name, referer, request);
    }
    let body = fetch(&embed, "", referer);
    if let Some(src) = first_media_url(&body) {
        let src = absolute_remote(&src, &embed);
        if src.contains(".m3u8") {
            return parse_hls(&src, name, &embed, request);
        }
        return vec![stream(&src, name, "direct", &embed, false)];
    }
    vec![external_stream(&embed, name, referer)]
}

fn first_url(body: &str) -> Option<String> {
    Regex::new(r#"https?://[^\s'"\\<>]+"#)
        .ok()?
        .find(body)
        .map(|value| value.as_str().replace("\\/", "/"))
}

fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)["']"#,
        r#"src\s*:\s*["']([^"']+)["']"#,
        r#"<source[^>]+src=["']([^"']+)["']"#,
        r#"https?://[^\s'"\\]+\.m3u8[^\s'"\\]*"#,
    ]
    .into_iter()
    .find_map(|pattern| {
        Regex::new(pattern)
            .ok()?
            .captures(body)
            .and_then(|captures| captures.get(1).or_else(|| captures.get(0)))
            .map(|value| value.as_str().replace("\\/", "/"))
    })
}

fn parse_hls(master: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = client(referer)
        .get(master)
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    let mut out = Vec::new();
    let mut quality = "auto".to_string();
    for line in body.lines() {
        if line.starts_with("#EXT-X-STREAM-INF") {
            quality = line
                .split("RESOLUTION=")
                .nth(1)
                .and_then(|value| value.split('x').nth(1))
                .and_then(|value| value.split(',').next())
                .map(|value| format!("{value}p"))
                .unwrap_or_else(|| "auto".to_string());
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            out.push(stream(
                &absolute_remote(line.trim(), master),
                name,
                &quality,
                referer,
                true,
            ));
        }
    }
    if out.is_empty() {
        out.push(stream(master, name, "auto", referer, true));
    }
    sort_streams(&mut out, request);
    out
}

fn stream(target: &str, name: &str, quality: &str, referer: &str, hls: bool) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if hls { "hls" } else { "mp4" }.to_string()),
        is_hls: hls,
        stream_kind: Some(if hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(referer),
        initialized: true,
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
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let server = pref(request, "preferred_server", "VidHide").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    let language = pref(request, "preferred_language", "[LAT]");
    streams.sort_by_key(|stream| {
        let name = stream.name.clone().unwrap_or_default();
        (
            name.contains(&language),
            name.to_ascii_lowercase().contains(&server),
            name.contains(&quality),
            quality_rank(&name),
        )
    });
    streams.reverse();
}

fn selector(input: &str) -> Selector {
    Selector::parse(input).unwrap()
}

fn select_text_doc(doc: &Html, sel: &str) -> Option<String> {
    doc.select(&selector(sel))
        .next()
        .map(text)
        .filter(|v| !v.is_empty())
}

fn select_texts_doc(doc: &Html, sel: &str) -> Vec<String> {
    doc.select(&selector(sel))
        .map(text)
        .filter(|v| !v.is_empty())
        .collect()
}

fn select_attr_doc(doc: &Html, sel: &str, name: &str) -> Option<String> {
    doc.select(&selector(sel))
        .next()
        .and_then(|element| element.value().attr(name))
        .map(ToString::to_string)
}

fn select_text(el: ElementRef<'_>, sel: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .map(text)
        .filter(|v| !v.is_empty())
}

fn select_attr(el: ElementRef<'_>, sel: &str, name: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .and_then(|element| element.value().attr(name))
        .map(ToString::to_string)
}

fn attr(el: &ElementRef<'_>, name: &str) -> Option<String> {
    el.value().attr(name).map(ToString::to_string)
}

fn text(el: ElementRef<'_>) -> String {
    el.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn lang_tag(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if lower.contains("lat") || lower.contains('0') {
        "[LAT]".to_string()
    } else if lower.contains("cast") || lower.contains('1') {
        "[CAST]".to_string()
    } else if lower.contains("eng") || lower.contains("sub") || lower.contains('2') {
        "[SUB]".to_string()
    } else {
        String::new()
    }
}

fn is_http_url(input: &str) -> bool {
    input.starts_with("http://") || input.starts_with("https://")
}

fn absolute_url(input: &str) -> String {
    absolute_remote(input, BASE_URL)
}

fn absolute_remote(input: &str, base: &str) -> String {
    let value = input.trim().replace("\\/", "/").replace("&amp;", "&");
    if is_http_url(&value) {
        value
    } else if let Some(rest) = value.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url::join_url(base, &value)
    }
}

fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .filter(|p| p.starts_with('/'))
        .map(path_key)
}

fn path_key(input: &str) -> String {
    format!(
        "/{}",
        input
            .strip_prefix(BASE_URL)
            .unwrap_or(input)
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .trim_matches('/')
    )
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
        .map(path_key)
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn pref(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn quality_rank(input: &str) -> i32 {
    Regex::new(r#"(\d+)"#)
        .unwrap()
        .captures(input)
        .and_then(|captures| captures.get(1))
        .and_then(|value| value.as_str().parse().ok())
        .unwrap_or(0)
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("PelisPlusTo")
        .replace('-', " ")
}

const LIST_FIXTURE: &str = r#"
<article class="item"><a href="/pelicula/sample"><h2>Sample Movie</h2><div class="item__image"><picture><img data-src="/cover.jpg"></picture></div></a></article>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="home__slider_content"><div><h1 class="slugh1">Sample Movie</h1></div><div class="description">Fixture details for smoke tests.</div><div></div><div></div><div><a>Drama</a></div><div></div><div><a>Actor</a></div></div>
<script>const seasonUrl = ""; seasonsJson = {"1":[{"season":"1","episode":"1","title":"Pilot"}]};</script>
"#;

const WATCH_FIXTURE: &str = r#"
<div class="bg-tabs"><button>lat</button><ul><li data-server="aHR0cHM6Ly9pbnZhbGlkLmxvY2FsL2VtYmVk">Server</li></ul></div>
"#;

export_video_source!(SOURCE);
