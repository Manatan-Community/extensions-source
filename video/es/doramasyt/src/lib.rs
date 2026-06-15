use base64::{Engine, engine::general_purpose};
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

const SOURCE: Doramasyt = Doramasyt;
const BASE_URL: &str = "https://www.doramasyt.com";

struct Doramasyt;

impl VideoSource for Doramasyt {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/emision?p={page}")
        } else {
            format!("{BASE_URL}/doramas?p={page}")
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
            format!("{BASE_URL}/buscar?q={}", url::query_escape(query))
        } else {
            let qs = filter_params(&request, ["categoria", "genero", "fecha", "letra"]);
            if qs.is_empty() {
                format!("{BASE_URL}/doramas?p={page}")
            } else {
                format!("{BASE_URL}/doramas?{qs}&p={page}")
            }
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/dorama/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/dorama/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body, &referer))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/ver/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let mut streams = Vec::new();
        let doc = Html::parse_document(&body);
        for el in doc.select(&selector("[data-player]")) {
            let raw = attr(&el, "data-player");
            if let Ok(bytes) = general_purpose::STANDARD.decode(raw) {
                if let Ok(embed) = String::from_utf8(bytes) {
                    streams.extend(resolve_embed(
                        &absolute_remote(&embed, &referer),
                        &host_name(&embed),
                        &referer,
                        &request,
                    ));
                }
            }
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
                title: "Doramas".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "En emision".to_string(),
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
        .select(&selector(".ficha_efecto a"))
        .filter_map(card)
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\""),
    }
}
fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href");
    Some(CatalogItem {
        key: path_key(&href),
        title: select_text(el, ".title_cap")
            .or_else(|| select_attr(el, "img", "alt"))
            .unwrap_or_else(|| title_from_path(&href)),
        cover: image_url(el).map(|src| absolute_url(&src)),
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
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, ".flex-column h1.text-capitalize, h1")
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, ".gap-3 img, img", "data-src")
            .or_else(|| select_attr_doc(&doc, ".gap-3 img, img", "src"))
            .map(|src| absolute_url(&src)),
        url: Some(absolute_url(path)),
        description: select_text_doc(&doc, ".h-100 .mb-3 p, .mb-3 p"),
        tags: select_texts_doc(&doc, ".lh-lg span, a[href*='genero']"),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: if body.contains("Finalizado") {
            ItemStatus::Completed
        } else if body.contains("Estreno") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        initialized: true,
        ..CatalogItem::default()
    }
}
fn parse_episodes(body: &str, referer: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let token = select_attr_doc(&doc, "meta[name='csrf-token']", "content").unwrap_or_default();
    let caplist = select_attr_doc(&doc, ".caplist", "data-ajax").unwrap_or_default();
    if token.is_empty() || caplist.is_empty() {
        return direct_episode_links(&doc);
    }
    let details = client(referer)
        .post(absolute_url(&caplist))
        .xhr()
        .referer(referer)
        .header("X-Requested-With", "XMLHttpRequest")
        .form(&[("_token", token.as_str())])
        .send_text()
        .unwrap_or_default();
    let value: Value = serde_json::from_str(&details).unwrap_or(Value::Null);
    let total = value
        .get("eps")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0) as f64;
    let per_page = value
        .get("perpage")
        .and_then(Value::as_f64)
        .unwrap_or(total.max(1.0));
    let paginate = value
        .get("paginate_url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let pages = (total / per_page).ceil().max(1.0) as u64;
    let mut episodes = Vec::new();
    for p in 1..=pages {
        let page_body = client(referer)
            .post(absolute_url(paginate))
            .xhr()
            .referer(referer)
            .header("X-Requested-With", "XMLHttpRequest")
            .form(&[("_token", token.as_str()), ("p", &p.to_string())])
            .send_text()
            .unwrap_or_default();
        let page_json: Value = serde_json::from_str(&page_body).unwrap_or(Value::Null);
        if let Some(caps) = page_json.get("caps").and_then(Value::as_array) {
            for (idx, cap) in caps.iter().enumerate() {
                let num = cap
                    .get("episodio")
                    .and_then(Value::as_f64)
                    .unwrap_or((idx + 1) as f64) as f32;
                if let Some(href) = cap.get("url").and_then(Value::as_str) {
                    episodes.push(VideoEpisode {
                        key: path_key(href),
                        title: Some(format!("Capitulo {num}")),
                        episode_number: Some(num),
                        url: Some(absolute_url(href)),
                        language: Some("es".to_string()),
                        ..VideoEpisode::default()
                    });
                }
            }
        }
    }
    episodes.reverse();
    episodes
}
fn direct_episode_links(doc: &Html) -> Vec<VideoEpisode> {
    doc.select(&selector("a[href*='/ver/']"))
        .enumerate()
        .map(|(idx, el)| {
            let href = attr(&el, "href");
            VideoEpisode {
                key: path_key(&href),
                title: Some(format!("Capitulo {}", idx + 1)),
                episode_number: Some((idx + 1) as f32),
                url: Some(absolute_url(&href)),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            }
        })
        .collect()
}
fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    if embed.contains(".m3u8") {
        return parse_hls(embed, name, referer, request);
    }
    let body = fetch(embed, "", referer);
    if let Some(src) = first_media_url(&body).map(|s| absolute_remote(&s, embed)) {
        if src.contains(".m3u8") {
            parse_hls(&src, name, embed, request)
        } else {
            vec![stream(&src, name, "direct", embed, false)]
        }
    } else {
        vec![external_stream(embed, name, referer)]
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
    let server = pref(request, "preferred_server", "Filemoon").to_ascii_lowercase();
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
fn image_url(el: ElementRef<'_>) -> Option<String> {
    ["data-src", "data-lazy-src", "srcset", "src"]
        .into_iter()
        .find_map(|a| select_attr(el, "img", a))
        .map(|v| v.split_whitespace().next().unwrap_or("").to_string())
        .filter(|v| !v.contains("anime.png") && !v.is_empty())
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
fn filter_params<const N: usize>(request: &Value, keys: [&str; N]) -> String {
    keys.into_iter()
        .filter_map(|k| {
            filter(request, k)
                .filter(|v| !v.is_empty())
                .map(|v| format!("{k}={}", url::query_escape(&v)))
        })
        .collect::<Vec<_>>()
        .join("&")
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
        .unwrap_or("Doramasyt")
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

const LIST_FIXTURE: &str = r#"<div class="ficha_efecto"><a href="/dorama/sample"><img src="/cover.jpg"><span class="title_cap">Sample</span></a></div>"#;
const DETAILS_FIXTURE: &str =
    r#"<h1 class="text-capitalize">Sample</h1><div class="caplist"></div>"#;
const WATCH_FIXTURE: &str = r#"<div data-player="aHR0cHM6Ly9leGFtcGxlLmludmFsaWQvZW1iZWQ="></div>"#;

export_video_source!(SOURCE);
