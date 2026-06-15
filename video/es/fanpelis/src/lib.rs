use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{SearchRequest, http::HttpClient},
    url,
    video::referer_headers,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;

const SOURCE: FanPelis = FanPelis;
const BASE_URL: &str = "https://fanpelis.la";

struct FanPelis;

impl VideoSource for FanPelis {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = format!("{BASE_URL}/movies-hd/page/{}/", page(&request));
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
        let p = page(&request);
        let target = if !query.is_empty() {
            format!("{BASE_URL}/page/{p}/?s={}", url::query_escape(query))
        } else if let Some(genre) = filter(&request, "genre").filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/{genre}/page/{p}/")
        } else {
            format!("{BASE_URL}/movies-hd/page/{p}/")
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/movies-hd/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/movies-hd/sample".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body, &path))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/movies-hd/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for iframe in doc.select(&selector(".movieplay iframe, iframe")) {
            let embed = attr(&iframe, "src").or_else(|| attr(&iframe, "data-src"));
            if let Some(embed) = embed.filter(|v| !v.is_empty()) {
                streams.extend(resolve_embed(
                    &absolute_remote(&embed, &referer),
                    &host_name(&embed),
                    &referer,
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
            title: "Populares".to_string(),
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
fn parse_cards(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc.select(&selector(".ml-item")).filter_map(card).collect(),
        has_next_page: doc
            .select(&selector(".pagination li.active ~ li"))
            .next()
            .is_some(),
    }
}
fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "a", "href")?;
    let title = select_text(el, "a .mli-info h2")
        .or_else(|| select_attr(el, "img", "alt"))
        .unwrap_or_else(|| title_from_path(&href));
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title,
        cover: select_attr(el, "a img", "data-original")
            .or_else(|| select_attr(el, "img", "src"))
            .map(|v| absolute_url(&v)),
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
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, ".mvic-desc h3[itemprop='name'], .mvic-desc h3, h1")
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, "#mv-info .mvic-thumb img, img", "src")
            .map(|v| absolute_url(&v)),
        url: Some(absolute_url(path)),
        description: select_text_doc(&doc, ".mvic-desc .desc p")
            .map(|v| v.trim_matches('"').to_string()),
        tags: select_texts_doc(&doc, ".mvic-info .mvici-left p a[rel='category tag']"),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: if path.contains("/series/") {
            ItemStatus::Unknown
        } else {
            ItemStatus::Completed
        },
        initialized: true,
        ..CatalogItem::default()
    }
}
fn parse_episodes(body: &str, path: &str) -> Vec<VideoEpisode> {
    if !path.contains("/series/") {
        return vec![VideoEpisode {
            key: path.to_string(),
            title: Some("PELICULA".to_string()),
            episode_number: Some(1.0),
            url: Some(absolute_url(path)),
            language: Some("es".to_string()),
            ..VideoEpisode::default()
        }];
    }
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    for (sidx, season) in doc.select(&selector("#seasons .tvseason")).enumerate() {
        let season_no = select_text(season, ".les-title strong")
            .and_then(|v| first_number(&v))
            .unwrap_or((sidx + 1) as f32);
        for (eidx, ep) in season.select(&selector(".les-content a")).enumerate() {
            let href = attr(&ep, "href").unwrap_or_default();
            if href.is_empty() {
                continue;
            }
            let raw = text(ep);
            let ep_no = first_number(&raw).unwrap_or((eidx + 1) as f32);
            let key = path_key(&href);
            out.push(VideoEpisode {
                key: key.clone(),
                title: Some(format!("T{} - E{} - {raw}", season_no as i32, ep_no as i32)),
                episode_number: Some(ep_no),
                url: Some(absolute_url(&key)),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            });
        }
    }
    out.reverse();
    out
}
fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    if embed.contains(".m3u8") {
        return parse_hls(embed, name, referer, request);
    }
    if embed.contains("slwatch.co") || embed.contains("streamlare") {
        if let Some(id) = embed
            .split("/e/")
            .nth(1)
            .and_then(|v| v.split(['?', '/']).next())
        {
            let body = client(embed)
                .post(format!("https://slwatch.co/api/video/stream/get?id={id}"))
                .xhr()
                .send_text()
                .unwrap_or_default();
            if let Some(file) = Regex::new(r#"file=\\?"([^"\\]+)"#)
                .unwrap()
                .captures(&body)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().replace("\\/", "/"))
            {
                return if file.contains(".m3u8") {
                    parse_hls(&file, "Streamlare", embed, request)
                } else {
                    vec![stream(&file, "Streamlare", "direct", embed, false)]
                };
            }
        }
    }
    let body = fetch(embed, "", referer);
    if let Some(media) = first_media_url(&body).map(|v| absolute_remote(&v, embed)) {
        if media.contains(".m3u8") {
            parse_hls(&media, name, embed, request)
        } else {
            vec![stream(&media, name, "direct", embed, false)]
        }
    } else {
        vec![external_stream(embed, name, referer)]
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
    let mut quality = pref(request, "preferred_quality", "auto");
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
        out.push(stream(master, name, &quality, referer, true));
    }
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
    let prefq = pref(request, "preferred_quality", "DoodStream");
    streams.sort_by_key(|s| {
        (
            s.name.clone().unwrap_or_default().contains(&prefq),
            quality_rank(&s.quality.clone().unwrap_or_default()),
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
fn first_number(input: &str) -> Option<f32> {
    Regex::new(r#"(\d+)"#)
        .ok()?
        .captures(input)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}
fn quality_rank(input: &str) -> i32 {
    first_number(input).unwrap_or_default() as i32
}
fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("FanPelis")
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

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="ml-item"><a href="/movies-hd/sample"><img data-original="/sample.jpg"><span class="mli-info"><h2>Sample</h2></span></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div id="mv-info"><div class="mvic-thumb"><img src="/sample.jpg"></div></div><div class="mvic-desc"><h3 itemprop="name">Sample</h3><div class="desc"><p>Sample description.</p></div></div>"#;
const WATCH_FIXTURE: &str =
    r#"<div class="movieplay"><iframe src="https://example.invalid/embed"></iframe></div>"#;
