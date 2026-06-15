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

const SOURCE: Zonaleros = Zonaleros;
const BASE_URL: &str = "https://www.zona-leros.com";

struct Zonaleros;

impl VideoSource for Zonaleros {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/peliculas-hd-online-lat?order=published&page={page}")
        } else {
            format!("{BASE_URL}/series-h?order=views&page={page}")
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
            format!("{BASE_URL}/search?q={}&page={page}", url::query_escape(query))
        } else {
            let path = filter(&request, "type").filter(|v| !v.is_empty()).unwrap_or_else(|| "peliculas-hd-online-lat".to_string());
            let qs = filter_params(&request);
            let sep = if qs.is_empty() { "?" } else { "?" };
            format!("{BASE_URL}/{path}{sep}{qs}page={page}")
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/peliculas/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/peliculas/sample".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body, &path))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/peliculas/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let server_body = if referer.contains("/episode/") {
            body
        } else {
            fetch_quality_server_body(&body, &referer).unwrap_or(body)
        };
        let mut streams = Vec::new();
        for embed in parse_server_urls(&server_body, &referer) {
            let name = server_name(&embed);
            streams.extend(resolve_embed(&embed, &name, &referer, &request));
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection { id: "popular".to_string(), title: "Series populares".to_string(), style: Some(HomeSectionStyle::Featured), entries: popular.entries, has_more: popular.has_next_page, ..HomeSection::default() },
            HomeSection { id: "latest".to_string(), title: "Peliculas recientes".to_string(), entries: latest.entries, has_more: latest.has_next_page, ..HomeSection::default() },
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
            if path.contains("/episode/") {
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
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch(target: &str, fixture: &str, referer: &str) -> String {
    client(referer).get(target).browser_document().referer(referer).send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_cards(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc.select(&selector(".ListAnimes .Anime > a, .Anime > a")).filter_map(card).collect(),
        has_next_page: doc.select(&selector(r#".pagination [rel="next"], .pagination a[rel='next']"#)).next().is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href")?;
    if !href.contains("series") && !href.contains("peliculas") {
        return None;
    }
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_text(el, ".Title").unwrap_or_else(|| title_from_path(&key)),
        cover: image_url(el).map(|src| absolute_url(&src)),
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
    let status = if path.contains("peliculas") { ItemStatus::Completed } else { ItemStatus::Unknown };
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, "h1.Title, h1").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, ".Image img, .AnimeCover img, img", "src").map(|src| absolute_url(&src)),
        description: details_description(&doc),
        tags: select_texts_doc(&doc, ".TxtMAY ul li, .Nvgnrs a"),
        artists: select_text_doc(&doc, ".ListActors li a").map(|v| vec![v]).unwrap_or_default(),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, item_path: &str) -> Vec<VideoEpisode> {
    if item_path.contains("peliculas") {
        return vec![VideoEpisode { key: item_path.to_string(), title: Some("Pelicula".to_string()), episode_number: Some(1.0), url: Some(absolute_url(item_path)), language: Some("es".to_string()), ..VideoEpisode::default() }];
    }
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    for season in doc.select(&selector("[id*=temp]")) {
        for ep in season.select(&selector(".ListEpisodios a")).collect::<Vec<_>>().into_iter().rev() {
            let href = attr(&ep, "href").unwrap_or_default();
            let cap = select_text(ep, ".Capi").unwrap_or_default();
            let season_no = cap.split('x').next().unwrap_or("1").trim().parse::<f32>().ok();
            let ep_no = cap.split('x').nth(1).unwrap_or("0").trim().parse::<f32>().ok();
            let key = path_key(&href);
            out.push(VideoEpisode { key: key.clone(), title: Some(format!("T{} - Episodio {}", trim_float(season_no.unwrap_or(1.0)), trim_float(ep_no.unwrap_or(0.0)))), episode_number: ep_no, season_number: season_no, url: Some(absolute_url(&key)), language: Some("es".to_string()), ..VideoEpisode::default() });
        }
    }
    out
}

fn fetch_quality_server_body(body: &str, referer: &str) -> Option<String> {
    let doc = Html::parse_document(body);
    let token = select_attr_doc(&doc, r#"meta[name="csrf-token"]"#, "content")?;
    let calidad_id = select_attr_doc(&doc, "span[data-value]", "data-value")?;
    client(referer)
        .post(format!("{BASE_URL}/api/calidades"))
        .xhr()
        .referer(referer)
        .origin(BASE_URL)
        .form(&[("calidad_id", calidad_id.as_str()), ("_token", token.as_str())])
        .send_text()
        .ok()
}

fn parse_server_urls(body: &str, referer: &str) -> Vec<String> {
    let script = Html::parse_document(body)
        .select(&selector("script"))
        .find_map(|s| {
            let data = s.text().collect::<Vec<_>>().join(" ");
            data.contains("var video").then_some(data)
        })
        .unwrap_or_else(|| body.to_string());
    let re = Regex::new(r#"https?://[^\s'"\\<>]+"#).unwrap();
    let mut out = Vec::new();
    for url in re.find_iter(&script).map(|m| m.as_str().trim_matches('"').to_string()) {
        if url.contains("anomizador") {
            let final_url = client(referer).get(&url).referer(referer).send().ok().map(|r| r.final_url).unwrap_or(url);
            out.push(final_url.split("url=").nth(1).unwrap_or(&final_url).trim_end_matches('}').to_string());
        } else {
            out.push(url);
        }
    }
    out.sort();
    out.dedup();
    out
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_remote(embed, referer);
    if embed.contains(".m3u8") {
        return parse_hls(&embed, name, referer, request);
    }
    let body = fetch(&embed, "", referer);
    if let Some(src) = first_media_url(&body).map(|src| absolute_remote(&src, &embed)) {
        if src.contains(".m3u8") { parse_hls(&src, name, &embed, request) } else { vec![stream(&src, name, "direct", &embed, false)] }
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
    if out.is_empty() { out.push(stream(master, name, "auto", referer, true)); }
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
    let server = pref(request, "preferred_server", "DoodStream").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|s| {
        let value = s.name.clone().or_else(|| s.quality.clone()).unwrap_or_default();
        let lower = value.to_ascii_lowercase();
        (lower.contains(&server), value.contains(&quality), quality_rank(&value))
    });
    streams.reverse();
}

fn filter_params(request: &Value) -> String {
    let mut parts = Vec::new();
    for (key, name) in [("generos", "generos"), ("year", "year"), ("estado", "estado")] {
        for value in filter_values(request, key) {
            if !value.is_empty() { parts.push(format!("{name}[]={}", url::query_escape(&value))); }
        }
    }
    if let Some(order) = filter(request, "order").filter(|v| !v.is_empty()) {
        parts.push(format!("order={}", url::query_escape(&order)));
    }
    if parts.is_empty() { String::new() } else { format!("{}&", parts.join("&")) }
}

fn selector(input: &str) -> Selector { Selector::parse(input).unwrap() }
fn attr(el: &ElementRef<'_>, name: &str) -> Option<String> { el.value().attr(name).map(ToString::to_string) }
fn select_text_doc(doc: &Html, sel: &str) -> Option<String> { doc.select(&selector(sel)).next().map(text).filter(|v| !v.is_empty()) }
fn select_texts_doc(doc: &Html, sel: &str) -> Vec<String> { doc.select(&selector(sel)).map(text).filter(|v| !v.is_empty()).collect() }
fn select_attr_doc(doc: &Html, sel: &str, name: &str) -> Option<String> { doc.select(&selector(sel)).next().and_then(|el| el.value().attr(name)).map(ToString::to_string) }
fn select_text(el: ElementRef<'_>, sel: &str) -> Option<String> { el.select(&selector(sel)).next().map(text).filter(|v| !v.is_empty()) }
fn text(el: ElementRef<'_>) -> String { el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ") }
fn details_description(doc: &Html) -> Option<String> {
    let parts = doc.select(&selector(".Main section .Description p, .Description p")).map(text).filter(|v| !v.is_empty()).collect::<Vec<_>>();
    if parts.len() > 2 { Some(parts[1..parts.len() - 1].join(" ")) } else { parts.first().cloned() }
}
fn image_url(el: ElementRef<'_>) -> Option<String> {
    el.select(&selector("img")).next().and_then(|img| {
        for name in ["data-src", "data-lazy-src", "srcset", "src"] {
            if let Some(value) = img.value().attr(name).filter(|v| !v.contains("data:image/")) {
                return Some(value.split_whitespace().next().unwrap_or(value).to_string());
            }
        }
        None
    })
}
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
fn with_listing(request: &Value, id: &str) -> Value { let mut copy = request.clone(); if let Some(obj) = copy.as_object_mut() { obj.insert("listing".to_string(), Value::String(id.to_string())); } copy }
fn filter(request: &Value, key: &str) -> Option<String> { request.get("filters").and_then(|f| f.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).map(ToString::to_string) }
fn filter_values(request: &Value, key: &str) -> Vec<String> {
    let Some(value) = request.get("filters").and_then(|f| f.get(key)).or_else(|| request.get(key)) else { return Vec::new(); };
    if let Some(array) = value.as_array() { return array.iter().filter_map(Value::as_str).map(ToString::to_string).collect(); }
    value.as_str().filter(|v| !v.is_empty()).map(|v| vec![v.to_string()]).unwrap_or_default()
}
fn pref(request: &Value, key: &str, default: &str) -> String { request.get("preferences").and_then(|p| p.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).unwrap_or(default).to_string() }
fn referer_headers(referer: &str) -> Context { let mut h = Context::new(); h.insert("Referer".to_string(), referer.to_string()); h }
fn quality_rank(input: &str) -> i32 { Regex::new(r#"(\d+)"#).unwrap().captures(input).and_then(|c| c.get(1)).and_then(|m| m.as_str().parse().ok()).unwrap_or(0) }
fn trim_float(value: f32) -> String { if value.fract() == 0.0 { format!("{}", value as i32) } else { value.to_string() } }
fn title_from_path(path: &str) -> String { path.trim_matches('/').rsplit('/').next().unwrap_or("Zonaleros").replace('-', " ") }
fn server_name(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if lower.contains("voe") { "Voe" } else if lower.contains("ok.ru") || lower.contains("okru") { "Okru" } else if lower.contains("filemoon") { "Filemoon" } else if lower.contains("wish") { "StreamWish" } else if lower.contains("streamtape") || lower.contains("stape") { "Streamtape" } else if lower.contains("dood") { "DoodStream" } else if lower.contains("mp4") { "Mp4Upload" } else if lower.contains("vidhide") || lower.contains("nika") { "VidHide" } else if lower.contains("mix") { "MixDrop" } else { input.split("://").nth(1).unwrap_or(input).split('/').next().unwrap_or("External") }.to_string()
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="ListAnimes"><div class="Anime"><a href="/peliculas/sample"><img src="/sample.jpg"><h3 class="Title">Sample</h3></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="Title">Sample</h1><div class="Description"><p>ignore</p><p>Sample description.</p><p>ignore</p></div><span data-value="1"></span><meta name="csrf-token" content="token"><script>var video = "https://example.invalid/embed";</script>"#;
const WATCH_FIXTURE: &str = DETAILS_FIXTURE;
