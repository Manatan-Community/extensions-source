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

const SOURCE: VerSeriesOnline = VerSeriesOnline;
const BASE_URL: &str = "https://www.verseriesonline.net";

struct VerSeriesOnline;

impl VideoSource for VerSeriesOnline {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_cards(&fetch(&format!("{BASE_URL}/series-online/page/{}", page(&request)), LIST_FIXTURE, BASE_URL)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged { entries: vec![fetch_details(&path)], has_next_page: false });
        }
        let page = page(&request);
        let target = if !query.is_empty() {
            format!("{BASE_URL}/recherche?q={}&page={page}", url::query_escape(query))
        } else if let Some(year) = filter(&request, "year").filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/series-online/ano/{year}/page/{page}")
        } else if let Some(genre) = filter(&request, "genre").filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/series-online/genero/{genre}/page/{page}")
        } else {
            format!("{BASE_URL}/series-online/page/{page}")
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/serie/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/serie/sample".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body, &absolute_url(&path)))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/episode/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let mut streams = Vec::new();
        for (embed, language, server) in parse_hash_embeds(&body, &referer) {
            let label = [language.as_str(), server.as_str()].into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join(" ");
            streams.extend(resolve_embed(&embed, &label, &referer, &request));
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection { id: "popular".to_string(), title: "Series online".to_string(), style: Some(HomeSectionStyle::Featured), entries: popular.entries, has_more: popular.has_next_page, ..HomeSection::default() }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> { Ok(request_key(&request, "item").map(|path| absolute_url(&path))) }
    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> { Ok(request_key(&request, "episode").map(|path| absolute_url(&path))) }
    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(path) = path_from_url(input) {
            if path.contains("/episode/") || path.contains("/capitulo/") {
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
        entries: doc.select(&selector("div.short.gridder-list, .short.gridder-list")).filter_map(card).collect(),
        has_next_page: doc.select(&selector(".navigation a:last-of-type, .pagination a")).next().is_some(),
    }
}
fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "a.short_img, a[href]", "href")?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_text(el, "div.short_title a, .short_title a").unwrap_or_else(|| title_from_path(&key)),
        cover: select_attr(el, "a.short_img img, img", "data-src").or_else(|| select_attr(el, "img", "src")).map(|src| absolute_url(&src)),
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
        title: select_text_doc(&doc, "h1, .full_content h1").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, "img.lazy-loaded, .full_img img, img", "data-src").or_else(|| select_attr_doc(&doc, "img", "src")).map(|src| absolute_url(&src)),
        description: select_text_doc(&doc, "div.full_content-desc p span, div.full_content-desc p, .full_content-desc"),
        tags: select_texts_doc(&doc, "ul#full_info li a"),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}
fn parse_episodes(body: &str, item_url: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let mut season_urls = doc.select(&selector("div.floats a")).filter_map(|a| attr(&a, "href")).collect::<Vec<_>>();
    if season_urls.is_empty() {
        season_urls.push(item_url.to_string());
    }
    season_urls.sort();
    season_urls.dedup();
    let mut out = Vec::new();
    for season_url in season_urls {
        let season_no = Regex::new(r#"temporada-(\d+)"#).unwrap().captures(&season_url).and_then(|c| c.get(1)).and_then(|m| m.as_str().parse::<f32>().ok()).unwrap_or(1.0);
        let season_body = if season_url == item_url { body.to_string() } else { fetch(&absolute_remote(&season_url, BASE_URL), "", item_url) };
        let season_doc = Html::parse_document(&season_body);
        for ep in season_doc.select(&selector("#dle-content > article > div > div:nth-child(3) > div > div > a, div.seasontab div.floats a.th-hover, a.th-hover")) {
            let href = attr(&ep, "href").unwrap_or_default();
            if href.is_empty() { continue; }
            let name = select_text(ep, "span.name").unwrap_or_else(|| text(ep));
            let ep_no = Regex::new(r#"Capitulo\s+(\d+)|Capítulo\s+(\d+)|(\d+)"#).unwrap().captures(&name).and_then(|c| c.iter().skip(1).flatten().next()).and_then(|m| m.as_str().parse::<f32>().ok()).unwrap_or(0.0);
            let key = path_key(&href);
            out.push(VideoEpisode { key: key.clone(), title: Some(format!("Temporada {} - {name}", trim_float(season_no))), episode_number: Some(ep_no), season_number: Some(season_no), url: Some(absolute_url(&key)), language: Some("es".to_string()), ..VideoEpisode::default() });
        }
    }
    out
}

fn parse_hash_embeds(body: &str, referer: &str) -> Vec<(String, String, String)> {
    let doc = Html::parse_document(body);
    let token = select_attr_doc(&doc, r#"meta[name="csrf-token"]"#, "content").unwrap_or_default();
    if token.is_empty() { return Vec::new(); }
    let mut out = Vec::new();
    for div in doc.select(&selector(".undervideo .player-list li div.lien, .player-list div.lien")) {
        let hash = attr(&div, "data-hash").unwrap_or_default();
        if hash.is_empty() { continue; }
        let server = select_text(div, ".serv").unwrap_or_else(|| "External".to_string());
        let language = language_from_element(div, &server);
        let response = client(referer)
            .post(format!("{BASE_URL}/hashembedlink"))
            .xhr()
            .referer(referer)
            .origin(BASE_URL)
            .header("X-CSRF-TOKEN", token.as_str())
            .form(&[("hash", hash.as_str()), ("_token", token.as_str())])
            .send_text()
            .unwrap_or_default();
        let value: Value = serde_json::from_str(&response).unwrap_or(Value::Null);
        if let Some(link) = value.get("link").and_then(Value::as_str).filter(|v| !v.is_empty()) {
            out.push((link.to_string(), language, server));
        }
    }
    out
}

fn language_from_element(el: ElementRef<'_>, server: &str) -> String {
    let raw = format!("{} {}", server, select_attr(el, "img", "src").unwrap_or_default()).to_ascii_lowercase();
    if raw.contains("lat") || raw.contains("latino") {
        "Latino"
    } else if raw.contains("esp") || raw.contains("castellano") || raw.contains("espanol") {
        "Castellano"
    } else if raw.contains("subesp") || raw.contains("sub") || raw.contains("vose") {
        "VOSE"
    } else {
        ""
    }.to_string()
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
    let server = pref(request, "preferred_server", "Voe").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    let lang = pref(request, "preferred_language", "Latino").to_ascii_lowercase();
    streams.sort_by_key(|s| {
        let value = s.name.clone().or_else(|| s.quality.clone()).unwrap_or_default();
        let lower = value.to_ascii_lowercase();
        (lower.contains(&lang), lower.contains(&server), value.contains(&quality), quality_rank(&value))
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
fn filter(request: &Value, key: &str) -> Option<String> { request.get("filters").and_then(|f| f.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).map(ToString::to_string) }
fn pref(request: &Value, key: &str, default: &str) -> String { request.get("preferences").and_then(|p| p.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).unwrap_or(default).to_string() }
fn referer_headers(referer: &str) -> Context { let mut h = Context::new(); h.insert("Referer".to_string(), referer.to_string()); h }
fn quality_rank(input: &str) -> i32 { Regex::new(r#"(\d+)"#).unwrap().captures(input).and_then(|c| c.get(1)).and_then(|m| m.as_str().parse().ok()).unwrap_or(0) }
fn trim_float(value: f32) -> String { if value.fract() == 0.0 { format!("{}", value as i32) } else { value.to_string() } }
fn title_from_path(path: &str) -> String { path.trim_matches('/').rsplit('/').next().unwrap_or("VerSeriesOnline").replace('-', " ") }

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="short gridder-list"><a class="short_img" href="/serie/sample"><img data-src="/sample.jpg"></a><div class="short_title"><a>Sample</a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><img class="lazy-loaded" data-src="/sample.jpg"><div class="full_content-desc"><p><span>Sample description.</span></p></div><div class="floats"><a href="/serie/sample/temporada-1">Temporada 1</a></div>"#;
const WATCH_FIXTURE: &str = r#"<meta name="csrf-token" content="token"><div class="undervideo"><ul class="player-list"><li><div class="lien" data-hash="abc"><span class="serv">Voe Latino</span><img src="/lat.png"></div></li></ul></div>"#;
