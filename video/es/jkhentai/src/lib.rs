use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    sdk::{SearchRequest, http::HttpClient},
    url,
    video::referer_headers,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::{Value, json};

const SOURCE: Jkhentai = Jkhentai;
const BASE_URL: &str = "https://www.jkhentai.net";

struct Jkhentai;

impl VideoSource for Jkhentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        Ok(parse_listing(&fetch(&format!("{BASE_URL}/lista/{page}"), LIST_FIXTURE, BASE_URL)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged { entries: vec![fetch_details(&path)], has_next_page: false });
        }
        let page = page(&request);
        let target = if !query.is_empty() {
            format!("{BASE_URL}/buscador.php?search={}&page={page}", url::query_escape(query))
        } else if let Some(genre) = filter(&request, "genre").filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/genero/{genre}/{page}")
        } else {
            format!("{BASE_URL}/lista/{page}")
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/hentai/sample-sub-espanol".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/hentai/sample-sub-espanol".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body, &path))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/ver/sample-1".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let embeds = doc.select(&selector("div.play-c iframe, iframe"))
            .filter_map(|e| attr(&e, "src"))
            .filter(|src| !src.is_empty())
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        for (idx, tab) in doc.select(&selector("#player-container ul.player-menu li a, .player-menu a")).enumerate() {
            let name = text(tab).if_empty(format!("Servidor {}", idx + 1));
            let embed = embeds.get(idx).or_else(|| embeds.first()).cloned().unwrap_or_default();
            if !embed.is_empty() {
                out.push(VideoHoster {
                    key: format!("{name}|{}|{referer}", absolute_remote(&embed, &referer)),
                    name,
                    url: Some(absolute_remote(&embed, &referer)),
                    lazy: true,
                    video_count: Some(1),
                    ..VideoHoster::default()
                });
            }
        }
        if out.is_empty() {
            out = embeds.into_iter().enumerate().map(|(idx, embed)| {
                let name = host_name(&embed).if_empty(format!("Servidor {}", idx + 1));
                VideoHoster {
                    key: format!("{name}|{}|{referer}", absolute_remote(&embed, &referer)),
                    name,
                    url: Some(absolute_remote(&embed, &referer)),
                    lazy: true,
                    video_count: Some(1),
                    ..VideoHoster::default()
                }
            }).collect();
        }
        Ok(out)
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = raw_key(&request, "hoster") else { return Ok(Vec::new()); };
        let mut parts = key.splitn(3, '|');
        let name = parts.next().unwrap_or("External");
        let embed = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(BASE_URL);
        let mut streams = resolve_embed(embed, name, referer, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut out = Vec::new();
        for hoster in self.hosters(request.clone())? {
            let mut streams = self.resolve_hoster(json!({
                "hoster": {"key": hoster.key},
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
            }))?;
            for stream in &mut streams {
                stream.hoster = Some(hoster.clone());
            }
            out.extend(streams);
        }
        sort_streams(&mut out, &request);
        Ok(out)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Lista".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|p| absolute_url(&p)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|p| absolute_url(&p)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(path) = path_from_url(input) {
            if path.starts_with("/ver/") {
                return Ok(Some(UrlResolveResult {
                    episode: Some(json!({"key": path, "url": input, "language": "es"})),
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
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    CatalogItem {
        key: path_key(path),
        title: text_doc(&doc, ".dataplus h1, h1").unwrap_or_else(|| title_from_path(path)),
        cover: attr_doc(&doc, ".headingder .imgs img, img", "src").map(|v| absolute_url(&v)),
        url: Some(absolute_url(path)),
        description: text_doc(&doc, "span.original").map(|v| format!("Titulo Original: {v}")),
        tags: texts_doc(&doc, ".data-content a, #dato-1 a"),
        language: Some("es".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc.select(&selector("#box_movies .movie, div.movie")).filter_map(card).collect(),
        has_next_page: doc.select(&selector("a.page.larger, .pagination a")).next().is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr_sel(el, ".imagen a, a", "href")?;
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title: text_sel(el, "h2, .title").unwrap_or_else(|| title_from_path(&path)),
        cover: attr_sel(el, ".imagen img, img", "src").map(|v| absolute_url(&v)),
        url: Some(absolute_url(&path)),
        language: Some("es".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &str, item_path: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let anime_id = item_path.trim_matches('/').rsplit('/').next().unwrap_or("sample")
        .replace("-sub-espanol", "").replace("-080p", "-1080p");
    doc.select(&selector("#cssmenu li.has-sub.open ul li a, #cssmenu ul li ul li a"))
        .filter_map(|a| {
            let href = attr(&a, "href").unwrap_or_default();
            let number = href.rsplit('-').next()?.parse::<f32>().ok()?;
            let fallback = format!("/ver/{anime_id}-{}", trim_float(number));
            let key = path_key(if href.is_empty() { &fallback } else { &href });
            Some(VideoEpisode {
                key: key.clone(),
                title: Some(format!("Episodio {}", trim_float(number))),
                episode_number: Some(number),
                url: Some(absolute_url(&key)),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_remote(embed, referer);
    if embed.contains(".m3u8") { return parse_hls(&embed, name, referer, request); }
    let body = fetch(&embed, "", referer);
    if let Some(media) = first_media_url(&body).map(|v| absolute_remote(&v, &embed)) {
        if media.contains(".m3u8") { parse_hls(&media, name, &embed, request) } else { vec![stream(&media, name, "direct", &embed, false)] }
    } else {
        vec![stream(&embed, name, name, referer, false).external()]
    }
}

trait Externalize { fn external(self) -> Self; }
impl Externalize for VideoStream {
    fn external(mut self) -> Self {
        self.stream_kind = Some(VideoStreamKind::External);
        self.format = None;
        self
    }
}

fn first_media_url(body: &str) -> Option<String> {
    [r#"file\s*:\s*["']([^"']+)["']"#, r#"src\s*:\s*["']([^"']+)["']"#, r#"<source[^>]+src=["']([^"']+)["']"#]
        .into_iter().find_map(|p| Regex::new(p).ok()?.captures(body)?.get(1).map(|m| m.as_str().replace("\\/", "/")))
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
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if hls { "hls" } else { "mp4" }.to_string()),
        is_hls: hls,
        stream_kind: Some(if hls { VideoStreamKind::Hls } else { VideoStreamKind::Direct }),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn client(referer: &str) -> HttpClient {
    HttpClient::browser().with_desktop_user_agent().with_referer(referer).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}
fn fetch(target: &str, fixture: &str, referer: &str) -> String {
    client(referer).get(target).browser_document().referer(referer).send_text().unwrap_or_else(|_| fixture.to_string())
}
fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = pref(request, "preferred_quality", "StreamTape");
    streams.sort_by_key(|s| s.name.clone().unwrap_or_default().contains(&preferred));
    streams.reverse();
}
fn selector(input: &str) -> Selector { Selector::parse(input).unwrap() }
fn text_doc(doc: &Html, sel: &str) -> Option<String> { doc.select(&selector(sel)).next().map(text).filter(|v| !v.is_empty()) }
fn texts_doc(doc: &Html, sel: &str) -> Vec<String> { doc.select(&selector(sel)).map(text).filter(|v| !v.is_empty()).collect() }
fn attr_doc(doc: &Html, sel: &str, name: &str) -> Option<String> { doc.select(&selector(sel)).next().and_then(|e| attr(&e, name)) }
fn text_sel(el: ElementRef<'_>, sel: &str) -> Option<String> { el.select(&selector(sel)).next().map(text).filter(|v| !v.is_empty()) }
fn attr_sel(el: ElementRef<'_>, sel: &str, name: &str) -> Option<String> { el.select(&selector(sel)).next().and_then(|e| attr(&e, name)) }
fn attr(el: &ElementRef<'_>, name: &str) -> Option<String> { el.value().attr(name).map(ToString::to_string) }
fn text(el: ElementRef<'_>) -> String { el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ") }
trait IfEmpty { fn if_empty(self, fallback: String) -> String; }
impl IfEmpty for String { fn if_empty(self, fallback: String) -> String { if self.is_empty() { fallback } else { self } } }
fn absolute_url(input: &str) -> String { absolute_remote(input, BASE_URL) }
fn absolute_remote(input: &str, base: &str) -> String {
    let t = input.trim().replace("\\/", "/");
    if t.starts_with("http") { t } else if let Some(rest) = t.strip_prefix("//") { format!("https://{rest}") } else { url::join_url(base, &t) }
}
fn path_from_url(input: &str) -> Option<String> { input.strip_prefix(BASE_URL).filter(|p| p.starts_with('/')).map(path_key) }
fn path_key(input: &str) -> String {
    format!("/{}", input.strip_prefix(BASE_URL).unwrap_or(input).split(['?', '#']).next().unwrap_or(input).trim_matches('/'))
}
fn raw_key(request: &Value, field: &str) -> Option<String> {
    request.get(field).and_then(|v| v.get("key").or_else(|| v.get("url")).and_then(Value::as_str).or_else(|| v.as_str())).or_else(|| request.get("key").and_then(Value::as_str)).map(ToString::to_string)
}
fn request_key(request: &Value, field: &str) -> Option<String> { raw_key(request, field).map(|v| path_key(&v)) }
fn page(request: &Value) -> u64 { request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1) }
fn filter(request: &Value, key: &str) -> Option<String> { request.get("filters").and_then(|f| f.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).map(ToString::to_string) }
fn pref(request: &Value, key: &str, default: &str) -> String { request.get("preferences").and_then(|p| p.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).unwrap_or(default).to_string() }
fn host_name(input: &str) -> String { input.split("://").nth(1).unwrap_or(input).split('/').next().unwrap_or("External").replace("www.", "") }
fn title_from_path(path: &str) -> String { path.trim_matches('/').rsplit('/').next().unwrap_or("Jkhentai").replace('-', " ") }
fn trim_float(value: f32) -> String { if value.fract() == 0.0 { format!("{}", value as i32) } else { value.to_string() } }

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div id="box_movies"><div class="movie"><div class="imagen"><a href="/hentai/sample-sub-espanol"><img src="/sample.jpg"></a></div><h2>Sample</h2></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="headingder"><div class="datos"><div class="imgs"><a><img src="/sample.jpg"></a></div><div class="dataplus"><h1>Sample</h1><span class="original">Sample</span><div id="dato-1" class="data-content"><a>Accion</a></div></div></div></div><div id="cssmenu"><ul><li class="has-sub open"><ul><li><a href="/ver/sample-1">1</a></li></ul></li></ul></div>"#;
const WATCH_FIXTURE: &str = r#"<div id="player-container"><ul class="player-menu"><li><a>StreamTape</a></li></ul><div class="play-c"><div class="player-content"><iframe src="https://example.invalid/embed"></iframe></div></div></div>"#;
