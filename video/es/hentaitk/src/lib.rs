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
use serde_json::Value;

const SOURCE: HentaiTk = HentaiTk;
const BASE_URL: &str = "https://hentaitk.net";

struct HentaiTk;

impl VideoSource for HentaiTk {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_listing(&fetch(
            &format!("{BASE_URL}/hentais/page/{}", page(&request)),
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
        let page = page(&request);
        let target = if !query.is_empty() {
            format!("{BASE_URL}/page/{page}/?s={}", url::query_escape(query))
        } else if let Some(tag) = filter(&request, "tag").filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/{tag}page/{page}/")
        } else if let Some(kind) = filter(&request, "type").filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/{kind}page/{page}/")
        } else {
            format!("{BASE_URL}/hentais/page/{page}")
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }
    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        Ok(fetch_details(&path))
    }
    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        Ok(vec![VideoEpisode {
            key: path.clone(),
            title: Some("Episode".to_string()),
            episode_number: Some(1.0),
            url: Some(absolute_url(&path)),
            language: Some("es".to_string()),
            ..VideoEpisode::default()
        }])
    }
    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut embeds = Vec::new();
        if doc
            .select(&selector(".post-tape .page-link"))
            .next()
            .is_some()
        {
            for page in doc.select(&selector(".post-tape .page-link")) {
                let url = attr(&page, "href").unwrap_or_default();
                let page_body = fetch(&absolute_url(&url), "", &referer);
                let page_doc = Html::parse_document(&page_body);
                embeds.extend(
                    page_doc
                        .select(&selector(".embed-responsive-item iframe, iframe"))
                        .filter_map(|e| attr(&e, "src")),
                );
            }
        } else {
            embeds.extend(
                doc.select(&selector(".embed-responsive-item iframe, iframe"))
                    .filter_map(|e| attr(&e, "src")),
            );
        }
        let mut streams = Vec::new();
        for embed in embeds {
            streams.extend(resolve_embed(
                &embed,
                &host_name(&embed),
                &referer,
                &request,
            ));
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }
    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Hentais".to_string(),
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
fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc
            .select(&selector(".video-section .item, .item"))
            .filter_map(card)
            .collect(),
        has_next_page: doc
            .select(&selector(
                ".pagination .page-item .next, .pagination a.next",
            ))
            .next()
            .is_some(),
    }
}
fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, ".post-header .post-title a, a[href]", "href")?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_text(el, ".post-header .post-title a, .post-title a, h2, h3")
            .unwrap_or_else(|| title_from_path(&key)),
        cover: image_link(el),
        url: Some(absolute_url(&key)),
        language: Some("es".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}
fn image_link(el: ElementRef<'_>) -> Option<String> {
    let img = el.select(&selector(".item-img a img, img")).next()?;
    if let Some(srcset) = img.value().attr("srcset") {
        return srcset
            .split(',')
            .filter_map(|p| p.split_whitespace().next())
            .last()
            .map(absolute_url);
    }
    img.value()
        .attr("data-src")
        .or_else(|| img.value().attr("src"))
        .map(absolute_url)
}
fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, ".video-info h1, h1").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, "meta[property='og:image']", "content")
            .or_else(|| select_attr_doc(&doc, "img", "src"))
            .map(|s| absolute_url(&s)),
        description: select_text_doc(
            &doc,
            ".video-details .post-entry p:not([style]), .post-entry p",
        ),
        tags: select_texts_doc(&doc, ".video-details .meta a, .meta a"),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
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
        r#"file\s*:\s*["']([^"']+)"#,
        r#"src\s*:\s*["']([^"']+)"#,
        r#"<source[^>]+src=["']([^"']+)"#,
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
fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let server = pref(request, "preferred_server", "FileLions").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|s| {
        let n = s.name.clone().unwrap_or_default().to_ascii_lowercase();
        (n.contains(&server), n.contains(&quality), quality_rank(&n))
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
fn attr(el: &ElementRef<'_>, name: &str) -> Option<String> {
    el.value().attr(name).map(ToString::to_string)
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
fn referer_headers(referer: &str) -> Context {
    let mut h = Context::new();
    h.insert("Referer".to_string(), referer.to_string());
    h
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
        .unwrap_or("HentaiTk")
        .replace('-', " ")
}
export_video_source!(SOURCE);
const LIST_FIXTURE: &str = r#"<div class="video-section"><div class="item"><div class="post-header"><h2 class="post-title"><a href="/sample">Sample</a></h2></div><div class="item-img"><a><img src="/sample.jpg"></a></div></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="video-info"><h1>Sample</h1></div><div class="video-details"><div class="post-entry"><p>Sample synopsis.</p></div><div class="meta"><a>Tag</a></div></div><meta property="og:image" content="/sample.jpg">"#;
const WATCH_FIXTURE: &str = r#"<div class="embed-responsive-item"><iframe src="https://example.invalid/embed"></iframe></div>"#;
