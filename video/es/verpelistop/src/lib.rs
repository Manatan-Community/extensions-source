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

const SOURCE: VerPelisTop = VerPelisTop;
const BASE_URL: &str = "https://www1.verpelis.top";
const AJAX_URL: &str = "https://www1.verpelis.top/wp-admin/admin-ajax.php";

struct VerPelisTop;

impl VideoSource for VerPelisTop {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/online/page/{}", page(&request))
        } else {
            BASE_URL.to_string()
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
            format!("{BASE_URL}/?s={}", url::query_escape(query))
        } else if let Some(genre) = filter(&request, "genre").filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/{genre}/page/{page}")
        } else {
            BASE_URL.to_string()
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
        let mut streams = Vec::new();
        for (embed, lang, server) in parse_embeds(&body, &referer) {
            let label = [lang.as_str(), server.as_str()].into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join(" ");
            streams.extend(resolve_embed(&embed, &label, &referer, &request));
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection { id: "popular".to_string(), title: "Populares".to_string(), style: Some(HomeSectionStyle::Featured), entries: popular.entries, has_more: popular.has_next_page, ..HomeSection::default() },
            HomeSection { id: "latest".to_string(), title: "Online".to_string(), entries: latest.entries, has_more: latest.has_next_page, ..HomeSection::default() },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> { Ok(request_key(&request, "item").map(|path| absolute_url(&path))) }
    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> { Ok(request_key(&request, "episode").map(|path| absolute_url(&path))) }
    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(path) = path_from_url(input) {
            if path.starts_with("/episodio/") {
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
fn post_ajax(params: &[(&str, &str)], referer: &str) -> String {
    client(referer).post(AJAX_URL).xhr().referer(referer).origin(BASE_URL).form(params).send_text().unwrap_or_default()
}

fn parse_cards(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc.select(&selector("#featured-titles article > div.poster, #archive-content article > div.poster, article div.poster")).filter_map(card).collect(),
        has_next_page: doc.select(&selector("#nextpagination, .pagination a, div.pag_episodes a")).next().is_some(),
    }
}
fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "a[href]", "href")?;
    let key = path_key(&href);
    let title = select_attr(el, "img", "alt").or_else(|| select_text(el, ".data h3, h3")).unwrap_or_else(|| title_from_path(&key));
    Some(CatalogItem { key: key.clone(), title, cover: image_url(el), url: Some(absolute_url(&key)), language: Some("es".to_string()), content_rating: Some("safe".to_string()), status: ItemStatus::Unknown, ..CatalogItem::default() })
}
fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, "div.sheader div.data h1, h1").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, "div.sheader div.poster img, div.poster img", "src").or_else(|| select_attr_doc(&doc, "meta[property='og:image']", "content")).map(|src| absolute_url(&src)),
        description: select_text_doc(&doc, "#info .wp-content p, #single .wp-content p, .wp-content p, .description"),
        tags: select_texts_doc(&doc, "div.sgeneros a, a[href*='/genero/']"),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: if path.contains("/series/") { ItemStatus::Unknown } else { ItemStatus::Completed },
        initialized: true,
        ..CatalogItem::default()
    }
}
fn parse_episodes(body: &str, item_path: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let seasons = doc.select(&selector("div#seasons div.se-c")).collect::<Vec<_>>();
    if seasons.is_empty() {
        return vec![VideoEpisode { key: item_path.to_string(), title: Some("Pelicula".to_string()), episode_number: Some(1.0), url: Some(absolute_url(item_path)), language: Some("es".to_string()), ..VideoEpisode::default() }];
    }
    let mut out = Vec::new();
    for season in seasons {
        let season_no = attr(&season, "data-season").unwrap_or_else(|| "1".to_string());
        for el in season.select(&selector("ul.episodios li")) {
            let href = select_attr(el, "a[href]", "href").unwrap_or_default();
            let ep_text = select_text(el, "div.numerando").unwrap_or_default();
            let ep_no = first_number(&ep_text).unwrap_or(0.0);
            let name = select_text(el, "div.epst").unwrap_or_else(|| "Sin titulo".to_string());
            let key = path_key(&href);
            out.push(VideoEpisode { key: key.clone(), title: Some(format!("Temporada {season_no} - Episodio {}: {name}", trim_float(ep_no))), episode_number: Some(ep_no), season_number: season_no.parse::<f32>().ok(), url: Some(absolute_url(&key)), language: Some("es".to_string()), ..VideoEpisode::default() });
        }
    }
    out.reverse();
    out
}

fn parse_embeds(body: &str, referer: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let player_re = Regex::new(r#"data-type=["'](.+?)["'][^>]+data-post=["'](.+?)["'][^>]+data-nume=["'](.+?)["']|data-post=["'](.+?)["'][^>]+data-nume=["'](.+?)["'][^>]+data-type=["'](.+?)["']"#).unwrap();
    for cap in player_re.captures_iter(body) {
        let player_type = cap.get(1).or_else(|| cap.get(6)).map(|m| m.as_str()).unwrap_or_default();
        let post = cap.get(2).or_else(|| cap.get(4)).map(|m| m.as_str()).unwrap_or_default();
        let nume = cap.get(3).or_else(|| cap.get(5)).map(|m| m.as_str()).unwrap_or_default();
        let response = post_ajax(&[("action", "doo_player_ajax"), ("post", post), ("nume", nume), ("type", player_type)], referer);
        if let Some(iframe) = iframe_src(&response) {
            let page = fetch(&absolute_remote(&iframe, BASE_URL), "", referer);
            out.extend(parse_embed_page(&page));
        }
    }
    if out.is_empty() {
        for iframe in iframe_sources(body) {
            let page = fetch(&absolute_remote(&iframe, BASE_URL), "", referer);
            let links = parse_embed_page(&page);
            if links.is_empty() {
                out.push((iframe.clone(), String::new(), server_name(&iframe)));
            } else {
                out.extend(links);
            }
        }
    }
    out
}

fn parse_embed_page(body: &str) -> Vec<(String, String, String)> {
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    for item in doc.select(&selector(".OD li[onclick], li[onclick]")) {
        let onclick = attr(&item, "onclick").unwrap_or_default();
        let Some(url) = Regex::new(r#"\(['"]([^'"]+)"#).unwrap().captures(&onclick).and_then(|cap| cap.get(1).map(|m| m.as_str().to_string())) else { continue; };
        let server = select_text(item, "span").unwrap_or_else(|| server_name(&url));
        let lang = select_text(item, "p").map(|v| v.split('-').next().unwrap_or("").trim().to_string()).unwrap_or_default();
        out.push((url, lang, server));
    }
    if out.is_empty() {
        out.extend(iframe_sources(body).into_iter().map(|url| {
            let server = server_name(&url);
            (url, String::new(), server)
        }));
    }
    out
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_remote(embed, referer);
    if embed.contains(".m3u8") { return parse_hls(&embed, name, referer, request); }
    let body = fetch(&embed, "", referer);
    if let Some(src) = first_media_url(&body).map(|src| absolute_remote(&src, &embed)) {
        if src.contains(".m3u8") { parse_hls(&src, name, &embed, request) } else { vec![stream(&src, name, "direct", &embed, false)] }
    } else {
        vec![external_stream(&embed, name, referer)]
    }
}
fn first_media_url(body: &str) -> Option<String> {
    [r#"file\s*:\s*["']([^"']+)"#, r#"src\s*:\s*["']([^"']+)"#, r#"<source[^>]+src=["']([^"']+)"#, r#"https?://[^\s'"\\]+\.m3u8[^\s'"\\]*"#]
        .into_iter().find_map(|p| Regex::new(p).ok()?.captures(body).and_then(|c| c.get(1).or_else(|| c.get(0))).map(|m| m.as_str().replace("\\/", "/")))
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
    let server = pref(request, "preferred_server", "VidHide").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080p");
    let lang = pref(request, "preferred_language", "Latino");
    streams.sort_by_key(|s| {
        let value = s.name.clone().or_else(|| s.quality.clone()).unwrap_or_default();
        let lower = value.to_ascii_lowercase();
        (lower.contains(&lang.to_ascii_lowercase()), lower.contains(&server), value.contains(quality.trim_end_matches('p')), quality_rank(&value))
    });
    streams.reverse();
}

fn selector(input: &str) -> Selector { Selector::parse(input).unwrap() }
fn attr(el: &ElementRef<'_>, name: &str) -> Option<String> { el.value().attr(name).map(ToString::to_string) }
fn select_text_doc(doc: &Html, sel: &str) -> Option<String> { doc.select(&selector(sel)).next().map(text).filter(|v| !v.is_empty()) }
fn select_texts_doc(doc: &Html, sel: &str) -> Vec<String> { doc.select(&selector(sel)).map(text).filter(|v| !v.is_empty()).collect() }
fn select_attr_doc(doc: &Html, sel: &str, name: &str) -> Option<String> { doc.select(&selector(sel)).next().and_then(|el| el.value().attr(name)).map(ToString::to_string) }
fn select_text(el: ElementRef<'_>, sel: &str) -> Option<String> { el.select(&selector(sel)).next().map(text).filter(|v| !v.is_empty()) }
fn select_attr(el: ElementRef<'_>, sel: &str, name: &str) -> Option<String> { el.select(&selector(sel)).next().and_then(|el| el.value().attr(name)).map(ToString::to_string) }
fn text(el: ElementRef<'_>) -> String { el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ") }
fn image_url(el: ElementRef<'_>) -> Option<String> {
    el.select(&selector("img")).next().and_then(|img| img.value().attr("data-src").or_else(|| img.value().attr("src")).map(absolute_url))
}
fn iframe_src(body: &str) -> Option<String> { iframe_sources(body).into_iter().next() }
fn iframe_sources(body: &str) -> Vec<String> {
    Regex::new(r#"<iframe[^>]+(?:src|data-src)=['"]([^'"]+)"#).unwrap().captures_iter(body).filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string())).collect()
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
fn pref(request: &Value, key: &str, default: &str) -> String { request.get("preferences").and_then(|p| p.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).unwrap_or(default).to_string() }
fn referer_headers(referer: &str) -> Context { let mut h = Context::new(); h.insert("Referer".to_string(), referer.to_string()); h }
fn first_number(input: &str) -> Option<f32> { Regex::new(r#"\d+(?:\.\d+)?"#).unwrap().find(input).and_then(|m| m.as_str().parse().ok()) }
fn quality_rank(input: &str) -> i32 { Regex::new(r#"(\d+)"#).unwrap().captures(input).and_then(|c| c.get(1)).and_then(|m| m.as_str().parse().ok()).unwrap_or(0) }
fn trim_float(value: f32) -> String { if value.fract() == 0.0 { format!("{}", value as i32) } else { value.to_string() } }
fn server_name(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if lower.contains("streamtape") || lower.contains("stape") { "StreamTape" } else if lower.contains("filemoon") { "FileMoon" } else if lower.contains("hexload") { "HexLoad" } else if lower.contains("uqload") { "Uqload" } else if lower.contains("wish") { "StreamWish" } else if lower.contains("vidhide") || lower.contains("streamvid") || lower.contains("hgcloud") || lower.contains("hglink") { "VidHide" } else { input.split("://").nth(1).unwrap_or(input).split('/').next().unwrap_or("External") }.to_string()
}
fn title_from_path(path: &str) -> String { path.trim_matches('/').rsplit('/').next().unwrap_or("VerPelisTop").replace('-', " ") }

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div id="featured-titles"><article><div class="poster"><a href="/peliculas/sample"><img src="/sample.jpg" alt="Sample"></a></div></article></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="sheader"><div class="poster"><img src="/sample.jpg"></div><div class="data"><h1>Sample</h1><div class="sgeneros"><a>accion</a></div></div></div><div class="wp-content"><p>Sample description.</p></div><ul id="playeroptionsul"><li class="dooplay_player_option" data-type="movie" data-post="1" data-nume="1"></li></ul>"#;
const WATCH_FIXTURE: &str = DETAILS_FIXTURE;
