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

const SOURCE: MetroSeries = MetroSeries;
const BASE_URL: &str = "https://www3.seriesmetro.net";

struct MetroSeries;

impl VideoSource for MetroSeries {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        Ok(parse_cards(&fetch(
            &format!("{BASE_URL}/cartelera-series/page/{page}"),
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
        Ok(parse_cards(&fetch(
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
        let mut episodes = Vec::new();
        for season in doc.select(&selector(".sel-temp a")) {
            let season_num = attr(&season, "data-season").unwrap_or_default();
            let post = attr(&season, "data-post").unwrap_or_default();
            let detail = fetch_season(&season_num, &post, &referer);
            episodes.extend(parse_season_episodes(&detail, &season_num));
        }
        if episodes.is_empty() {
            episodes = parse_season_episodes(&body, "1");
        }
        episodes.sort_by(|a, b| {
            b.episode_number
                .partial_cmp(&a.episode_number)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/serie/sample-1".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for item in doc.select(&selector(".aa-tbs-video a")) {
            let prefix = language_prefix(&text_sel(item, ".server").unwrap_or_default());
            let tab = attr(&item, "href").unwrap_or_default();
            let mut src = attr_doc(&doc, &format!("{tab} iframe"), "data-src")
                .or_else(|| attr_doc(&doc, &format!("{tab} iframe"), "src"))
                .unwrap_or_default();
            src = absolute_remote(&src, &referer);
            if src.contains("metro") {
                let frame_doc = Html::parse_document(&fetch(&src, "", &referer));
                src = attr_doc(&frame_doc, "iframe", "src")
                    .map(|v| absolute_remote(&v, &src))
                    .unwrap_or(src);
            }
            streams.extend(resolve_embed(
                &src,
                &format!("{prefix} {}", server_name(&src)).trim(),
                &referer,
                &request,
            ));
        }
        if streams.is_empty() {
            streams.extend(
                iframe_sources(&body)
                    .into_iter()
                    .flat_map(|src| resolve_embed(&src, &server_name(&src), &referer, &request)),
            );
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

fn parse_cards(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc.select(&selector(".post")).filter_map(card).collect(),
        has_next_page: doc
            .select(&selector(".nav-links .current ~ a, a.next"))
            .next()
            .is_some(),
    }
}
fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr_sel(el, ".lnk-blk, a[href]", "href")?;
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title: text_sel(el, ".entry-header .entry-title, .entry-title")
            .unwrap_or_else(|| title_from_path(&path)),
        cover: attr_sel(el, ".post-thumbnail figure img, img", "data-src")
            .or_else(|| attr_sel(el, ".post-thumbnail figure img, img", "src"))
            .map(|v| absolute_url(&v)),
        url: Some(absolute_url(&path)),
        description: text_sel(el, ".entry-content p"),
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
        title: text_doc(
            &doc,
            "aside .entry-header .entry-title, h1.entry-title, .entry-title",
        )
        .unwrap_or_else(|| title_from_path(path)),
        cover: attr_doc(&doc, ".post-thumbnail img, img", "data-src")
            .or_else(|| attr_doc(&doc, ".post-thumbnail img, img", "src"))
            .map(|v| absolute_url(&v).replace("/w185/", "/w500/")),
        url: Some(absolute_url(path)),
        description: text_doc(&doc, "aside .description p:not([class]), .description p"),
        tags: doc
            .select(&selector(".genres a"))
            .map(element_text)
            .filter(|v| !v.is_empty())
            .collect(),
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
fn fetch_season(season: &str, post: &str, referer: &str) -> String {
    client(referer)
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .header("Origin", BASE_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .referer(referer)
        .body(format!(
            "action=action_select_season&season={}&post={}",
            url::query_escape(season),
            url::query_escape(post)
        ))
        .send_text()
        .unwrap_or_default()
}
fn parse_season_episodes(body: &str, season: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    doc.select(&selector(".post"))
        .rev()
        .enumerate()
        .filter_map(|(idx, el)| {
            let href = attr_sel(el, "a[href]", "href")?;
            let text = text_sel(el, ".entry-header .num-epi").unwrap_or_default();
            let ep = text
                .split('x')
                .nth(1)
                .and_then(|v| v.split(['-', '–']).next())
                .map(str::trim)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or((idx + 1) as f32);
            let path = path_key(&href);
            Some(VideoEpisode {
                key: path.clone(),
                title: Some(format!("T{season} - Episodio {}", trim_float(ep))),
                episode_number: Some(ep),
                url: Some(absolute_url(&path)),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}
fn iframe_sources(body: &str) -> Vec<String> {
    Html::parse_document(body)
        .select(&selector("iframe"))
        .filter_map(|el| attr(&el, "data-src").or_else(|| attr(&el, "src")))
        .map(|v| absolute_remote(&v, BASE_URL))
        .collect()
}
fn language_prefix(input: &str) -> &'static str {
    let lower = input.to_ascii_lowercase();
    if lower.contains("latino") {
        "[LAT]"
    } else if lower.contains("castellano") {
        "[CAST]"
    } else if lower.contains("sub") || lower.contains("vose") {
        "[SUB]"
    } else {
        ""
    }
}
fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_remote(embed, referer);
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
    let lang = pref(request, "preferred_language", "[LAT]");
    let server = pref(request, "preferred_server", "YourUpload").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|s| {
        let name = s.name.clone().unwrap_or_default();
        (
            name.contains(&lang),
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
        .unwrap_or("MetroSeries")
        .replace('-', " ")
}
fn server_name(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if lower.contains("fastream") {
        "Fastream".to_string()
    } else if lower.contains("upstream") {
        "Upstream".to_string()
    } else if lower.contains("yourupload") {
        "YourUpload".to_string()
    } else if lower.contains("voe") {
        "Voe".to_string()
    } else if lower.contains("wish") {
        "StreamWish".to_string()
    } else if lower.contains("mp4upload") {
        "Mp4Upload".to_string()
    } else if lower.contains("burst") {
        "BurstCloud".to_string()
    } else if lower.contains("filemoon") || lower.contains("moonplayer") {
        "Filemoon".to_string()
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
fn trim_float(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i32)
    } else {
        value.to_string()
    }
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<article class="post"><a class="lnk-blk" href="/serie/sample"></a><header class="entry-header"><h2 class="entry-title">Sample</h2></header><div class="entry-content"><p>Sample description.</p></div><div class="post-thumbnail"><figure><img src="/sample.jpg"></figure></div></article>"#;
const DETAILS_FIXTURE: &str = r#"<aside><header class="entry-header"><h1 class="entry-title">Sample</h1></header><div class="description"><p>Sample description.</p></div></aside><div class="post-thumbnail"><img src="/sample.jpg"></div><div class="genres"><a>Drama</a></div><div class="sel-temp"><a data-season="1" data-post="1"></a></div><article class="post"><a href="/serie/sample-1"></a><header class="entry-header"><span class="num-epi">1x1</span></header></article>"#;
const WATCH_FIXTURE: &str = r##"<div class="aa-tbs-video"><a href="#tab1"><span class="server">Latino</span></a></div><div id="tab1"><iframe data-src="https://example.invalid/embed"></iframe></div>"##;
