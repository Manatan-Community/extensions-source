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
use serde_json::{Value, json};

const SOURCE: ZeroAnime = ZeroAnime;
const BASE_URL: &str = "https://www4.zeroanime.xyz";

struct ZeroAnime;

impl VideoSource for ZeroAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/search?q=&letra=ALL&genero=ALL&years=2024&estado=2&orden=asc&p={page}")
        } else {
            format!("{BASE_URL}/search?q=&letra=&genero=ALL&years=ALL&estado=2&orden=desc&p={page}")
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged { entries: vec![fetch_details(&path)], has_next_page: false });
        }
        let page = page(&request);
        let target = if !query.is_empty() {
            format!("{BASE_URL}/search?q={}&p={page}", url::query_escape(query))
        } else {
            let qs = filter_params(&request);
            if qs.is_empty() {
                format!("{BASE_URL}/search?q=&letra=&genero=ALL&years=ALL&estado=2&orden=desc&p={page}")
            } else {
                format!("{BASE_URL}/search?{qs}&p={page}")
            }
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/ver/sample-1".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let mut streams = Vec::new();
        for (embed, server) in parse_embeds(&body, &referer) {
            streams.extend(resolve_embed(&embed, &server, &referer, &request));
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection { id: "popular".to_string(), title: "Populares".to_string(), style: Some(HomeSectionStyle::Featured), entries: popular.entries, has_more: popular.has_next_page, ..HomeSection::default() },
            HomeSection { id: "latest".to_string(), title: "Ultimos".to_string(), entries: latest.entries, has_more: latest.has_next_page, ..HomeSection::default() },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|path| absolute_url(&path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|path| absolute_url(&path)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(path) = path_from_url(input) {
            if path.contains("/ver/") {
                return Ok(Some(UrlResolveResult { episode: Some(json!({"key": path, "url": input, "language": "es"})), url: Some(input.to_string()), ..UrlResolveResult::default() }));
            }
            return Ok(Some(UrlResolveResult { item: Some(fetch_details(&path)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch(target: &str, fixture: &str, referer: &str) -> String {
    client(referer).get(target).browser_document().referer(referer).send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_cards(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc.select(&selector("ul.animes.list-unstyled.row li, li.col-6.col-sm-4.col-md-3.col-xl-2")).filter_map(card).collect(),
        has_next_page: doc.select(&selector("ul.pagination li.page-item:not(.active) a, .pagination a[rel='next']")).next().is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "a[href]", "href")?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_text(el, "div.title, .title").unwrap_or_else(|| title_from_path(&key)),
        cover: select_attr(el, "div.thumb img, img", "src").map(|src| absolute_url(&src)),
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
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, "h1.htitle, h1").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, "div.hentai_cover img, .hentai_cover img, img", "src").map(|src| absolute_url(&src)),
        description: select_text_doc(&doc, "div.vraven_text.single, .vraven_text.single, .description"),
        tags: select_texts_doc(&doc, "div.single_data div.list a, .single_data a"),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&doc),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let mut out = doc.select(&selector("li.hentai__chapter")).filter_map(|el| {
        let href = select_attr(el, "a[href]", "href")?;
        let name = select_text(el, "div.chapter_info span.title, span.title").unwrap_or_else(|| title_from_path(&href));
        let number = first_number(&name).unwrap_or(0.0);
        let key = path_key(&href);
        Some(VideoEpisode { key: key.clone(), title: Some(name), episode_number: Some(number), url: Some(absolute_url(&key)), language: Some("es".to_string()), ..VideoEpisode::default() })
    }).collect::<Vec<_>>();
    out.sort_by(|a, b| b.episode_number.partial_cmp(&a.episode_number).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn parse_embeds(body: &str, referer: &str) -> Vec<(String, String)> {
    let compact = body.replace(['\n', '\r', '\t'], "");
    let re = Regex::new(r#"<button[^>]+data-url=["']([^"']+)["'][^>]*>(.*?)</button>"#).unwrap();
    re.captures_iter(&compact).filter_map(|cap| {
        let raw = cap.get(1)?.as_str();
        let label = strip_tags(cap.get(2).map(|m| m.as_str()).unwrap_or("External"));
        let redirect = absolute_remote(&raw.replace("../redirect.php?", "/redirect.php?"), BASE_URL);
        let response = client(referer).get(&redirect).referer(referer).send().ok()?;
        let first = header_value(&response.headers, "refresh")
            .and_then(|value| refresh_url(&value))
            .or_else(|| query_url(&redirect, "url"))
            .unwrap_or(response.final_url);
        let target = absolute_remote(&first.replace("../video/", "/video/"), BASE_URL);
        let second = client(referer).get(&target).referer(referer).send().ok();
        let embed = second.as_ref()
            .and_then(|r| header_value(&r.headers, "refresh").and_then(|value| refresh_url(&value)))
            .unwrap_or_else(|| second.map(|r| r.final_url).unwrap_or(target));
        Some((embed, label))
    }).collect()
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_remote(embed, referer);
    if embed.contains(".m3u8") {
        return parse_hls(&embed, name, referer, request);
    }
    let body = fetch(&embed, "", referer);
    if let Some(src) = first_media_url(&body).map(|src| absolute_remote(&src, &embed)) {
        if src.contains(".m3u8") {
            parse_hls(&src, name, &embed, request)
        } else {
            vec![stream(&src, name, "direct", &embed, false)]
        }
    } else {
        vec![external_stream(&embed, name, referer)]
    }
}

fn first_media_url(body: &str) -> Option<String> {
    [r#"file\s*:\s*["']([^"']+)"#, r#"src\s*:\s*["']([^"']+)"#, r#"<source[^>]+src=["']([^"']+)"#, r#"https?://[^\s'"\\]+\.m3u8[^\s'"\\]*"#]
        .into_iter()
        .find_map(|pattern| Regex::new(pattern).ok()?.captures(body).and_then(|cap| cap.get(1).or_else(|| cap.get(0))).map(|m| m.as_str().replace("\\/", "/")))
}

fn parse_hls(master: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = client(referer).get(master).referer(referer).send_text().unwrap_or_default();
    let mut out = Vec::new();
    let mut quality = "auto".to_string();
    for line in body.lines() {
        if line.starts_with("#EXT-X-STREAM-INF") {
            quality = line.split("RESOLUTION=").nth(1).and_then(|v| v.split('x').nth(1)).and_then(|v| v.split(',').next()).map(|v| format!("{v}p")).unwrap_or_else(|| "auto".to_string());
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            out.push(stream(&absolute_remote(line.trim(), master), name, &quality, referer, true));
        }
    }
    if out.is_empty() {
        out.push(stream(master, name, "auto", referer, true));
    }
    sort_streams(&mut out, request);
    out
}

fn stream(target: &str, name: &str, quality: &str, referer: &str, hls: bool) -> VideoStream {
    VideoStream { url: target.to_string(), name: Some(format!("{name} {quality}")), quality: Some(format!("{name} {quality}")), format: Some(if hls { "hls" } else { "mp4" }.to_string()), is_hls: hls, stream_kind: Some(if hls { VideoStreamKind::Hls } else { VideoStreamKind::Direct }), headers: referer_headers(referer), initialized: true, ..VideoStream::default() }
}

fn external_stream(target: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream { url: target.to_string(), name: Some(format!("{name} External")), quality: Some(name.to_string()), stream_kind: Some(VideoStreamKind::External), headers: referer_headers(referer), initialized: true, ..VideoStream::default() }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let server = pref(request, "preferred_server", "mp4upload").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|stream| {
        let value = stream.name.clone().or_else(|| stream.quality.clone()).unwrap_or_default();
        let lower = value.to_ascii_lowercase();
        (lower.contains(&server), value.contains(&quality), quality_rank(&value))
    });
    streams.reverse();
}

fn filter_params(request: &Value) -> String {
    let mut parts = Vec::new();
    for value in filter_values(request, "genre") {
        parts.push(format!("genero[]={}", url::query_escape(&value)));
    }
    if let Some(year) = filter(request, "years").filter(|v| !v.is_empty()) {
        parts.push(format!("years={}", url::query_escape(&year)));
    }
    if let Some(status) = filter(request, "estado").filter(|v| !v.is_empty()) {
        parts.push(format!("estado={}", url::query_escape(&status)));
    }
    parts.join("&")
}

fn selector(input: &str) -> Selector { Selector::parse(input).unwrap() }
fn select_text_doc(doc: &Html, sel: &str) -> Option<String> { doc.select(&selector(sel)).next().map(text).filter(|v| !v.is_empty()) }
fn select_texts_doc(doc: &Html, sel: &str) -> Vec<String> { doc.select(&selector(sel)).map(text).filter(|v| !v.is_empty()).collect() }
fn select_attr_doc(doc: &Html, sel: &str, name: &str) -> Option<String> { doc.select(&selector(sel)).next().and_then(|el| el.value().attr(name)).map(ToString::to_string) }
fn select_text(el: ElementRef<'_>, sel: &str) -> Option<String> { el.select(&selector(sel)).next().map(text).filter(|v| !v.is_empty()) }
fn select_attr(el: ElementRef<'_>, sel: &str, name: &str) -> Option<String> { el.select(&selector(sel)).next().and_then(|el| el.value().attr(name)).map(ToString::to_string) }
fn text(el: ElementRef<'_>) -> String { el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ") }
fn strip_tags(input: &str) -> String { text(Html::parse_fragment(input).root_element()) }
fn absolute_url(input: &str) -> String { absolute_remote(input, BASE_URL) }
fn absolute_remote(input: &str, base: &str) -> String {
    let trimmed = input.trim().replace("\\/", "/").replace("&amp;", "&");
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") { trimmed } else if let Some(rest) = trimmed.strip_prefix("//") { format!("https://{rest}") } else { url::join_url(base, &trimmed) }
}
fn path_from_url(input: &str) -> Option<String> { input.strip_prefix(BASE_URL).filter(|p| p.starts_with('/')).map(path_key) }
fn path_key(input: &str) -> String { format!("/{}", input.strip_prefix(BASE_URL).unwrap_or(input).split(['?', '#']).next().unwrap_or(input).trim_matches('/')) }
fn request_key(request: &Value, field: &str) -> Option<String> {
    request.get(field).and_then(|v| v.get("key").or_else(|| v.get("url")).and_then(Value::as_str).or_else(|| v.as_str())).or_else(|| request.get("key").and_then(Value::as_str)).map(path_key)
}
fn page(request: &Value) -> u64 { request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1) }
fn listing(request: &Value) -> &str { request.get("listing").or_else(|| request.get("listingId")).and_then(Value::as_str).unwrap_or("popular") }
fn with_listing(request: &Value, id: &str) -> Value {
    let mut copy = request.clone();
    if let Some(obj) = copy.as_object_mut() { obj.insert("listing".to_string(), Value::String(id.to_string())); }
    copy
}
fn filter(request: &Value, key: &str) -> Option<String> { request.get("filters").and_then(|f| f.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).map(ToString::to_string) }
fn filter_values(request: &Value, key: &str) -> Vec<String> {
    let Some(value) = request.get("filters").and_then(|f| f.get(key)).or_else(|| request.get(key)) else { return Vec::new(); };
    if let Some(array) = value.as_array() { return array.iter().filter_map(Value::as_str).map(ToString::to_string).collect(); }
    value.as_str().filter(|v| !v.is_empty()).map(|v| vec![v.to_string()]).unwrap_or_default()
}
fn pref(request: &Value, key: &str, default: &str) -> String { request.get("preferences").and_then(|p| p.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).unwrap_or(default).to_string() }
fn referer_headers(referer: &str) -> Context { let mut h = Context::new(); h.insert("Referer".to_string(), referer.to_string()); h }
fn first_number(input: &str) -> Option<f32> { Regex::new(r#"\d+(?:\.\d+)?"#).unwrap().find(input).and_then(|m| m.as_str().parse().ok()) }
fn quality_rank(input: &str) -> i32 { Regex::new(r#"(\d+)"#).unwrap().captures(input).and_then(|c| c.get(1)).and_then(|m| m.as_str().parse().ok()).unwrap_or(0) }
fn title_from_path(path: &str) -> String { path.trim_matches('/').rsplit('/').next().unwrap_or("zeroanime").replace('-', " ") }
fn parse_status(doc: &Html) -> ItemStatus {
    let lower = select_text_doc(doc, "div.data").unwrap_or_default().to_ascii_lowercase();
    if lower.contains("finalizado") { ItemStatus::Completed } else if lower.contains("emision") || lower.contains("emisión") { ItemStatus::Ongoing } else { ItemStatus::Unknown }
}
fn header_value(headers: &[(String, String)], name: &str) -> Option<String> { headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()) }
fn refresh_url(input: &str) -> Option<String> { Regex::new(r#"(?i)url=([^;]+)$"#).unwrap().captures(input).and_then(|cap| cap.get(1).map(|m| m.as_str().trim().to_string())) }
fn query_url(input: &str, key: &str) -> Option<String> {
    let marker = format!("{key}=");
    input.split('?').nth(1)?.split('&').find_map(|part| part.strip_prefix(&marker).map(ToString::to_string))
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<ul class="animes list-unstyled row"><li class="col-6"><a href="/anime/sample"><div class="thumb"><img src="/sample.jpg"></div><div class="title">Sample</div></a></li></ul>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="htitle">Sample</h1><div class="hentai_cover"><img src="/sample.jpg"></div><div class="vraven_text single">Sample description.</div><li class="hentai__chapter"><a href="/ver/sample-1"><div class="chapter_info"><span class="title">Episodio 1</span></div></a></li>"#;
const WATCH_FIXTURE: &str = r#"<button id="embed-1" data-url="https://example.invalid/embed">mp4upload</button>"#;
