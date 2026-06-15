use base64::{Engine as _, engine::general_purpose};
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

const SOURCE: Jkanime = Jkanime;
const BASE_URL: &str = "https://jkanime.net";

struct Jkanime;

impl VideoSource for Jkanime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        if listing(&request) == "latest" {
            return Ok(parse_home(&fetch(BASE_URL, HOME_FIXTURE, BASE_URL)));
        }
        Ok(parse_directory(&fetch(&format!("{BASE_URL}/directorio?filtro=popularidad&p={page}"), DIRECTORY_FIXTURE, BASE_URL)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged { entries: vec![fetch_details(&path)], has_next_page: false });
        }
        let page = page(&request);
        let target = if let Some(day) = filter(&request, "day").filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/horario/#{day}")
        } else if !query.is_empty() {
            format!("{BASE_URL}/buscar/{}", query.replace(' ', "_"))
        } else {
            let params = filter_params(&request);
            format!("{BASE_URL}/directorio?{params}p={page}")
        };
        let body = fetch(&target, DIRECTORY_FIXTURE, BASE_URL);
        if target.contains("/buscar/") {
            Ok(parse_search(&body))
        } else if target.contains("/horario") {
            Ok(parse_schedule(&body, target.rsplit('#').next().unwrap_or_default()))
        } else {
            Ok(parse_directory(&body))
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, DETAILS_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let anime_id = attr_doc(&doc, "#guardar-anime", "data-anime").unwrap_or_default();
        if anime_id.is_empty() {
            return Ok(Vec::new());
        }
        let token = attr_doc(&doc, "meta[name=csrf-token]", "content").unwrap_or_default();
        let first = post_episodes(&anime_id, 1, &token, &referer);
        let mut episodes = first.data.iter().map(|ep| ep_to_episode(ep, &path)).collect::<Vec<_>>();
        if pref(&request, "pref_episodes_info", "0") == "0" {
            let start = first.to.saturating_add(1);
            let end = first.total.max(start);
            for num in start..=end {
                episodes.push(numbered_episode(num, &path));
            }
        } else {
            for current in 2..=first.last_page.max(1) {
                episodes.extend(post_episodes(&anime_id, current, &token, &referer).data.iter().map(|ep| ep_to_episode(ep, &path)));
            }
        }
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/sample/1".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        Ok(parse_hosters(&body, &referer))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = raw_key(&request, "hoster") else { return Ok(Vec::new()); };
        let mut parts = key.splitn(4, '|');
        let name = parts.next().unwrap_or("External");
        let lang = parts.next().unwrap_or("");
        let embed = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(BASE_URL);
        let label = format!("{lang} {name}").trim().to_string();
        let mut streams = resolve_embed(embed, &label, referer, &request);
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
            if episode_like(&path) {
                return Ok(Some(UrlResolveResult { episode: Some(json!({"key":path,"url":input,"language":"es"})), url: Some(input.to_string()), ..UrlResolveResult::default() }));
            }
            return Ok(Some(UrlResolveResult { item: Some(fetch_details(&path)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn parse_directory(body: &str) -> Paged<CatalogItem> {
    let data = Regex::new(r#"var\s+animes\s*=\s*(\{.*?\})\s*;"#).unwrap().captures(body).and_then(|c| c.get(1)).map(|m| m.as_str()).unwrap_or("");
    if let Ok(page) = serde_json::from_str::<AnimePage>(data) {
        return Paged {
            entries: page.data.into_iter().map(|a| CatalogItem {
                key: path_key(&a.url), title: a.title, cover: Some(absolute_url(&a.image)),
                url: Some(absolute_url(&a.url)), description: a.synopsis, authors: a.studios.into_iter().collect(),
                language: Some("es".to_string()), content_rating: Some("safe".to_string()),
                status: parse_status(a.estado.as_deref().unwrap_or_default()), initialized: false, ..CatalogItem::default()
            }).collect(),
            has_next_page: page.next_page_url.filter(|v| !v.is_empty()).is_some(),
        };
    }
    let doc = Html::parse_document(body);
    Paged {
        entries: doc.select(&selector(".anime__item, .custom_thumb_home")).filter_map(card).collect(),
        has_next_page: doc.select(&selector(".pagination .active ~ li:not(.disabled), a[rel=next]")).next().is_some(),
    }
}

fn parse_home(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged { entries: doc.select(&selector("div.trending_div div.custom_thumb_home a")).filter_map(|a| {
        let href = attr(&a, "href")?;
        let img = a.select(&selector("img")).next();
        Some(CatalogItem {
            key: path_key(&href),
            title: img.as_ref().and_then(|i| attr(i, "alt")).unwrap_or_else(|| title_from_path(&href)),
            cover: img.as_ref().and_then(|i| attr(i, "src")).map(|v| absolute_url(&v)),
            url: Some(absolute_url(&href)),
            language: Some("es".to_string()),
            content_rating: Some("safe".to_string()),
            ..CatalogItem::default()
        })
    }).collect(), has_next_page: true }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged { entries: doc.select(&selector("div.row.page_directorio div.anime__item, .anime__item")).filter_map(card).collect(), has_next_page: false }
}

fn parse_schedule(body: &str, day: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged { entries: doc.select(&selector("div.cajas div.boxx, .boxx")).filter_map(|e| {
        let href = attr_sel(e, "a", "href")?;
        Some(CatalogItem {
            key: path_key(&href),
            title: attr_sel(e, "img", "title").or_else(|| attr_sel(e, "img", "alt")).unwrap_or_else(|| title_from_path(&href)),
            cover: attr_sel(e, "img", "src").map(|v| absolute_url(&v)),
            url: Some(absolute_url(&href)),
            language: Some("es".to_string()),
            content_rating: Some("safe".to_string()),
            tags: vec![day.to_string()],
            ..CatalogItem::default()
        })
    }).collect(), has_next_page: false }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr_sel(el, "a[href]", "href")?;
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title: text_sel(el, ".anime__item__text a, h3, a").or_else(|| attr_sel(el, "img", "alt")).unwrap_or_else(|| title_from_path(&path)),
        cover: attr_sel(el, ".g-0", "data-setbg").or_else(|| attr_sel(el, "img", "src")).map(|v| absolute_url(&v)),
        url: Some(absolute_url(&path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    let mut status = ItemStatus::Unknown;
    let mut tags = Vec::new();
    let mut authors = Vec::new();
    for li in doc.select(&selector(".anime_data.pc li, .anime_data li")) {
        let row = text(li);
        if row.contains("Generos") { tags = li.select(&selector("a")).map(text).collect(); }
        if row.contains("Estado") { status = parse_status(&row); }
        if row.contains("Studios") { authors = li.select(&selector("a")).map(text).filter(|v| !v.is_empty()).collect(); }
    }
    CatalogItem {
        key: path_key(path),
        title: text_doc(&doc, ".anime_info h3, h1").unwrap_or_else(|| title_from_path(path)),
        cover: attr_doc(&doc, ".anime_pic img, img", "src").map(|v| absolute_url(&v)),
        url: Some(absolute_url(path)),
        description: text_doc(&doc, ".anime_info p.scroll, p.scroll"),
        tags,
        authors,
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_hosters(body: &str, referer: &str) -> Vec<VideoHoster> {
    let doc = Html::parse_document(body);
    let script = doc.select(&selector("script")).map(|s| s.inner_html()).find(|s| s.contains("var video = [];")).unwrap_or_default();
    let mut links = Vec::new();
    if let Some(remote) = Regex::new(r#"var\s+servers\s*=\s*(\[.*?\]);"#).unwrap().captures(&script).and_then(|c| c.get(1)) {
        if let Ok(values) = serde_json::from_str::<Vec<JsLink>>(remote.as_str()) {
            for value in values {
                if let Some(raw) = value.remote.and_then(|r| general_purpose::STANDARD.decode(r).ok()).and_then(|b| String::from_utf8(b).ok()) {
                    links.push((value.server.unwrap_or_else(|| host_name(&raw)), lang(value.lang), raw));
                }
            }
        }
    }
    for server in doc.select(&selector("div.bg-servers a, .bg-servers a")) {
        let id = attr(&server, "data-id").unwrap_or_default();
        let name = text(server).if_empty("External".to_string());
        let lg = attr(&server, "class").and_then(|c| c.split("lg_").nth(1).and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse::<u64>().ok())).map(|v| lang(Some(v))).unwrap_or_default();
        if let Some(raw) = video_slot(&script, &id) {
            links.push((name, lg, normalize_embed(&raw)));
        }
    }
    links.into_iter().filter(|(_, _, u)| !u.is_empty()).map(|(name, lg, embed)| {
        let url = absolute_url(&embed);
        VideoHoster { key: format!("{name}|{lg}|{url}|{referer}"), name: format!("{lg} {name}").trim().to_string(), url: Some(url), lazy: true, video_count: Some(1), ..VideoHoster::default() }
    }).collect()
}

fn post_episodes(anime_id: &str, current_page: u64, token: &str, referer: &str) -> EpisodesPage {
    let body = client(referer).post(format!("{BASE_URL}/ajax/episodes/{anime_id}/{current_page}"))
        .xhr().referer(referer).header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!("_token={}", url::query_escape(token))).send_text().unwrap_or_default();
    serde_json::from_str(&body).unwrap_or_default()
}

fn ep_to_episode(ep: &EpisodeDto, path: &str) -> VideoEpisode { numbered_episode(ep.number, path) }
fn numbered_episode(num: u64, path: &str) -> VideoEpisode {
    let key = format!("{}/{}", path.trim_end_matches('/'), num);
    VideoEpisode { key: key.clone(), title: Some(format!("Episodio {num}")), episode_number: Some(num as f32), url: Some(absolute_url(&key)), language: Some("es".to_string()), ..VideoEpisode::default() }
}

fn normalize_embed(raw: &str) -> String {
    raw.replace("/jkokru.php?u=", "http://ok.ru/videoembed/")
        .replace("/jkvmixdrop.php?u=", "https://mixdrop.ag/e/")
        .replace("/jksw.php?u=", "https://sfastwish.com/e/")
        .replace("/jk.php?u=", &format!("{BASE_URL}/"))
}
fn video_slot(script: &str, id: &str) -> Option<String> {
    Regex::new(&format!(r#"video\[{id}\]\s*=\s*'<iframe class=\\"player_conte\\" src=\\"([^"]+)""#)).ok()?.captures(script)?.get(1).map(|m| m.as_str().replace("\\/", "/"))
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_url(embed);
    if embed.contains(".m3u8") { return parse_hls(&embed, name, referer, request); }
    let body = fetch(&embed, "", referer);
    if let Some(src) = first_media_url(&body).map(|v| absolute_remote(&v, &embed)) {
        if src.contains(".m3u8") { parse_hls(&src, name, &embed, request) } else { vec![stream(&src, name, "direct", &embed, false)] }
    } else { vec![external_stream(&embed, name, referer)] }
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

fn client(referer: &str) -> HttpClient { HttpClient::browser().with_desktop_user_agent().with_referer(referer).with_cookies_for(BASE_URL).with_webview_challenge_fallback() }
fn fetch(target: &str, fixture: &str, referer: &str) -> String { client(referer).get(target).browser_document().referer(referer).send_text().unwrap_or_else(|_| fixture.to_string()) }
fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let lang = pref(request, "preferred_language", "[JAP]");
    let server = pref(request, "preferred_server", "Okru").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|s| { let n = s.name.clone().unwrap_or_default(); (n.contains(&lang), n.to_ascii_lowercase().contains(&server), n.contains(&quality), quality_rank(&n)) });
    streams.reverse();
}
fn filter_params(request: &Value) -> String {
    ["genre", "letter", "demografia", "categoria", "tipo", "estado", "fecha", "temporada", "filtro", "orden"].into_iter()
        .filter_map(|k| filter(request, k).filter(|v| !v.is_empty()).map(|v| format!("{}={}&", if k == "letter" { "letra" } else { k }, url::query_escape(&v))))
        .collect::<String>()
}
fn selector(input: &str) -> Selector { Selector::parse(input).unwrap() }
fn text_doc(doc: &Html, sel: &str) -> Option<String> { doc.select(&selector(sel)).next().map(text).filter(|v| !v.is_empty()) }
fn attr_doc(doc: &Html, sel: &str, name: &str) -> Option<String> { doc.select(&selector(sel)).next().and_then(|e| attr(&e, name)) }
fn text_sel(el: ElementRef<'_>, sel: &str) -> Option<String> { el.select(&selector(sel)).next().map(text).filter(|v| !v.is_empty()) }
fn attr_sel(el: ElementRef<'_>, sel: &str, name: &str) -> Option<String> { el.select(&selector(sel)).next().and_then(|e| attr(&e, name)) }
fn attr(el: &ElementRef<'_>, name: &str) -> Option<String> { el.value().attr(name).map(ToString::to_string) }
fn text(el: ElementRef<'_>) -> String { el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ") }
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
fn title_from_path(path: &str) -> String { path.trim_matches('/').rsplit('/').next().unwrap_or("Jkanime").replace('-', " ") }
fn parse_status(input: &str) -> ItemStatus { if input.contains("Concluido") || input.contains("Finalizado") { ItemStatus::Completed } else if input.contains("emision") || input.contains("Emisión") || input.contains("estrenar") { ItemStatus::Ongoing } else { ItemStatus::Unknown } }
fn lang(value: Option<u64>) -> String { match value { Some(1) => "[JAP]", Some(3) => "[LAT]", Some(4) => "[CHIN]", _ => "" }.to_string() }
fn host_name(input: &str) -> String { input.split("://").nth(1).unwrap_or(input).split('/').next().unwrap_or("External").replace("www.", "") }
fn episode_like(path: &str) -> bool { path.trim_matches('/').rsplit('/').next().and_then(|v| v.parse::<u64>().ok()).is_some() }

#[derive(Default, Deserialize)]
struct AnimePage { data: Vec<AnimeDto>, next_page_url: Option<String> }
#[derive(Default, Deserialize)]
struct AnimeDto { title: String, synopsis: Option<String>, image: String, studios: Option<String>, estado: Option<String>, url: String }
#[derive(Default, Deserialize)]
struct EpisodesPage { data: Vec<EpisodeDto>, last_page: u64, to: u64, total: u64 }
#[derive(Default, Deserialize)]
struct EpisodeDto { number: u64 }
#[derive(Default, Deserialize)]
struct JsLink { remote: Option<String>, server: Option<String>, lang: Option<u64> }

export_video_source!(SOURCE);

const HOME_FIXTURE: &str = r#"<div class="trending_div"><div class="custom_thumb_home"><a href="/sample"><img alt="Sample" src="/sample.jpg"></a></div></div>"#;
const DIRECTORY_FIXTURE: &str = r#"<script>var animes = {"data":[{"title":"Sample","synopsis":"Sample description.","image":"/sample.jpg","studios":"Studio","estado":"Concluido","url":"/sample"}],"next_page_url":null};</script>"#;
const DETAILS_FIXTURE: &str = r#"<meta name="csrf-token" content="token"><div class="anime__details__content"><div class="anime_pic"><img src="/sample.jpg"></div><div class="anime_info"><h3>Sample</h3><p class="scroll">Sample description.</p></div><div class="pc"><div id="guardar-anime" data-anime="1"></div></div></div><ul class="anime_data pc"><li><span>Generos:</span><a>Accion</a></li><li><span>Estado</span><div>Concluido</div></li></ul>"#;
const WATCH_FIXTURE: &str = r#"<div class="bg-servers"><a data-id="0" class="lg_1">Okru</a></div><script>var video = []; video[0] = '<iframe class=\"player_conte\" src=\"https://example.invalid/embed\"';</script>"#;
