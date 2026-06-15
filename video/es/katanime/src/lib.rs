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
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Katanime = Katanime;
const BASE_URL: &str = "https://katanime.net";

struct Katanime;

impl VideoSource for Katanime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/animes?fecha=2026&p={page}")
        } else {
            format!("{BASE_URL}/populares?p={page}")
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged { entries: vec![fetch_details(&path)], has_next_page: false });
        }
        let page = page(&request);
        let params = filter_query(&request);
        let target = if !query.is_empty() {
            format!("{BASE_URL}/buscar?q={}&p={page}", url::query_escape(query))
        } else if !params.is_empty() {
            format!("{BASE_URL}/animes{params}&p={page}")
        } else {
            format!("{BASE_URL}/populares?p={page}")
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, DETAILS_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let pagination_url = attr_doc(&doc, "._pagination", "data-url").map(|v| absolute_url(&v)).unwrap_or_default();
        let token = attr_doc(&doc, "meta[name=\"csrf-token\"]", "content").unwrap_or_default();
        if pagination_url.is_empty() {
            return Ok(Vec::new());
        }
        let first = post_episodes(&pagination_url, &token, 1, &referer);
        let mut episodes = first.ep.data.iter().filter_map(|ep| ep_to_episode(ep)).collect::<Vec<_>>();
        let pages = first.ep.last_page.unwrap_or_else(|| {
            let total = first.ep.total.unwrap_or(1) as f64;
            let per = first.ep.per_page.unwrap_or(1).max(1) as f64;
            (total / per).ceil() as u64
        }).max(1);
        for current in 2..=pages {
            episodes.extend(post_episodes(&pagination_url, &token, current, &referer).ep.data.iter().filter_map(ep_to_episode));
        }
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/ver/sample-1".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        Ok(doc.select(&selector("[data-player]:not([data-player-name=\"Mega\"])"))
            .filter_map(|el| {
                let player = attr(&el, "data-player")?;
                let name = attr(&el, "data-player-name").unwrap_or_else(|| text(el).if_empty("External".to_string()));
                let player_url = format!("{BASE_URL}/reproductor?url={}", url::query_escape(&player));
                Some(VideoHoster { key: format!("{name}|{player_url}|{referer}"), name, url: Some(player_url), lazy: true, video_count: Some(1), ..VideoHoster::default() })
            })
            .collect())
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = raw_key(&request, "hoster") else { return Ok(Vec::new()); };
        let mut parts = key.splitn(3, '|');
        let name = parts.next().unwrap_or("External");
        let player_url = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(BASE_URL);
        let mut streams = resolve_player(player_url, name, referer, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut out = Vec::new();
        for hoster in self.hosters(request.clone())? {
            let mut streams = self.resolve_hoster(json!({"hoster":{"key":hoster.key},"preferences":request.get("preferences").cloned().unwrap_or(Value::Null)}))?;
            for stream in &mut streams { stream.hoster = Some(hoster.clone()); }
            out.extend(streams);
        }
        sort_streams(&mut out, &request);
        Ok(out)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection { id: "popular".to_string(), title: "Populares".to_string(), style: Some(HomeSectionStyle::Featured), entries: popular.entries, has_more: popular.has_next_page, ..HomeSection::default() },
            HomeSection { id: "latest".to_string(), title: "Ultimos episodios".to_string(), entries: latest.entries, has_more: latest.has_next_page, ..HomeSection::default() },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> { Ok(request_key(&request, "item").map(|p| absolute_url(&p))) }
    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> { Ok(request_key(&request, "episode").map(|p| absolute_url(&p))) }
    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(path) = path_from_url(input) {
            if path.contains("/ver/") || path.contains("/episodio") {
                return Ok(Some(UrlResolveResult { episode: Some(json!({"key":path,"url":input,"language":"es"})), url: Some(input.to_string()), ..UrlResolveResult::default() }));
            }
            return Ok(Some(UrlResolveResult { item: Some(fetch_details(&path)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn parse_cards(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc.select(&selector("#article-div .full > a, .full > a")).filter_map(card).collect(),
        has_next_page: doc.select(&selector(".pagination .active ~ li:not(.disabled), a[rel=next]")).next().is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href")?;
    let img = el.select(&selector("img")).next();
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title: img.as_ref().and_then(|i| attr(i, "alt")).unwrap_or_else(|| title_from_path(&path)),
        cover: img.as_ref().and_then(image_url).map(|v| absolute_url(&v)),
        url: Some(absolute_url(&path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: false,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    CatalogItem {
        key: path_key(path),
        title: text_doc(&doc, ".comics-title, h1").unwrap_or_else(|| title_from_path(path)),
        cover: attr_doc(&doc, ".anime-cover img, .thumb img, img", "src").map(|v| absolute_url(&v)),
        url: Some(absolute_url(path)),
        description: text_doc(&doc, "#sinopsis p, .sinopsis p"),
        tags: doc.select(&selector(".anime-genres a")).map(text).filter(|v| !v.is_empty()).collect(),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&text_doc(&doc, ".details-by #estado, #estado").unwrap_or_default()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn resolve_player(player_url: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = fetch(player_url, "", referer);
    if let Some(src) = first_media_url(&body).map(|v| absolute_remote(&v, player_url)) {
        if src.contains(".m3u8") { return parse_hls(&src, name, player_url, request); }
        return vec![stream(&src, name, "direct", player_url, false)];
    }
    vec![external_stream(player_url, name, referer)]
}

fn first_media_url(body: &str) -> Option<String> {
    [r#"file\s*:\s*["']([^"']+)["']"#, r#"url:\s*["']([^"']+)["']"#, r#"src\s*:\s*["']([^"']+)["']"#, r#"<source[^>]+src=["']([^"']+)["']"#]
        .into_iter().find_map(|p| Regex::new(p).ok()?.captures(body)?.get(1).map(|m| m.as_str().replace("\\/", "/")))
}

fn parse_hls(master: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = client(referer).get(master).referer(referer).send_text().unwrap_or_default();
    let mut out = Vec::new();
    let mut q = "auto".to_string();
    for line in body.lines() {
        if line.starts_with("#EXT-X-STREAM-INF") {
            q = line.split("RESOLUTION=").nth(1).and_then(|v| v.split('x').nth(1)).and_then(|v| v.split(',').next()).map(|v| format!("{v}p")).unwrap_or_else(|| "auto".to_string());
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            out.push(stream(&absolute_remote(line.trim(), master), name, &q, referer, true));
        }
    }
    if out.is_empty() { out.push(stream(master, name, "auto", referer, true)); }
    sort_streams(&mut out, request);
    out
}

fn stream(target: &str, name: &str, quality: &str, referer: &str, hls: bool) -> VideoStream {
    VideoStream { url: target.to_string(), name: Some(format!("{name} {quality}")), quality: Some(quality.to_string()), format: Some(if hls { "hls" } else { "mp4" }.to_string()), is_hls: hls, stream_kind: Some(if hls { VideoStreamKind::Hls } else { VideoStreamKind::Direct }), headers: referer_headers(referer), initialized: true, ..VideoStream::default() }
}
fn external_stream(target: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream { url: target.to_string(), name: Some(format!("{name} External")), quality: Some(name.to_string()), stream_kind: Some(VideoStreamKind::External), headers: referer_headers(referer), initialized: true, ..VideoStream::default() }
}
fn post_episodes(url: &str, token: &str, page: u64, referer: &str) -> EpisodeList {
    let body = client(referer).post(url).xhr().referer(referer).header("Origin", BASE_URL).header("Content-Type", "application/x-www-form-urlencoded").body(format!("_token={}&pagina={page}", url::query_escape(token))).send_text().unwrap_or_default();
    serde_json::from_str(&body).unwrap_or_default()
}
fn ep_to_episode(ep: &EpisodeData) -> Option<VideoEpisode> {
    let url = ep.url.as_ref()?;
    let num = ep.numero.as_deref().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
    let key = path_key(url);
    Some(VideoEpisode { key: key.clone(), title: Some(ep.numero.as_ref().map(|n| format!("Episodio {n}")).unwrap_or_else(|| "Episodio".to_string())), episode_number: Some(num), url: Some(absolute_url(&key)), language: Some("es".to_string()), ..VideoEpisode::default() })
}

fn client(referer: &str) -> HttpClient { HttpClient::browser().with_desktop_user_agent().with_referer(referer).with_cookies_for(BASE_URL).with_webview_challenge_fallback() }
fn fetch(target: &str, fixture: &str, referer: &str) -> String { client(referer).get(target).browser_document().referer(referer).send_text().unwrap_or_else(|_| fixture.to_string()) }
fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let server = pref(request, "preferred_server", "VidGuard").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|s| { let n = s.name.clone().unwrap_or_default().to_ascii_lowercase(); (n.contains(&server), n.contains(&quality), quality_rank(&n)) });
    streams.reverse();
}
fn filter_query(request: &Value) -> String {
    let mut pairs = Vec::new();
    for key in ["categoria", "idioma", "fecha"] {
        if let Some(value) = filter(request, key).filter(|v| !v.is_empty()) { pairs.push(format!("{key}={}", url::query_escape(&value))); }
    }
    if let Some(value) = filter(request, "genero").filter(|v| !v.is_empty()) { pairs.push(format!("genero={}", url::query_escape(&value))); }
    if pairs.is_empty() { String::new() } else { format!("?{}", pairs.join("&")) }
}
fn selector(input: &str) -> Selector { Selector::parse(input).unwrap() }
fn text_doc(doc: &Html, sel: &str) -> Option<String> { doc.select(&selector(sel)).next().map(text).filter(|v| !v.is_empty()) }
fn attr_doc(doc: &Html, sel: &str, name: &str) -> Option<String> { doc.select(&selector(sel)).next().and_then(|e| attr(&e, name)) }
fn attr(el: &ElementRef<'_>, name: &str) -> Option<String> { el.value().attr(name).map(ToString::to_string) }
fn text(el: ElementRef<'_>) -> String { el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ") }
fn image_url(el: &ElementRef<'_>) -> Option<String> { ["data-src", "data-lazy-src", "srcset", "src"].into_iter().filter_map(|name| attr(el, name)).find(|v| !v.contains("data:image/")).map(|v| v.split_whitespace().next().unwrap_or(&v).to_string()) }
trait IfEmpty { fn if_empty(self, fallback: String) -> String; }
impl IfEmpty for String { fn if_empty(self, fallback: String) -> String { if self.is_empty() { fallback } else { self } } }
fn referer_headers(referer: &str) -> Context { let mut h = Context::new(); h.insert("Referer".to_string(), referer.to_string()); h }
fn absolute_url(input: &str) -> String { absolute_remote(input, BASE_URL) }
fn absolute_remote(input: &str, base: &str) -> String { let t = input.trim().replace("\\/", "/"); if t.starts_with("http") { t } else if let Some(rest) = t.strip_prefix("//") { format!("https://{rest}") } else { url::join_url(base, &t) } }
fn path_from_url(input: &str) -> Option<String> { input.strip_prefix(BASE_URL).filter(|p| p.starts_with('/')).map(path_key) }
fn path_key(input: &str) -> String { format!("/{}", input.strip_prefix(BASE_URL).unwrap_or(input).split(['?', '#']).next().unwrap_or(input).trim_matches('/')) }
fn raw_key(request: &Value, field: &str) -> Option<String> { request.get(field).and_then(|v| v.get("key").or_else(|| v.get("url")).and_then(Value::as_str).or_else(|| v.as_str())).or_else(|| request.get("key").and_then(Value::as_str)).map(ToString::to_string) }
fn request_key(request: &Value, field: &str) -> Option<String> { raw_key(request, field).map(|v| path_key(&v)) }
fn page(request: &Value) -> u64 { request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1) }
fn listing(request: &Value) -> &str { request.get("listing").or_else(|| request.get("listingId")).and_then(Value::as_str).unwrap_or("popular") }
fn with_listing(request: &Value, listing: &str) -> Value { json!({"listing":listing,"preferences":request.get("preferences").cloned().unwrap_or(Value::Null)}) }
fn filter(request: &Value, key: &str) -> Option<String> { request.get("filters").and_then(|f| f.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).map(ToString::to_string) }
fn pref(request: &Value, key: &str, default: &str) -> String { request.get("preferences").and_then(|p| p.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).unwrap_or(default).to_string() }
fn quality_rank(input: &str) -> i32 { Regex::new(r#"(\d+)"#).unwrap().captures(input).and_then(|c| c.get(1)).and_then(|m| m.as_str().parse().ok()).unwrap_or(0) }
fn title_from_path(path: &str) -> String { path.trim_matches('/').rsplit('/').next().unwrap_or("Katanime").replace('-', " ") }
fn parse_status(input: &str) -> ItemStatus { if input.contains("Finalizado") { ItemStatus::Completed } else if input.contains("Emision") || input.contains("Emisión") { ItemStatus::Ongoing } else { ItemStatus::Unknown } }

#[derive(Default, Deserialize)]
struct EpisodeList { #[serde(default)] ep: EpisodePage }
#[derive(Default, Deserialize)]
struct EpisodePage { #[serde(default)] data: Vec<EpisodeData>, last_page: Option<u64>, per_page: Option<u64>, total: Option<u64> }
#[derive(Default, Deserialize)]
struct EpisodeData { numero: Option<String>, url: Option<String> }

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div id="article-div"><div class="full"><a href="/anime/sample"><img alt="Sample" src="/sample.jpg"></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<meta name="csrf-token" content="token"><h1 class="comics-title">Sample</h1><div id="sinopsis"><p>Sample description.</p></div><div class="anime-genres"><a>Accion</a></div><div class="details-by"><span id="estado">Finalizado</span></div><div class="_pagination" data-url="/ajax/episodes"></div>"#;
const WATCH_FIXTURE: &str = r#"<button data-player="https://example.invalid/embed" data-player-name="VidGuard">VidGuard</button>"#;
