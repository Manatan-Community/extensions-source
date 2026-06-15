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

const SOURCE: Lacartoons = Lacartoons;
const BASE_URL: &str = "https://www.lacartoons.com";

struct Lacartoons;

impl VideoSource for Lacartoons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_listing(&fetch(
            &format!("{BASE_URL}/?page={}", page(&request)),
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
        let target = if !query.is_empty() {
            format!(
                "{BASE_URL}/?utf8=%E2%9C%93&Titulo={}",
                url::query_escape(query)
            )
        } else if let Some(studio) = filter(&request, "studio").filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/?Categoria_id={studio}&page={}", page(&request))
        } else {
            format!("{BASE_URL}/?page={}", page(&request))
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/serie/1".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/serie/1".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode =
            request_key(&request, "episode").unwrap_or_else(|| "/capitulo/sample".to_string());
        let referer = absolute_url(&episode);
        let body = fetch(&referer, EPISODE_FIXTURE, BASE_URL);
        Ok(parse_hosters(&body, &referer))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(raw) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = raw.splitn(3, '|');
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
            for mut stream in self.resolve_hoster(json!({
                "hoster": { "key": hoster.key },
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
            }))? {
                stream.hoster = Some(hoster.clone());
                out.push(stream);
            }
        }
        sort_streams(&mut out, &request);
        Ok(out)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Series".to_string(),
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
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input) {
            if !path.starts_with("/serie/") {
                return Ok(Some(UrlResolveResult {
                    episode: Some(
                        json!({ "key": path, "url": absolute_url(&path), "language": "es" }),
                    ),
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
            .select(&sel(".conjuntos-series a"))
            .filter_map(card)
            .collect(),
        has_next_page: doc.select(&sel(".pagination a[rel=next]")).next().is_some(),
    }
}
fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href")?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_text(el, ".serie .informacion-serie .nombre-serie")
            .unwrap_or_else(|| title_from_path(&key)),
        cover: select_attr(el, ".serie img, img", "src").map(|v| absolute_url(&v)),
        url: Some(absolute_url(&key)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}
fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, ".subtitulo-serie-seccion, h1")
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, "div.h-thumb figure img, img", "src")
            .map(|v| absolute_url(&v)),
        url: Some(absolute_url(path)),
        description: select_text_doc(&doc, ".informacion-serie-seccion p span")
            .or_else(|| select_text_doc(&doc, ".informacion-serie-seccion p")),
        tags: select_text_doc(&doc, ".marcador-cartoon")
            .into_iter()
            .collect(),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}
fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let seasons: Vec<_> = doc.select(&sel(".estilo-temporada")).collect();
    let panels: Vec<_> = doc.select(&sel(".episodio-panel")).collect();
    let mut out = Vec::new();
    let mut counter = 1.0;
    for (idx, season) in seasons.into_iter().enumerate() {
        let sn = first_number(&own_text(season)).unwrap_or((idx + 1) as f32);
        if let Some(panel) = panels.get(idx) {
            for ep in panel.select(&sel("ul > li > a")) {
                let no = select_text(ep, "span")
                    .and_then(|t| first_number(&t))
                    .unwrap_or(counter);
                let href = attr(&ep, "href").unwrap_or_default();
                let key = path_key(&href);
                out.push(VideoEpisode {
                    key: key.clone(),
                    title: Some(format!(
                        "T{} - E{} - {}",
                        sn as i32,
                        no as i32,
                        own_text(ep)
                    )),
                    episode_number: Some(counter),
                    url: Some(absolute_url(&key)),
                    language: Some("es".to_string()),
                    ..VideoEpisode::default()
                });
                counter += 1.0;
            }
        }
    }
    out.reverse();
    out
}
fn parse_hosters(body: &str, referer: &str) -> Vec<VideoHoster> {
    let doc = Html::parse_document(body);
    doc.select(&sel("iframe"))
        .filter_map(|iframe| {
            let src = attr(&iframe, "src").filter(|v| !v.is_empty())?;
            let embed = absolute_remote(&src, referer);
            let name = matched_server(&embed).unwrap_or_else(|| host_name(&embed));
            Some(VideoHoster {
                key: format!("{name}|{embed}|{referer}"),
                name,
                url: Some(embed),
                lazy: true,
                video_count: Some(1),
                ..VideoHoster::default()
            })
        })
        .collect()
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
        return vec![media_stream(&src, name, "direct", &embed)];
    }
    vec![external_stream(&embed, name, referer)]
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
            out.push(media_stream(
                &absolute_remote(line.trim(), master),
                name,
                &quality,
                referer,
            ));
        }
    }
    if out.is_empty() {
        out.push(media_stream(master, name, "auto", referer));
    }
    sort_streams(&mut out, request);
    out
}
fn media_stream(target: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = target.contains(".m3u8");
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
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
fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)["']"#,
        r#"src\s*:\s*["']([^"']+)["']"#,
        r#"<source[^>]+src=["']([^"']+)["']"#,
    ]
    .into_iter()
    .find_map(|p| {
        Regex::new(p)
            .ok()?
            .captures(body)?
            .get(1)
            .map(|m| m.as_str().replace("\\/", "/"))
    })
}
fn matched_server(input: &str) -> Option<String> {
    let lower = input.to_ascii_lowercase();
    [
        (
            "StreamWish",
            &["wishembed", "streamwish", "strwish", "wish"][..],
        ),
        ("Voe", &["voe"][..]),
        ("Okru", &["ok.ru", "okru"][..]),
        ("YourUpload", &["yourupload", "upload"][..]),
        ("FileLions", &["filelions", "lion"][..]),
        (
            "StreamHideVid",
            &["vidhide", "streamhide", "guccihide", "streamvid"][..],
        ),
        ("Sendvid", &["sendvid"][..]),
    ]
    .iter()
    .find(|(_, keys)| keys.iter().any(|k| lower.contains(k)))
    .map(|(name, _)| (*name).to_string())
}
fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let server = pref(request, "preferred_server", "FileLions").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|s| {
        let name = s.name.clone().unwrap_or_default().to_ascii_lowercase();
        let q = s.quality.clone().unwrap_or_default();
        (
            name.contains(&server),
            q.contains(&quality),
            quality_rank(&q),
        )
    });
    streams.reverse();
}
fn sel(input: &str) -> Selector {
    Selector::parse(input).unwrap()
}
fn select_text_doc(doc: &Html, s: &str) -> Option<String> {
    doc.select(&sel(s))
        .next()
        .map(text)
        .filter(|v| !v.is_empty())
}
fn select_attr_doc(doc: &Html, s: &str, a: &str) -> Option<String> {
    doc.select(&sel(s))
        .next()
        .and_then(|e| e.value().attr(a))
        .map(ToString::to_string)
}
fn select_text(el: ElementRef<'_>, s: &str) -> Option<String> {
    el.select(&sel(s))
        .next()
        .map(text)
        .filter(|v| !v.is_empty())
}
fn select_attr(el: ElementRef<'_>, s: &str, a: &str) -> Option<String> {
    el.select(&sel(s))
        .next()
        .and_then(|e| e.value().attr(a))
        .map(ToString::to_string)
}
fn attr(el: &ElementRef<'_>, a: &str) -> Option<String> {
    el.value().attr(a).map(ToString::to_string)
}
fn text(el: ElementRef<'_>) -> String {
    el.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn own_text(el: ElementRef<'_>) -> String {
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
    let t = input.trim().replace("\\/", "/").replace("&amp;", "&");
    if t.starts_with("http://") || t.starts_with("https://") {
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
        .or_else(|| input.strip_prefix("https://lacartoons.com"))
        .filter(|p| p.starts_with('/'))
        .map(path_key)
}
fn path_key(input: &str) -> String {
    if let Some(p) = input
        .strip_prefix(BASE_URL)
        .or_else(|| input.strip_prefix("https://lacartoons.com"))
    {
        return path_key(p);
    }
    format!(
        "/{}",
        input
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
fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("LACartoons")
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
fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
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
fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|f| f.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}
fn quality_rank(input: &str) -> i32 {
    Regex::new(r#"(\d+)"#)
        .unwrap()
        .captures(input)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0)
}
fn first_number(input: &str) -> Option<f32> {
    Regex::new(r#"(\d+(?:\.\d+)?)"#)
        .ok()?
        .captures(input)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}
fn referer_headers(referer: &str) -> Context {
    let mut h = Context::new();
    h.insert("Referer".to_string(), referer.to_string());
    h
}

const LIST_FIXTURE: &str = r#"<div class="conjuntos-series"><a href="/serie/1"><div class="serie"><img src="/cover.jpg"><div class="informacion-serie"><p class="nombre-serie">Serie Demo</p></div></div></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="subtitulo-serie-seccion">Serie Demo</h1><div class="h-thumb"><figure><img src="/cover.jpg"></figure></div><div class="marcador-cartoon">Nickelodeon</div><div class="informacion-serie-seccion"><p>Reseña <span>Demo</span></p></div><div class="estilo-temporada">Temporada 1</div><div class="episodio-panel"><ul><li><a href="/capitulo/demo-1"><span>1</span> Piloto</a></li></ul></div>"#;
const EPISODE_FIXTURE: &str = r#"<iframe src="https://example.com/embed/demo"></iframe>"#;

export_video_source!(SOURCE);
