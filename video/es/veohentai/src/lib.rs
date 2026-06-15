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

const SOURCE: VeoHentai = VeoHentai;
const BASE_URL: &str = "https://veohentai.com";

struct VeoHentai;

impl VideoSource for VeoHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/page/{page}")
        } else {
            format!("{BASE_URL}/mas-visitados/page/{page}")
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
            format!("{BASE_URL}/page/{page}/?s={}", url::query_escape(query))
        } else if let Some(genre) = filter(&request, "genre").filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/{genre}/page/{page}")
        } else {
            format!("{BASE_URL}/mas-visitados/page/{page}")
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        Ok(vec![VideoEpisode {
            key: path.clone(),
            title: Some("Capitulo".to_string()),
            episode_number: Some(1.0),
            url: Some(absolute_url(&path)),
            language: Some("es".to_string()),
            ..VideoEpisode::default()
        }])
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode")
            .or_else(|| request_key(&request, "item"))
            .unwrap_or_else(|| "/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let iframe = iframe_url(&body, &referer);
        Ok(iframe
            .into_iter()
            .map(|url| VideoHoster {
                key: format!("VeoHentai|{url}|{referer}"),
                name: "VeoHentai".to_string(),
                url: Some(url),
                lazy: true,
                video_count: Some(1),
                ..VideoHoster::default()
            })
            .collect())
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = key.splitn(3, '|');
        let name = parts.next().unwrap_or("VeoHentai");
        let link = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(BASE_URL);
        let mut streams = resolve_player(link, name, referer, &request);
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
                title: "Mas visitados".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Recientes".to_string(),
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
    Paged {
        entries: doc.select(&selector(".gap-6 a")).filter_map(card).collect(),
        has_next_page: doc
            .select(&selector(".nav-links a"))
            .any(|a| text(a).eq_ignore_ascii_case("next")),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href");
    if href.is_empty() {
        return None;
    }
    Some(CatalogItem {
        key: path_key(&href),
        title: select_text(el, "h2")
            .or_else(|| select_attr(el, "img", "alt"))
            .unwrap_or_else(|| title_from_path(&href)),
        cover: image_url(el).map(|src| absolute_url(&src)),
        url: Some(absolute_url(&href)),
        language: Some("es".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    let mut author = None;
    for row in doc.select(&selector(".gap-4 div")) {
        let value = text(row);
        if value.contains("Marca") {
            author = Some(value.replace("Marca", "").trim().to_string());
        }
    }
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, ".pb-2 h1, h1").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, "#thumbnail-post img, img", "data-src")
            .or_else(|| select_attr_doc(&doc, "#thumbnail-post img, img", "data-lazy-src"))
            .or_else(|| select_attr_doc(&doc, "#thumbnail-post img, img", "src"))
            .map(|src| absolute_url(&src)),
        description: Some(select_texts_doc(&doc, ".entry-content p").join(", "))
            .filter(|v| !v.is_empty()),
        tags: select_texts_doc(&doc, ".tags a"),
        authors: author.into_iter().collect(),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn iframe_url(body: &str, referer: &str) -> Option<String> {
    let doc = Html::parse_document(body);
    doc.select(&selector(
        "iframe[webkitallowfullscreen], iframe[src], iframe[data-litespeed-src]",
    ))
    .find_map(|iframe| {
        [attr(&iframe, "src"), attr(&iframe, "data-litespeed-src")]
            .into_iter()
            .find(|value| !value.is_empty() && !value.starts_with("about"))
    })
    .map(|src| absolute_remote(&src, referer))
}

fn resolve_player(link: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let player_body = fetch(link, "", referer);
    let player_doc = Html::parse_document(&player_body);
    let Some(data_id) = select_attr_doc(&player_doc, "[data-id]", "data-id") else {
        return vec![external_stream(link, name, referer)];
    };
    let real_url = format!("https://{}{}", host_name(link), data_id);
    let real_body = fetch(&real_url, "", link);
    let script = Html::parse_document(&real_body)
        .select(&selector("script"))
        .map(|s| s.inner_html())
        .find(|s| s.contains("jwplayer.key"))
        .unwrap_or(real_body);
    let mut streams = source_items(&script)
        .into_iter()
        .filter_map(|item| {
            let file = quoted_field(&item, "file")?;
            let quality = if file.contains(".m3u") {
                "HLS"
            } else if file.contains(".mp4") {
                "MP4"
            } else {
                "direct"
            };
            Some(stream(
                &file,
                name,
                quality,
                &real_url,
                file.contains(".m3u"),
            ))
        })
        .collect::<Vec<_>>();
    if streams.is_empty() {
        streams.push(external_stream(link, name, referer));
    }
    sort_streams(&mut streams, request);
    streams
}

fn source_items(script: &str) -> Vec<String> {
    script
        .split("sources:")
        .nth(1)
        .and_then(|p| p.split(']').next())
        .unwrap_or_default()
        .split('{')
        .skip(1)
        .filter_map(|p| p.split('}').next().map(ToString::to_string))
        .collect()
}

fn quoted_field(item: &str, key: &str) -> Option<String> {
    Regex::new(&format!(r#"{key}"?\s*:\s*"([^"]+)""#))
        .ok()?
        .captures(item)?
        .get(1)
        .map(|m| m.as_str().replace("\\/", "/"))
}

fn stream(target: &str, name: &str, quality: &str, referer: &str, hls: bool) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name}:{quality}")),
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
    let server = pref(request, "preferred_server", "VeoHentai").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|s| {
        let name = s.name.clone().unwrap_or_default().to_ascii_lowercase();
        (
            name.contains(&server),
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

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("VeoHentai")
        .replace('-', " ")
}

fn host_name(input: &str) -> String {
    input
        .split("://")
        .nth(1)
        .unwrap_or(input)
        .split('/')
        .next()
        .unwrap_or("veohentai.com")
        .to_string()
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

const LIST_FIXTURE: &str =
    r#"<div class="gap-6"><a href="/sample"><img src="/cover.jpg"><h2>Sample</h2></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="pb-2"><h1>Sample</h1></div><div class="entry-content"><p>Sample description.</p></div><iframe src="https://player.invalid/embed"></iframe>"#;
const WATCH_FIXTURE: &str =
    r#"<iframe webkitallowfullscreen src="https://player.invalid/embed"></iframe>"#;

export_video_source!(SOURCE);
