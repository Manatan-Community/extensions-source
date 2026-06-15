use base64::{Engine, engine::general_purpose};
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

const SOURCE: Tiodonghua = Tiodonghua;
const BASE_URL: &str = "https://anime.tiodonghua.com";

struct Tiodonghua;

impl VideoSource for Tiodonghua {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let order = if listing(&request) == "latest" {
            "update"
        } else {
            "popular"
        };
        Ok(parse_cards(&fetch(
            &format!("{BASE_URL}/anime/?page={}&order={order}", page(&request)),
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
        let target = if query.is_empty() {
            format!("{BASE_URL}/anime/?page={}&order=popular", page(&request))
        } else {
            format!(
                "{BASE_URL}/page/{}/?s={}",
                page(&request),
                url::query_escape(query)
            )
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(parse_episodes(&fetch(
            &absolute_url(&path),
            DETAILS_FIXTURE,
            BASE_URL,
        )))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode")
            .unwrap_or_else(|| "/anime/sample-episode-1".to_string());
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
                title: "Actualizados".to_string(),
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
            if path.contains("/episode") || path.contains("/ver/") {
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
            .select(&selector("div.listupd article a.tip, article a.tip"))
            .filter_map(card)
            .collect(),
        has_next_page: doc
            .select(&selector("div.pagination a.next, div.hpage > a.r, a.next"))
            .next()
            .is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href");
    if href.is_empty() {
        return None;
    }
    Some(CatalogItem {
        key: path_key(&href),
        title: select_text(el, "div.tt, div.ttl, h2, h3")
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
        title: select_text_doc(&doc, "h1.entry-title, h1").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, "div.thumb > img, div.limage > img, img", "data-src")
            .or_else(|| select_attr_doc(&doc, "div.thumb > img, div.limage > img, img", "src"))
            .map(|src| absolute_url(&src)),
        description: select_text_doc(
            &doc,
            ".entry-content[itemprop=description], .desc, .entry-content",
        ),
        tags: select_texts_doc(
            &doc,
            "div.genxed > a, li a[href*='genre'], a[href*='/genre/']",
        ),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&body),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    doc.select(&selector(
        "div.eplister > ul > li > a, a[href*='episode'], a[href*='/ver/']",
    ))
    .filter_map(|el| {
        let href = attr(&el, "href");
        if href.is_empty() {
            return None;
        }
        let ep_text = select_text(el, ".epl-num").unwrap_or_else(|| text(el));
        let number = Regex::new(r#"([0-9]+(?:\.[0-9]+)?)"#)
            .unwrap()
            .captures(&ep_text)
            .and_then(|cap| cap.get(1))
            .and_then(|m| m.as_str().parse::<f32>().ok());
        Some(VideoEpisode {
            key: path_key(&href),
            title: Some(format!(
                "Episode {}",
                number
                    .map(trim_float)
                    .unwrap_or_else(|| title_from_path(&href))
            )),
            episode_number: number,
            url: Some(absolute_url(&href)),
            language: Some("es".to_string()),
            ..VideoEpisode::default()
        })
    })
    .collect()
}

fn parse_hosters(body: &str, referer: &str) -> Vec<VideoHoster> {
    let doc = Html::parse_document(body);
    doc.select(&selector(
        "select.mirror > option[data-index], select.mirror option[value], ul.mirror a[data-em]",
    ))
    .filter_map(|el| {
        let name = text(el).if_empty("External");
        let encoded = if el.value().name() == "option" {
            attr(&el, "value")
        } else {
            attr(&el, "data-em")
        };
        let embed = hoster_url(&encoded, referer)?;
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

fn hoster_url(encoded: &str, referer: &str) -> Option<String> {
    let body = if encoded.starts_with("http") {
        fetch(encoded, "", referer)
    } else {
        general_purpose::STANDARD
            .decode(encoded)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())?
    };
    let doc = Html::parse_document(&body);
    select_attr_doc(&doc, "#embed_holder iframe[src], iframe[src]", "src")
        .or_else(|| select_attr_doc(&doc, "meta[itemprop='embedUrl']", "content"))
        .map(|src| absolute_remote(&src, referer))
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
    .find_map(|pat| {
        let re = Regex::new(pat).ok()?;
        if pat.starts_with("http") {
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
    let quality = pref(request, "preferred_quality", "720p");
    streams.sort_by_key(|s| {
        let name = s.name.clone().unwrap_or_default();
        let q = s.quality.clone().unwrap_or_default();
        (
            q.contains(&quality) || name.contains(&quality),
            quality_rank(&q).max(quality_rank(&name)),
        )
    });
    streams.reverse();
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") || lower.contains("finalizado") {
        ItemStatus::Completed
    } else if lower.contains("ongoing") || lower.contains("emision") || lower.contains("emisión") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
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
        .and_then(|el| el.value().attr(name))
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
        .and_then(|el| el.value().attr(name))
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
        .find_map(|name| select_attr(el, "img", name))
        .map(|v| v.split_whitespace().next().unwrap_or("").to_string())
        .filter(|v| !v.is_empty() && !v.starts_with("data:image/"))
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
        .unwrap_or("Tiodonghua")
        .replace('-', " ")
}

fn trim_float(n: f32) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i32)
    } else {
        n.to_string()
    }
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

const LIST_FIXTURE: &str = r#"<div class="listupd"><article><a class="tip" href="/anime/sample"><div class="tt">Sample</div><img src="/cover.jpg"></a></article></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="entry-title">Sample</h1><div class="eplister"><ul><li><a href="/anime/sample-episode-1"><span class="epl-num">1</span></a></li></ul></div>"#;
const WATCH_FIXTURE: &str = r#"<select class="mirror"><option data-index="1" value="PGRpdiBpZD0iZW1iZWRfaG9sZGVyIj48aWZyYW1lIHNyYz0iaHR0cHM6Ly9leGFtcGxlLmludmFsaWQvZW1iZWQiPjwvaWZyYW1lPjwvZGl2Pg==">Voe</option></select>"#;

export_video_source!(SOURCE);
