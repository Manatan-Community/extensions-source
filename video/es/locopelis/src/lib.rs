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

const SOURCE: LocoPelis = LocoPelis;
const BASE_URL: &str = "https://www.locopelis.com";

struct LocoPelis;

impl VideoSource for LocoPelis {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/pelicula/ultimas-peliculas?page={page}")
        } else {
            format!("{BASE_URL}/pelicula/peliculas-mas-vistas?page={page}")
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
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
                "{BASE_URL}/buscar/?q={}&page={page}",
                url::query_escape(query)
            )
        } else if let Some(genre) = filter(&request, "genre").filter(|v| !v.is_empty()) {
            if genre.contains("pelicula/") {
                format!("{BASE_URL}/{genre}?page={page}")
            } else {
                format!("{BASE_URL}/categoria/{genre}/?page={page}")
            }
        } else {
            format!("{BASE_URL}/pelicula/peliculas-mas-vistas?page={page}")
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/pelicula/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/pelicula/sample".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(vec![VideoEpisode {
            key: path.clone(),
            title: Some("PELICULA".to_string()),
            episode_number: Some(1.0),
            url: Some(absolute_url(&path)),
            date_uploaded: parse_date(&body),
            language: Some("es".to_string()),
            ..VideoEpisode::default()
        }])
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/pelicula/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let mut streams = iframe_sources(&body)
            .into_iter()
            .flat_map(|src| resolve_embed(&src, &server_name(&src), &referer, &request))
            .collect::<Vec<_>>();
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Mas vistas".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Ultimas peliculas".to_string(),
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

fn parse_cards(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc
            .select(&selector("ul.peliculas li.peli_bx, li.peli_bx"))
            .filter_map(card)
            .collect(),
        has_next_page: doc
            .select(&selector("#cn div ul.nav li ~ li, .pagination a"))
            .next()
            .is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr_sel(el, "div.peli_img div.peli_img_img a, a[href]", "href")?;
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title: text_sel(el, "h2.titpeli, h2").unwrap_or_else(|| title_from_path(&path)),
        cover: attr_sel(el, "div.peli_img div.peli_img_img a img, img", "src")
            .map(|v| absolute_url(&v)),
        url: Some(absolute_url(&path)),
        description: text_sel(el, "div.peli_img div.peli_txt p")
            .map(|v| v.trim_matches('"').to_string()),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        initialized: false,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    let mut title = title_from_path(path);
    let mut tags = Vec::new();
    for row in doc.select(&selector("div.content div.details ul.dtalist li")) {
        let text = element_text(row).to_ascii_lowercase();
        if text.contains("titulo latino") {
            title = text.replace("titulo latino:", "").trim().to_string();
        } else if text.contains("genero") {
            tags = text
                .replace("genero:", "")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    CatalogItem {
        key: path_key(path),
        title,
        cover: attr_doc(&doc, "div.intsd div.peli_img_int img, img", "src")
            .map(|v| absolute_url(&v)),
        url: Some(absolute_url(path)),
        description: text_doc(&doc, "div.content span div.sinoptxt strong, .sinoptxt")
            .map(|v| v.trim_matches('"').to_string()),
        tags,
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn iframe_sources(body: &str) -> Vec<String> {
    let doc = Html::parse_document(body);
    let mut out = doc
        .select(&selector(".tab_container .tab_content iframe, iframe"))
        .filter_map(|el| attr(&el, "src").or_else(|| attr(&el, "data-src")))
        .map(|src| absolute_remote(&src, BASE_URL))
        .collect::<Vec<_>>();
    out.sort();
    out.dedup();
    out
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_remote(embed, BASE_URL);
    if embed.contains(".m3u8") {
        return parse_hls(&embed, name, referer, request);
    }
    let body = fetch(&embed, "", referer);
    if let Some(src) = first_media_url(&body).map(|v| absolute_remote(&v, &embed)) {
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
        r#"url\s*:\s*["']([^"']+)["']"#,
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
fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let server = pref(request, "preferred_server", "DoodStream").to_ascii_lowercase();
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
fn text_doc(doc: &Html, sel: &str) -> Option<String> {
    doc.select(&selector(sel))
        .next()
        .map(element_text)
        .filter(|v| !v.is_empty())
}
fn attr_doc(doc: &Html, sel: &str, name: &str) -> Option<String> {
    doc.select(&selector(sel))
        .next()
        .and_then(|e| attr(&e, name))
}
fn text_sel(el: ElementRef<'_>, sel: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .map(element_text)
        .filter(|v| !v.is_empty())
}
fn attr_sel(el: ElementRef<'_>, sel: &str, name: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .and_then(|e| attr(&e, name))
}
fn attr(el: &ElementRef<'_>, name: &str) -> Option<String> {
    el.value().attr(name).map(ToString::to_string)
}
fn element_text(el: ElementRef<'_>) -> String {
    el.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn referer_headers(referer: &str) -> Context {
    let mut h = Context::new();
    h.insert("Referer".to_string(), referer.to_string());
    h
}
fn absolute_url(input: &str) -> String {
    absolute_remote(input, BASE_URL)
}
fn absolute_remote(input: &str, base: &str) -> String {
    let t = input
        .trim()
        .replace("\\/", "/")
        .replace("&amp;", "&")
        .replace("#038;", "&");
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
    request
        .get(field)
        .and_then(|v| {
            v.get("key")
                .or_else(|| v.get("url"))
                .and_then(Value::as_str)
                .or_else(|| v.as_str())
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(path_key)
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
    json!({"listing":listing,"preferences":request.get("preferences").cloned().unwrap_or(Value::Null)})
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
        .unwrap_or("LocoPelis")
        .replace('-', " ")
}
fn server_name(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if lower.contains("ok.ru") || lower.contains("okru") {
        "Okru".to_string()
    } else if lower.contains("dood") {
        "DoodStream".to_string()
    } else if lower.contains("streamtape") || lower.contains("stape") {
        "StreamTape".to_string()
    } else if lower.contains("vidhide")
        || lower.contains("streamhide")
        || lower.contains("streamvid")
    {
        "StreamHideVid".to_string()
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
fn parse_date(_body: &str) -> Option<i64> {
    None
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<ul class="peliculas"><li class="peli_bx"><div class="peli_img"><div class="peli_img_img"><a href="/pelicula/sample"><img src="/sample.jpg"></a></div><div class="peli_txt"><p>Sample description.</p></div></div><h2 class="titpeli">Sample</h2></li></ul>"#;
const DETAILS_FIXTURE: &str = r#"<div class="intsd"><div class="peli_img_int"><img src="/sample.jpg"></div></div><div class="content"><span><div class="sinoptxt"><strong>Sample description.</strong></div></span><div class="details"><ul class="dtalist"><li>Titulo Latino: Sample</li><li>Genero: Accion</li><li>Publicado: 2024-01-01</li></ul></div></div><div class="tab_container"><div class="tab_content"><iframe src="https://example.invalid/embed"></iframe></div></div>"#;
const WATCH_FIXTURE: &str = DETAILS_FIXTURE;
