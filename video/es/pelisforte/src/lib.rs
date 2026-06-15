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

const SOURCE: PelisForte = PelisForte;
const BASE_URL: &str = "https://www2.pelisforte.se";

struct PelisForte;

impl VideoSource for PelisForte {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_listing(&fetch(
            &format!("{BASE_URL}/todas-las-peliculas/page/{}", page(&request)),
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
                "{BASE_URL}/page/{}?s={}",
                page(&request),
                url::query_escape(query)
            )
        } else if let Some(genre) = filter(&request, "genre").filter(|value| !value.is_empty()) {
            format!("{BASE_URL}/{genre}")
        } else {
            format!("{BASE_URL}/todas-las-peliculas/page/{}", page(&request))
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/pelicula/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/pelicula/sample".to_string());
        Ok(vec![VideoEpisode {
            key: path.clone(),
            title: Some("Pelicula".to_string()),
            episode_number: Some(1.0),
            url: Some(absolute_url(&path)),
            language: Some("es".to_string()),
            ..VideoEpisode::default()
        }])
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/pelicula/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for iframe in doc.select(&selector(".video-player iframe")) {
            let parent_id = iframe
                .parent()
                .and_then(ElementRef::wrap)
                .and_then(|p| attr(&p, "id"))
                .unwrap_or_default();
            let tab_id = if parent_id.is_empty() {
                String::new()
            } else {
                doc.select(&selector(&format!("[href=\"#{parent_id}\"]")))
                    .next()
                    .and_then(|a| {
                        a.ancestors().filter_map(ElementRef::wrap).find(|e| {
                            attr(e, "class")
                                .unwrap_or_default()
                                .split_whitespace()
                                .any(|class| class == "lrt")
                        })
                    })
                    .and_then(|e| attr(&e, "id"))
                    .unwrap_or_default()
            };
            let lang_text = if tab_id.is_empty() {
                String::new()
            } else {
                select_text_doc(&doc, &format!("[tab=\"{tab_id}\"]")).unwrap_or_default()
            };
            let prefix = lang_prefix(&lang_text);
            let src = attr(&iframe, "src")
                .or_else(|| attr(&iframe, "data-src"))
                .unwrap_or_default();
            if src.is_empty() {
                continue;
            }
            let src = absolute_url(&src);
            let key = src.split("/?h=").nth(1).unwrap_or_default();
            let player = format!("https://{}/r.php?h={key}", host_name(&src));
            let locations = fetch(&player, PLAYER_FIXTURE, &src);
            for target in fetch_urls(&locations) {
                streams.extend(resolve_embed(
                    &target,
                    &format!("{prefix} {}", server_label(&target))
                        .trim()
                        .to_string(),
                    &src,
                    &request,
                ));
            }
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Todas las peliculas".to_string(),
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
        entries: doc
            .select(&selector("#movies-a li[id*=post-]"))
            .filter_map(card)
            .collect(),
        has_next_page: doc
            .select(&selector(
                ".pagination .nav-links .current ~ a:not(.page-link)",
            ))
            .next()
            .is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "article > a, a[href]", "href")?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_text(el, "article .entry-header .entry-title, h2, h3")
            .unwrap_or_else(|| title_from_path(&key)),
        cover: image_attr(el, "article .post-thumbnail figure img, article img, img"),
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
    let mut item = CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, ".alg-cr .entry-header .entry-title, h1")
            .unwrap_or_else(|| title_from_path(path)),
        description: select_text_doc(&doc, ".alg-cr .description, .description"),
        cover: image_attr_doc(
            &doc,
            ".alg-cr .post-thumbnail img, .post-thumbnail img, img",
        ),
        tags: select_texts_doc(&doc, ".genres a"),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    };
    for row in doc.select(&selector(".cast-lst li")) {
        let label = select_text(row, "span").unwrap_or_default();
        if label.contains("Director") {
            if let Some(value) = select_text(row, "p > a, a") {
                item.authors.push(value);
            }
        }
        if label.contains("Actores") {
            if let Some(value) = select_text(row, "p > a, a") {
                item.artists.push(value);
            }
        }
    }
    item
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
        return vec![stream(&src, name, "direct", &embed, false)];
    }
    vec![external_stream(&embed, name, referer)]
}

fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)["']"#,
        r#"src\s*:\s*["']([^"']+)["']"#,
        r#"<source[^>]+src=["']([^"']+)["']"#,
        r#"https?://[^\s'"\\]+\.m3u8[^\s'"\\]*"#,
    ]
    .into_iter()
    .find_map(|pattern| {
        Regex::new(pattern)
            .ok()?
            .captures(body)
            .and_then(|c| c.get(1).or_else(|| c.get(0)))
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
    let server = pref(request, "preferred_server", "StreamWish").to_ascii_lowercase();
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

fn select_attr_doc(doc: &Html, sel: &str, name: &str) -> Option<String> {
    doc.select(&selector(sel))
        .next()
        .and_then(|e| e.value().attr(name))
        .map(ToString::to_string)
}

fn image_attr(el: ElementRef<'_>, sel: &str) -> Option<String> {
    select_attr(el, sel, "srcset")
        .and_then(|s| fetch_urls(&s).last().cloned())
        .or_else(|| select_attr(el, sel, "data-src"))
        .or_else(|| select_attr(el, sel, "src"))
        .map(|s| absolute_url(&s))
}

fn image_attr_doc(doc: &Html, sel: &str) -> Option<String> {
    select_attr_doc(doc, sel, "srcset")
        .and_then(|s| fetch_urls(&s).last().cloned())
        .or_else(|| select_attr_doc(doc, sel, "data-src"))
        .or_else(|| select_attr_doc(doc, sel, "src"))
        .map(|s| absolute_url(&s))
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

fn fetch_urls(input: &str) -> Vec<String> {
    Regex::new(r#"https?://[^\s"',<>]+"#)
        .unwrap()
        .find_iter(input)
        .map(|m| m.as_str().trim_matches('"').to_string())
        .collect()
}

fn absolute_url(input: &str) -> String {
    absolute_remote(input, BASE_URL)
}

fn absolute_remote(input: &str, base: &str) -> String {
    let value = input.trim().replace("\\/", "/").replace("&amp;", "&");
    if value.starts_with("http://") || value.starts_with("https://") {
        value
    } else if let Some(rest) = value.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url::join_url(base, &value)
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
        .and_then(|filters| filters.get(key))
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
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn lang_prefix(input: &str) -> &'static str {
    let lower = input.to_ascii_lowercase();
    if lower.contains("latino") {
        "[LAT]"
    } else if lower.contains("subtitulado") || lower.contains("sub") {
        "[SUB]"
    } else if lower.contains("castellano") {
        "[CAST]"
    } else {
        ""
    }
}

fn server_label(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    for (label, keys) in [
        ("Voe", &["voe", "yip."][..]),
        ("Okru", &["ok.ru", "okru"][..]),
        ("Filemoon", &["filemoon", "moonplayer", "files.im"][..]),
        ("Uqload", &["uqload"][..]),
        ("Mp4Upload", &["mp4upload"][..]),
        (
            "StreamWish",
            &["wishembed", "streamwish", "strwish", "wish"][..],
        ),
        ("Doodstream", &["doodstream", "dood.", "d000d"][..]),
        ("StreamTape", &["streamtape", "stape", "shavetape"][..]),
        ("VidGuard", &["vembed", "guard", "bembed"][..]),
        ("YourUpload", &["yourupload", "upload"][..]),
        ("BurstCloud", &["burstcloud", "burst"][..]),
        ("Fastream", &["fastream"][..]),
        ("Upstream", &["upstream"][..]),
    ] {
        if keys.iter().any(|key| lower.contains(key)) {
            return label.to_string();
        }
    }
    host_name(input)
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
        .unwrap_or("PelisForte")
        .replace('-', " ")
}

const LIST_FIXTURE: &str = r#"
<ul id="movies-a"><li id="post-1"><article><a href="/pelicula/sample"><div class="entry-header"><h2 class="entry-title">Sample Movie</h2></div><div class="post-thumbnail"><figure><img src="/cover.jpg"></figure></div></a></article></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<section class="alg-cr"><div class="entry-header"><h1 class="entry-title">Sample Movie</h1></div><div class="description">Fixture details for local smoke tests.</div><div class="post-thumbnail"><img src="/cover.jpg"></div></section>
<div class="genres"><a>Drama</a></div><ul class="cast-lst"><li><span>Director</span><p><a>Director Name</a></p></li></ul>
"#;

const WATCH_FIXTURE: &str = r##"
<div class="lrt" id="tab-lat"><a href="#player1">LAT</a></div><div tab="tab-lat">Latino</div>
<div id="player1" class="video-player"><iframe src="https://pelisforte-player.test/?h=sample"></iframe></div>
"##;

const PLAYER_FIXTURE: &str = r#"https://streamwish.to/e/sample"#;

export_video_source!(SOURCE);
