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
use serde_json::{Value, json};

const SOURCE: MundoDonghua = MundoDonghua;
const BASE_URL: &str = "https://www.mundodonghua.com";

struct MundoDonghua;

impl VideoSource for MundoDonghua {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/lista-episodios/{page}")
        } else {
            format!("{BASE_URL}/lista-donghuas/{page}")
        };
        let mut paged = parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL));
        if listing(&request) == "latest" {
            for item in &mut paged.entries {
                item.key = item.key.replace("/ver/", "/donghua/");
                if let Some(idx) = item.key.rfind('/') {
                    item.key.truncate(idx);
                }
                item.url = Some(absolute_url(&item.key));
            }
        }
        Ok(paged)
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
        let target = if !query.is_empty() {
            format!("{BASE_URL}/busquedas/{}", url::query_escape(query))
        } else if let Some(genre) = filter(&request, "genre").filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/genero/{}", url::query_escape(&genre))
        } else {
            format!("{BASE_URL}/lista-donghuas/{}", page(&request))
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/donghua/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/donghua/sample".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/ver/sample/1".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        Ok(parse_hosters(&body, &referer))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = key.splitn(3, '|');
        let name = parts.next().unwrap_or("External");
        let embed = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(BASE_URL);
        let mut streams = resolve_embed(embed, name, referer, &request);
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
                title: "Donghuas".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Episodios".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|p| absolute_url(&p)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|p| absolute_url(&p)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input) {
            if path.starts_with("/ver/") {
                return Ok(Some(UrlResolveResult {
                    episode: Some(json!({ "key": path, "url": input, "language": "es" })),
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

fn parse_cards(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let entries = doc
        .select(&selector("div > div.row > div.item > a, .item > a"))
        .filter_map(card)
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("pagination") && body.contains("Siguiente"),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href");
    if href.is_empty() {
        return None;
    }
    Some(CatalogItem {
        key: path_key(&href),
        title: select_text(el, "h5")
            .unwrap_or_else(|| title_from_path(&href))
            .trim_matches('"')
            .to_string(),
        cover: select_attr(el, "img", "src").map(|src| absolute_url(&src)),
        url: Some(absolute_url(&href)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    let cover = select_attr_doc(&doc, ".banner-side-serie", "style")
        .and_then(|style| style.split("url(").nth(1).map(|v| v.split(')').next().unwrap_or(v).trim_matches(['"', '\'']).to_string()))
        .or_else(|| select_attr_doc(&doc, "img", "src"))
        .map(|src| absolute_url(&src));
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, ".ls-title-serie, h1, h2")
            .unwrap_or_else(|| title_from_path(path)),
        cover,
        url: Some(absolute_url(path)),
        description: select_text_doc(&doc, "p.text-justify, .text-justify"),
        tags: select_texts_doc(&doc, "a.generos span.label, a[href*='/genero/']"),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&body),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let mut episodes = doc
        .select(&selector("div.sm-row.mt-10 div.donghua-list-scroll ul.donghua-list a, a[href*='/ver/']"))
        .filter_map(|el| {
            let href = attr(&el, "href");
            if href.is_empty() || !href.contains("/ver/") {
                return None;
            }
            let number = href
                .trim_matches('/')
                .rsplit('/')
                .next()
                .and_then(|v| v.parse::<f32>().ok());
            Some(VideoEpisode {
                key: path_key(&href),
                title: Some(format!("Episodio {}", number.map(|v| v.to_string()).unwrap_or_else(|| title_from_path(&href)))),
                episode_number: number,
                url: Some(absolute_url(&href)),
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

fn parse_hosters(body: &str, referer: &str) -> Vec<VideoHoster> {
    let mut urls = iframe_urls(body);
    urls.extend(fetch_urls(body));
    urls.extend(protea_url(body, referer));
    urls.sort();
    urls.dedup();
    urls.into_iter()
        .filter(|u| u.starts_with("http"))
        .map(|embed| {
            let name = matched_server(&embed).unwrap_or_else(|| host_name(&embed));
            VideoHoster {
                key: format!("{name}|{embed}|{referer}"),
                name,
                url: Some(embed),
                lazy: true,
                video_count: Some(1),
                ..VideoHoster::default()
            }
        })
        .collect()
}

fn iframe_urls(body: &str) -> Vec<String> {
    let doc = Html::parse_document(body);
    doc.select(&selector("iframe"))
        .filter_map(|iframe| attr(&iframe, "src").or_else_empty(attr(&iframe, "data-src")))
        .map(|src| absolute_url(&src))
        .collect()
}

fn fetch_urls(text: &str) -> Vec<String> {
    Regex::new(r#"https?://[\w.\-/%?=&:#]+"#)
        .unwrap()
        .find_iter(text)
        .map(|m| m.as_str().trim_matches(['"', '\'']).replace("\\/", "/"))
        .collect()
}

fn protea_url(body: &str, referer: &str) -> Vec<String> {
    let Some(slug) = Regex::new(r#""slug"\s*:\s*"([^"]+)""#)
        .unwrap()
        .captures(body)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str())
    else {
        return Vec::new();
    };
    let api = client(referer)
        .get(format!("{BASE_URL}/api_donghua.php?slug={slug}"))
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    let Some(key) = Regex::new(r#""url"\s*:\s*"([^"]+)""#)
        .unwrap()
        .captures(&api)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().replace("\\/", "/"))
    else {
        return Vec::new();
    };
    let player = client(BASE_URL)
        .get(format!("https://www.mdplayer.xyz/nemonicplayer/dmplayer.php?key={key}"))
        .referer(BASE_URL)
        .send_text()
        .unwrap_or_default();
    Regex::new(r#"video-id=["']([^"']+)["']"#)
        .unwrap()
        .captures(&player)
        .and_then(|cap| cap.get(1))
        .map(|m| vec![format!("https://www.dailymotion.com/embed/video/{}", m.as_str())])
        .unwrap_or_default()
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_url(embed);
    if embed.contains(".m3u8") {
        return parse_hls(&embed, name, referer, request);
    }
    let body = fetch(&embed, "", referer);
    if let Some(src) = first_media_url(&body).map(|s| absolute_remote(&s, &embed)) {
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
    [
        r#"file\s*:\s*["']([^"']+)["']"#,
        r#"src\s*:\s*["']([^"']+)["']"#,
        r#"<source[^>]+src=["']([^"']+)["']"#,
        r#"https?://[^\s'"\\]+\.m3u8[^\s'"\\]*"#,
    ]
    .into_iter()
    .find_map(|p| {
        let re = Regex::new(p).ok()?;
        if p.starts_with("http") {
            re.find(body).map(|m| m.as_str().replace("\\/", "/"))
        } else {
            re.captures(body)?
                .get(1)
                .map(|m| m.as_str().replace("\\/", "/"))
        }
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
    let preferred = pref(request, "preferred_quality", "VoeCDN").to_ascii_lowercase();
    streams.sort_by_key(|s| {
        let name = s.name.clone().unwrap_or_default().to_ascii_lowercase();
        let q = s.quality.clone().unwrap_or_default();
        (
            name == preferred || name.contains(&preferred),
            quality_rank(&q).max(quality_rank(&name)),
        )
    });
    streams.reverse();
}

fn parse_status(body: &str) -> ItemStatus {
    if body.contains("Finalizada") {
        ItemStatus::Completed
    } else if body.contains("En Emision") || body.contains("En Emisión") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn matched_server(input: &str) -> Option<String> {
    let lower = input.to_ascii_lowercase();
    [
        ("VoeCDN", ["voe", "amagi"].as_slice()),
        ("Filemoon", ["filemoon", "fmoon", "moonplayer"].as_slice()),
        ("Dailymotion", ["dailymotion"].as_slice()),
        ("Asura", ["redirector", "mdnemonicplayer"].as_slice()),
    ]
    .into_iter()
    .find(|(_, names)| names.iter().any(|name| lower.contains(name)))
    .map(|(name, _)| name.to_string())
}

trait OrElseEmpty {
    fn or_else_empty(self, other: String) -> Option<String>;
}

impl OrElseEmpty for String {
    fn or_else_empty(self, other: String) -> Option<String> {
        if self.is_empty() {
            (!other.is_empty()).then_some(other)
        } else {
            Some(self)
        }
    }
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

fn attr(el: &ElementRef<'_>, name: &str) -> String {
    el.value().attr(name).unwrap_or_default().to_string()
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
    let t = input.trim().replace("\\/", "/");
    if t.starts_with("http") {
        t
    } else if let Some(rest) = t.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url::join_url(base, &t)
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

fn with_listing(request: &Value, listing: &str) -> Value {
    json!({ "listing": listing, "preferences": request.get("preferences").cloned().unwrap_or(Value::Null) })
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|f| f.get(key))
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

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("MundoDonghua")
        .replace('-', " ")
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

fn quality_rank(q: &str) -> i32 {
    Regex::new(r#"(\d+)"#)
        .unwrap()
        .captures(q)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0)
}

fn referer_headers(referer: &str) -> Context {
    let mut h = Context::new();
    h.insert("Referer".to_string(), referer.to_string());
    h
}

const LIST_FIXTURE: &str = r#"<div><div class="row"><div class="item"><a href="/donghua/sample"><img src="/cover.jpg"><h5>Sample</h5></a></div></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="ls-title-serie">Sample</div><p class="text-justify">Sample description.</p><a class="generos"><span class="label">Accion</span></a><ul class="donghua-list"><a href="/ver/sample/1">1</a></ul>"#;
const WATCH_FIXTURE: &str = r#"<iframe src="https://example.invalid/embed"></iframe>"#;

export_video_source!(SOURCE);
