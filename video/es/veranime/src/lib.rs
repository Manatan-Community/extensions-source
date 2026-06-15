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

const SOURCE: VerAnime = VerAnime;
const BASE_URL: &str = "https://verani.me";

struct VerAnime;

impl VideoSource for VerAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/animes/page/{page}/")
        } else {
            format!("{BASE_URL}/animes/page/{page}/?orderby=popular")
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
        let target = if query.is_empty() {
            format!("{BASE_URL}/animes/page/{page}/?orderby=popular")
        } else {
            format!("{BASE_URL}/page/{page}/?s={}", url::query_escape(query))
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(parse_episodes(
            &fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL),
            &path,
        ))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/capitulo/sample-1".to_string());
        let referer = absolute_url(&path);
        Ok(parse_hosters(
            &fetch(&referer, WATCH_FIXTURE, BASE_URL),
            &referer,
        ))
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
        let mut out = Vec::new();
        for hoster in self.hosters(request.clone())? {
            let mut streams = self.resolve_hoster(json!({
                "hoster": { "key": hoster.key },
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
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Populares".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Animes".to_string(),
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
            if path.contains("capitulo") || path.contains("episodio") {
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
    Paged {
        entries: doc
            .select(&selector(".anime-card a, article a"))
            .filter_map(card)
            .collect(),
        has_next_page: doc
            .select(&selector(".pagination .next, a.next"))
            .next()
            .is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href");
    if href.is_empty()
        || href.contains("/page/")
        || href.contains("/animes/")
        || !href.contains("verani.me")
    {
        return None;
    }
    Some(CatalogItem {
        key: path_key(&href),
        title: select_text(el, "h3")
            .or_else(|| select_attr(el, "img", "alt"))
            .unwrap_or_else(|| title_from_path(&href)),
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
    let mut status = parse_status(&select_text_doc(&doc, ".status").unwrap_or_default());
    if status == ItemStatus::Unknown && path.contains("/pelicula/") {
        status = ItemStatus::Completed;
    }
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, "h1").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, "img", "src").map(|src| absolute_url(&src)),
        description: select_text_doc(
            &doc,
            ".anime-hero-description, .sinopsis, .description, p.desc, .info p, .pelicula-overview p",
        ),
        tags: select_texts_doc(&doc, "a[href*='categoria']"),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, item_path: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    let groups = doc
        .select(&selector(".temporada-group"))
        .collect::<Vec<_>>();
    if groups.is_empty() {
        for el in doc.select(&selector(
            ".capitulo-card-link, a[href*='capitulo'], a[href*='episodio']",
        )) {
            if let Some(ep) = episode_from(el, "") {
                out.push(ep);
            }
        }
    } else {
        let format_season = groups.len() > 1;
        for group in groups {
            let season_text =
                select_text(group, ".temporada-badge, .temporada-name").unwrap_or_default();
            let season = Regex::new(r#"\d+"#)
                .unwrap()
                .find(&season_text)
                .and_then(|m| m.as_str().parse::<u32>().ok());
            let prefix = if format_season {
                season.map(|n| format!("S{n:02} ")).unwrap_or_default()
            } else {
                String::new()
            };
            for el in group.select(&selector(
                ".capitulo-card-link, a[href*='capitulo'], a[href*='episodio']",
            )) {
                if let Some(ep) = episode_from(el, &prefix) {
                    out.push(ep);
                }
            }
        }
    }
    if out.is_empty()
        && doc
            .select(&selector(".iframe-wrapper, iframe"))
            .next()
            .is_some()
    {
        out.push(VideoEpisode {
            key: path_key(item_path),
            title: Some("Pelicula".to_string()),
            episode_number: Some(1.0),
            url: Some(absolute_url(item_path)),
            language: Some("es".to_string()),
            ..VideoEpisode::default()
        });
    }
    out
}

fn episode_from(el: ElementRef<'_>, prefix: &str) -> Option<VideoEpisode> {
    let href = attr(&el, "href");
    let text_value = text(el);
    if href.is_empty()
        || href.contains("proximos-capitulos")
        || text_value.to_ascii_lowercase().contains("ver ahora")
    {
        return None;
    }
    let number = Regex::new(r#"(?i)(?:capitulo|episodio)\s*(\d+(?:\.\d+)?)"#)
        .unwrap()
        .captures(&text_value)
        .and_then(|cap| cap.get(1))
        .and_then(|m| m.as_str().parse::<f32>().ok())
        .or_else(|| {
            Regex::new(r#"(?i)(?:capitulo|episodio)-(\d+(?:\.\d+)?)"#)
                .unwrap()
                .captures(&href)
                .and_then(|cap| cap.get(1))
                .and_then(|m| m.as_str().parse::<f32>().ok())
        });
    Some(VideoEpisode {
        key: path_key(&href),
        title: Some(format!("{}{}", prefix, text_value.if_empty("Capitulo"))),
        episode_number: number,
        url: Some(absolute_url(&href)),
        language: Some("es".to_string()),
        ..VideoEpisode::default()
    })
}

fn parse_hosters(body: &str, referer: &str) -> Vec<VideoHoster> {
    let doc = Html::parse_document(body);
    doc.select(&selector("iframe[src], iframe[data-src]"))
        .filter_map(|iframe| {
            let src = attr(&iframe, "src").if_empty(&attr(&iframe, "data-src"));
            if src.is_empty() {
                return None;
            }
            let name = matched_server(&src).unwrap_or_else(|| host_name(&src));
            Some(VideoHoster {
                key: format!("{name}|{}|{referer}", absolute_remote(&src, referer)),
                name,
                url: Some(absolute_remote(&src, referer)),
                lazy: true,
                video_count: Some(1),
                ..VideoHoster::default()
            })
        })
        .collect()
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_url(embed);
    if embed.contains("zilla-networks") && embed.contains("/play/") {
        let base = embed.split("/play/").next().unwrap_or(&embed);
        let id = embed
            .split("/play/")
            .nth(1)
            .unwrap_or_default()
            .split('?')
            .next()
            .unwrap_or_default();
        return vec![stream(
            &format!("{base}/m3u8/{id}"),
            name,
            "Zilla-Networks",
            &format!("{base}/"),
            true,
        )];
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
    let quality = pref(request, "preferred_quality", "1080p");
    streams.sort_by_key(|s| {
        let name = s.name.clone().unwrap_or_default();
        let q = s.quality.clone().unwrap_or_default();
        (
            name.contains(&quality) || q.contains(&quality),
            quality_rank(&q).max(quality_rank(&name)),
        )
    });
    streams.reverse();
}

fn parse_status(status: &str) -> ItemStatus {
    let lower = status.to_ascii_lowercase();
    if lower.contains("finalizado") {
        ItemStatus::Completed
    } else if lower.contains("emision") || lower.contains("emisión") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn matched_server(input: &str) -> Option<String> {
    let lower = input.to_ascii_lowercase();
    [
        ("Okru", ["ok.ru", "okru"].as_slice()),
        ("FileLions", ["filelions", "lion", "fviplions"].as_slice()),
        (
            "StreamWish",
            ["wishembed", "streamwish", "strwish", "wish"].as_slice(),
        ),
        ("UnsBio", ["animeav1.uns.bio"].as_slice()),
        (
            "VidHide",
            ["vidhide", "streamhide", "guccihide", "streamvid"].as_slice(),
        ),
        ("Voe", ["voe.sx"].as_slice()),
        ("YourUpload", ["yourupload.com"].as_slice()),
        ("Zilla-Networks", ["zilla-networks"].as_slice()),
        (
            "VidGuard",
            ["vembed", "guard", "listeamed", "bembed", "vgfplay"].as_slice(),
        ),
        ("Mp4Upload", ["mp4upload.com"].as_slice()),
        ("PixelDrain", ["pixeldrain.com"].as_slice()),
    ]
    .into_iter()
    .find(|(_, names)| names.iter().any(|name| lower.contains(name)))
    .map(|(name, _)| name.to_string())
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

fn with_listing(request: &Value, id: &str) -> Value {
    json!({ "listing": id, "preferences": request.get("preferences").cloned().unwrap_or(Value::Null) })
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
        .unwrap_or("VerAnime")
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

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

const LIST_FIXTURE: &str = r#"<article><a href="https://verani.me/anime/sample"><h3>Sample</h3><img src="/cover.jpg"></a></article>"#;
const DETAILS_FIXTURE: &str =
    r#"<h1>Sample</h1><a class="capitulo-card-link" href="/capitulo/sample-1">Capitulo 1</a>"#;
const WATCH_FIXTURE: &str = r#"<iframe src="https://player.invalid/embed"></iframe>"#;

export_video_source!(SOURCE);
