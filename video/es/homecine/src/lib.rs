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

const SOURCE: HomeCine = HomeCine;
const BASE_URL: &str = "https://homecine.cc";

struct HomeCine;

impl VideoSource for HomeCine {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_listing(&fetch(
            &format!("{BASE_URL}/cartelera-series/page/{}", page(&request)),
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
        Ok(parse_listing(&fetch(
            &format!("{BASE_URL}/?s={}", url::query_escape(query)),
            LIST_FIXTURE,
            BASE_URL,
        )))
    }
    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/serie/sample".to_string());
        Ok(fetch_details(&path))
    }
    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/serie/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, DETAILS_FIXTURE, BASE_URL);
        if referer.contains("pelicula") {
            return Ok(vec![VideoEpisode {
                key: path.clone(),
                title: Some("Pelicula".to_string()),
                episode_number: Some(1.0),
                url: Some(referer),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let doc = Html::parse_document(&body);
        let mut out = Vec::new();
        for season in doc.select(&selector(".sel-temp a")) {
            out.extend(fetch_season_episodes(season, &referer));
        }
        out.sort_by(|a, b| b.title.cmp(&a.title));
        Ok(out)
    }
    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/pelicula/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for tab in doc.select(&selector(".aa-tbs-video a")) {
            let label = lang_prefix(&select_text(tab, ".server").unwrap_or_default());
            let target_id = attr(&tab, "href").unwrap_or_default();
            let mut src = select_attr_doc(&doc, &format!("{target_id} iframe"), "data-src")
                .or_else(|| select_attr_doc(&doc, &format!("{target_id} iframe"), "src"))
                .unwrap_or_default()
                .replace("#038;", "&")
                .replace("&amp;", "&");
            if src.contains("home") {
                let inner = fetch(&absolute_url(&src), "", &referer);
                src = select_attr_doc(&Html::parse_document(&inner), "iframe", "src")
                    .unwrap_or_default();
            }
            if !src.is_empty() {
                streams.extend(resolve_embed(
                    &src,
                    &format!("{label} {}", host_name(&src)).trim(),
                    &referer,
                    &request,
                ));
            }
        }
        if streams.is_empty() {
            for iframe in doc.select(&selector("iframe")) {
                if let Some(src) = attr(&iframe, "data-src").or_else(|| attr(&iframe, "src")) {
                    streams.extend(resolve_embed(&src, &host_name(&src), &referer, &request));
                }
            }
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }
    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Cartelera series".to_string(),
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
        entries: doc.select(&selector(".post")).filter_map(card).collect(),
        has_next_page: doc
            .select(&selector(".nav-links .current ~ a"))
            .next()
            .is_some(),
    }
}
fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, ".lnk-blk, a[href]", "href")?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_text(el, ".entry-header .entry-title, h2, h3")
            .unwrap_or_else(|| title_from_path(&key)),
        description: select_text(el, ".entry-content p"),
        cover: select_attr(
            el,
            ".post-thumbnail figure img, .post-thumbnail img, img",
            "data-src",
        )
        .or_else(|| {
            select_attr(
                el,
                ".post-thumbnail figure img, .post-thumbnail img, img",
                "src",
            )
        })
        .map(|s| absolute_url(&s)),
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
        title: select_text_doc(&doc, "aside .entry-header .entry-title, h1")
            .unwrap_or_else(|| title_from_path(path)),
        description: select_text_doc(&doc, "aside .description p:not([class]), .description p"),
        cover: select_attr_doc(&doc, ".post-thumbnail img, img", "data-src")
            .or_else(|| select_attr_doc(&doc, ".post-thumbnail img, img", "src"))
            .map(|s| absolute_url(&s).replace("/w185/", "/w500/")),
        tags: select_texts_doc(&doc, ".genres a"),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: if path.contains("pelicula") {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        },
        initialized: true,
        ..CatalogItem::default()
    }
}
fn fetch_season_episodes(season: ElementRef<'_>, referer: &str) -> Vec<VideoEpisode> {
    let post = attr(&season, "data-post").unwrap_or_default();
    let season_no = attr(&season, "data-season").unwrap_or_else(|| "1".to_string());
    let body = client(referer)
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .xhr()
        .referer(referer)
        .form(&[
            ("action", "action_select_season"),
            ("season", &season_no),
            ("post", &post),
        ])
        .send_text()
        .unwrap_or_else(|_| SEASON_FIXTURE.to_string());
    let doc = Html::parse_document(&body);
    let mut out = Vec::new();
    for (idx, ep) in doc.select(&selector(".post")).rev().enumerate() {
        let href = select_attr(ep, "a[href]", "href").unwrap_or_default();
        if href.is_empty() {
            continue;
        }
        let ep_no = select_text(ep, ".entry-header .num-epi")
            .and_then(|v| {
                v.split('x')
                    .nth(1)
                    .and_then(|v| v.split(['-', '–']).next())
                    .map(|v| v.trim().to_string())
            })
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or((idx + 1) as f32);
        let key = path_key(&href);
        out.push(VideoEpisode {
            key: key.clone(),
            title: Some(format!("T{season_no} - Episodio {}", trim_float(ep_no))),
            episode_number: Some(ep_no),
            url: Some(absolute_url(&key)),
            language: Some("es".to_string()),
            ..VideoEpisode::default()
        });
    }
    out
}
fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let mut embed = absolute_url(embed);
    if embed.contains("fastream") && embed.contains("emb.html") {
        let key = embed.rsplit('/').next().unwrap_or_default();
        embed = format!("https://fastream.to/embed-{key}.html");
    }
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
    let lang = pref(request, "preferred_language", "[LAT]");
    let server = pref(request, "preferred_server", "YourUpload").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|s| {
        let n = s.name.clone().unwrap_or_default();
        (
            n.contains(&lang),
            n.to_ascii_lowercase().contains(&server),
            n.contains(&quality),
            quality_rank(&n),
        )
    });
    streams.reverse();
}
fn lang_prefix(input: &str) -> &'static str {
    let l = input.to_ascii_lowercase();
    if l.contains("latino") {
        "[LAT]"
    } else if l.contains("castellano") {
        "[CAST]"
    } else if l.contains("sub") || l.contains("vose") {
        "[SUB]"
    } else {
        ""
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
        .unwrap_or("HomeCine")
        .replace('-', " ")
}
fn trim_float(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i32)
    } else {
        value.to_string()
    }
}
export_video_source!(SOURCE);
const LIST_FIXTURE: &str = r#"<article class="post"><a class="lnk-blk" href="/serie/sample"></a><header class="entry-header"><h2 class="entry-title">Sample</h2></header><div class="entry-content"><p>Sample synopsis.</p></div><div class="post-thumbnail"><figure><img src="/sample.jpg"></figure></div></article>"#;
const DETAILS_FIXTURE: &str = r#"<aside><header class="entry-header"><h1 class="entry-title">Sample</h1></header><div class="description"><p>Sample synopsis.</p></div></aside><div class="post-thumbnail"><img src="/sample.jpg"></div><div class="genres"><a>Drama</a></div><div class="sel-temp"><a data-post="1" data-season="1"></a></div>"#;
const SEASON_FIXTURE: &str = r#"<article class="post"><a href="/serie/sample/1"></a><header class="entry-header"><span class="num-epi">1x1</span></header></article>"#;
const WATCH_FIXTURE: &str = r##"<div class="aa-tbs-video"><a href="#tab1"><span class="server">Latino</span></a></div><div id="tab1"><iframe data-src="https://example.invalid/embed"></iframe></div>"##;
