use base64::{Engine, engine::general_purpose};
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
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Pandrama = Pandrama;
const BASE_URL: &str = "https://pandrama.com";

struct Pandrama;

impl VideoSource for Pandrama {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/explorar/Dramas--hits------{page}---/")
        } else {
            format!("{BASE_URL}/explorar/Dramas--------{page}---/")
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE, BASE_URL)))
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
                "{BASE_URL}/buscar/media/{}----------{page}---/",
                url::query_escape(query)
            )
        } else if let Some(genre) = filter(&request, "genre").filter(|value| !value.is_empty()) {
            format!("{BASE_URL}{}", genre.replace("page", &page.to_string()))
        } else {
            format!("{BASE_URL}/explorar/Dramas--------{page}---/")
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/media/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/media/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, DETAILS_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
        for link in doc.select(&selector(".anthology-list-play li a")) {
            let label = text(link);
            let href = attr(&link, "href")
                .map(|h| absolute_url(&h))
                .unwrap_or_default();
            if href.is_empty() {
                continue;
            }
            if let Some((_, urls)) = grouped.iter_mut().find(|(name, _)| *name == label) {
                urls.push(href);
            } else {
                grouped.push((label, vec![href]));
            }
        }
        let mut episodes = grouped
            .into_iter()
            .map(|(label, urls)| {
                let ep = label
                    .split("Ep.")
                    .nth(1)
                    .unwrap_or(&label)
                    .trim()
                    .parse::<f32>()
                    .unwrap_or(0.0);
                let key = serde_json::to_string(&urls).unwrap_or_else(|_| "[]".to_string());
                VideoEpisode {
                    key,
                    title: Some(format!("Episodio {}", trim_float(ep))),
                    episode_number: Some(ep),
                    language: Some("es".to_string()),
                    ..VideoEpisode::default()
                }
            })
            .collect::<Vec<_>>();
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let raw = request_raw_key(&request, "episode").unwrap_or_else(|| "[]".to_string());
        let pages = serde_json::from_str::<Vec<String>>(&raw).unwrap_or_else(|_| vec![raw]);
        let mut streams = Vec::new();
        for page_url in pages {
            let body = fetch(&page_url, WATCH_FIXTURE, BASE_URL);
            let doc = Html::parse_document(&body);
            let Some(script) = doc
                .select(&selector("script"))
                .map(|s| s.inner_html())
                .find(|s| s.contains("var player_aaaa"))
            else {
                continue;
            };
            let json = script
                .split("var player_aaaa=")
                .nth(1)
                .and_then(|s| s.split(';').next())
                .unwrap_or_default()
                .trim();
            let player = serde_json::from_str::<PlayerDto>(json).unwrap_or_default();
            let mut embed = player.url.unwrap_or_default();
            if player.encrypt == Some(2) {
                embed = general_purpose::STANDARD
                    .decode(embed.as_bytes())
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .unwrap_or_default();
            }
            embed = percent_decode(&embed);
            if !embed.is_empty() {
                streams.extend(resolve_embed(
                    &embed,
                    &server_label(&embed),
                    &page_url,
                    &request,
                ));
            }
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
                title: "Dramas".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Mas vistos".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|p| absolute_url(&p)))
    }

    fn episode_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(None)
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
    let doc = Html::parse_document(body);
    Paged {
        entries: doc
            .select(&selector("a.public-list-exp"))
            .filter_map(card)
            .collect(),
        has_next_page: doc
            .select(&selector(
                "[title=\"Pagina siguiente\"], [title=\"Página siguiente\"]",
            ))
            .next()
            .is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href")?;
    let key = path_key(&href);
    let lang = select_text(el, ".public-prt").unwrap_or_default();
    let prefix = if lang.contains("Español") {
        "[MX] "
    } else if lang.contains("Castellano") {
        "[ES] "
    } else {
        ""
    };
    Some(CatalogItem {
        key: key.clone(),
        title: format!(
            "{prefix}{}",
            attr(&el, "title").unwrap_or_else(|| title_from_path(&key))
        )
        .trim()
        .to_string(),
        cover: select_attr(el, "img", "data-src")
            .or_else(|| select_attr(el, "img", "src"))
            .map(|s| absolute_url(&s)),
        url: Some(absolute_url(&key)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    let mut item = CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, "h1, .this-title").unwrap_or_else(|| title_from_path(path)),
        description: select_text_doc(&doc, "#height_limit"),
        cover: select_attr_doc(&doc, "img", "data-src")
            .or_else(|| select_attr_doc(&doc, "img", "src"))
            .map(|s| absolute_url(&s)),
        tags: select_texts_doc(&doc, ".this-desc-labels a"),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    };
    for info in doc.select(&selector(".this-info")) {
        let row = text(info);
        if row.contains("Director:") {
            if let Some(value) = select_text(info, "a") {
                item.authors.push(value);
            }
        } else if row.contains("Actores:") {
            if let Some(value) = select_text(info, "a") {
                item.artists.push(value);
            }
        }
    }
    item
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
            .and_then(|c| c.get(1).or_else(|| c.get(0)))
            .map(|m| m.as_str().replace("\\/", "/"))
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
                .and_then(|v| v.split('x').nth(1))
                .and_then(|v| v.split(',').next())
                .map(|v| format!("{v}p"))
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
    let server = pref(request, "preferred_server", "Vk").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|s| {
        let name = s.name.clone().unwrap_or_default();
        (
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
        .and_then(|e| e.value().attr(name))
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
        .and_then(|e| e.value().attr(name))
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

fn absolute_url(input: &str) -> String {
    absolute_remote(input, BASE_URL)
}

fn absolute_remote(input: &str, base: &str) -> String {
    let value = input.trim().replace("\\/", "/").replace("&amp;", "&");
    if value.starts_with("http://") || value.starts_with("https://") {
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
    request_raw_key(request, field).map(|v| path_key(&v))
}

fn request_raw_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|v| {
            v.get("key")
                .or_else(|| v.get("url"))
                .and_then(Value::as_str)
                .or_else(|| v.as_str())
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(ToString::to_string)
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
        .and_then(|p| p.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut cloned = request.clone();
    if let Value::Object(ref mut map) = cloned {
        map.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    cloned
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn server_label(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if lower.contains("ok.ru") || lower.contains("okru") {
        "Okru".to_string()
    } else if lower.contains("vk.") {
        "Vk".to_string()
    } else {
        host_name(input)
    }
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

fn quality_rank(input: &str) -> i32 {
    Regex::new(r#"(\d+)"#)
        .unwrap()
        .captures(input)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0)
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("Pandrama")
        .replace(['-', '_'], " ")
}

fn trim_float(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i32)
    } else {
        value.to_string()
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                out.push(hex);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

#[derive(Default, Deserialize)]
struct PlayerDto {
    encrypt: Option<i32>,
    url: Option<String>,
}

const LIST_FIXTURE: &str = r#"
<a class="public-list-exp" href="/media/sample-drama" title="Sample Drama"><span class="public-prt">Español</span><img data-src="/cover.jpg"></a>
<a title="Pagina siguiente" href="/explorar/Dramas--------2---/">Next</a>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample Drama</h1><div id="height_limit">Fixture details for local smoke tests.</div>
<div class="this-desc-labels"><a>Drama</a><a>Romance</a></div>
<div class="this-info"><strong>Director:</strong><a>Director Name</a></div>
<ul class="anthology-list-play"><li><a href="/play/sample-1-vk">Ep. 1</a></li><li><a href="/play/sample-1-okru">Ep. 1</a></li></ul>
"#;

const WATCH_FIXTURE: &str = r#"<script>var player_aaaa={"encrypt":0,"url":"https://vk.com/video_ext.php?oid=1&id=2"};</script>"#;

export_video_source!(SOURCE);
