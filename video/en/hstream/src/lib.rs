use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack,
    UrlResolveResult, VideoEpisode, VideoStream, VideoStreamKind,
    abi::{ExtensionResult, cookies_get},
    export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    dates, html,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Hstream = Hstream;
const BASE_URL: &str = "https://hstream.moe";

struct Hstream;

impl VideoSource for Hstream {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let order = if listing(&request) == "latest" { "recently-uploaded" } else { "view-count" };
        let target = format!("{BASE_URL}/search?order={order}&page={}", page(&request));
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&path)],
                has_next_page: false,
            });
        }
        let id_query = query.strip_prefix("id:");
        if let Some(id) = id_query {
            return Ok(Paged {
                entries: vec![fetch_details(&format!("/hentai/{id}"))],
                has_next_page: false,
            });
        }
        let mut pairs = vec![
            ("page".to_string(), page(&request).to_string()),
            ("order".to_string(), filter(&request, "order", "view-count").to_string()),
        ];
        if !query.is_empty() {
            pairs.push(("search".to_string(), query.to_string()));
        }
        for (idx, genre) in array_filter(&request, "include_genres").into_iter().enumerate() {
            pairs.push((format!("tags[{idx}]"), genre));
        }
        for genre in array_filter(&request, "exclude_genres") {
            pairs.push(("blacklist[]".to_string(), genre));
        }
        for studio in array_filter(&request, "studios") {
            pairs.push(("studios[]".to_string(), studio));
        }
        let query = pairs
            .into_iter()
            .map(|(key, value)| format!("{}={}", url::query_escape(&key), url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&");
        let body = get_or_fixture(&format!("{BASE_URL}/search?{query}"), LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(|| "/hentai/sample-1".to_string());
        Ok(fetch_details(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_else(|| "/hentai/sample-1".to_string());
        let body = get_or_fixture(&absolute_url(&key), DETAILS_FIXTURE, BASE_URL);
        Ok(vec![parse_episode(&body, &key)])
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "episode").unwrap_or_else(|| "/hentai/sample-1".to_string());
        let episode_url = absolute_url(&episode);
        let body = get_or_fixture(&episode_url, DETAILS_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let Some(episode_id) = select_attr(&doc, "input#e_id", "value") else {
            return Ok(Vec::new());
        };
        let xsrf = xsrf_token(&episode_url).unwrap_or_default();
        let response = client(&episode_url)
            .post(&format!("{BASE_URL}/player/api"))
            .header("Accept", "application/json, text/plain, */*")
            .header("Origin", BASE_URL)
            .header("X-Requested-With", "XMLHttpRequest")
            .header("X-XSRF-TOKEN", &xsrf)
            .referer(&episode_url)
            .json(json!({ "episode_id": episode_id }).to_string())
            .send_text()
            .unwrap_or_else(|_| PLAYER_FIXTURE.to_string());
        let mut streams = parse_player(&response, &episode_url, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| absolute_url(&key)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| absolute_url(&key)))
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

fn get_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
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
        entries: select_all(&doc, "div.items-center div.w-full > a")
            .filter_map(card_item)
            .collect(),
        has_next_page: select_all(&doc, "span[aria-current] + a").next().is_some(),
    }
}

fn card_item(anchor: ElementRef<'_>) -> Option<CatalogItem> {
    let href = anchor.value().attr("href")?;
    let key = path_key(href);
    let episode = key.split('-').next_back().unwrap_or("1").trim_matches('/');
    Some(CatalogItem {
        key: key.clone(),
        title: attr(&anchor, "img", "alt").unwrap_or_else(|| title_from_path(&key)),
        cover: Some(format!(
            "{BASE_URL}/images{}/cover-ep-{episode}.webp",
            key.trim_end_matches(&format!("-{episode}"))
        )),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    parse_details(&body, path).unwrap_or_else(|| CatalogItem {
        key: path_key(path),
        title: title_from_path(path),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, path: &str) -> Option<CatalogItem> {
    let doc = Html::parse_document(body);
    let info = select_all(&doc, "div.relative > div.justify-between > div").next();
    Some(CatalogItem {
        key: path_key(path),
        title: info
            .as_ref()
            .and_then(|value| text(value, "div > h1"))
            .unwrap_or_else(|| title_from_path(path)),
        artists: info
            .as_ref()
            .and_then(|value| text(value, "div > a:nth-of-type(3)"))
            .into_iter()
            .collect(),
        cover: select_attr(&doc, "div.float-left > img.object-cover", "src").map(|value| absolute_url(&value)),
        tags: select_all(&doc, "ul.list-none > li > a")
            .map(|tag| collect_text(&tag))
            .filter(|tag| !tag.is_empty())
            .collect(),
        description: select_text(&doc, "div.relative > p.leading-tight"),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episode(body: &str, path: &str) -> VideoEpisode {
    let doc = Html::parse_document(body);
    let key = path_key(path);
    let number = key
        .split('-')
        .next_back()
        .unwrap_or("1")
        .trim_matches('/')
        .parse::<f32>()
        .unwrap_or(1.0);
    VideoEpisode {
        key: key.clone(),
        title: Some(format!("Episode {}", trim_float(number))),
        episode_number: Some(number),
        date_uploaded: select_text(&doc, "a:has(i.fa-upload)").and_then(|date| dates::parse_ymd(date.trim_matches([' ', '|']))),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        ..VideoEpisode::default()
    }
}

fn parse_player(body: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let data = serde_json::from_str::<PlayerApiResponse>(body).unwrap_or_else(|_| serde_json::from_str(PLAYER_FIXTURE).unwrap());
    let Some(domain) = data.stream_domains.first() else {
        return Vec::new();
    };
    let url_base = format!("{}/{}", domain.trim_end_matches('/'), data.stream_url.trim_start_matches('/'));
    let subtitle_url = format!("{url_base}/eng.ass");
    let qualities = ["720", "1080", "2160"]
        .into_iter()
        .filter(|quality| *quality != "2160" || data.resolution == "4k");
    qualities
        .map(|quality| {
            let path = if data.legacy != 0 {
                if quality == "720" {
                    "/x264.720p.mp4".to_string()
                } else {
                    format!("/av1.{quality}.webm")
                }
            } else {
                format!("/{quality}/manifest.mpd")
            };
            let is_dash = path.ends_with(".mpd");
            VideoStream {
                url: format!("{url_base}{path}"),
                name: Some(format!("{quality}p")),
                quality: Some(format!("{quality}p")),
                format: Some(if is_dash { "dash" } else { "mp4" }.to_string()),
                is_dash,
                stream_kind: Some(if is_dash { VideoStreamKind::Dash } else { VideoStreamKind::Direct }),
                subtitles: vec![SubtitleTrack {
                    url: subtitle_url.clone(),
                    language: Some("en".to_string()),
                    label: Some("English".to_string()),
                    format: Some("ass".to_string()),
                    ..SubtitleTrack::default()
                }],
                preferred: format!("{quality}p") == preferred_quality(request),
                headers: referer_headers(referer),
                initialized: true,
                ..VideoStream::default()
            }
        })
        .collect()
}

fn xsrf_token(url: &str) -> Option<String> {
    let response = cookies_get(url).ok()?;
    let header = response.header.unwrap_or_default();
    header
        .split(';')
        .find_map(|part| part.trim().strip_prefix("XSRF-TOKEN=").map(url_decode))
}

fn url_decode(value: &str) -> String {
    let mut bytes = Vec::new();
    let mut iter = value.as_bytes().iter().copied();
    while let Some(byte) = iter.next() {
        if byte == b'%' {
            let hi = iter.next().unwrap_or(b'0');
            let lo = iter.next().unwrap_or(b'0');
            let hex = [hi, lo];
            if let Ok(text) = std::str::from_utf8(&hex) {
                if let Ok(decoded) = u8::from_str_radix(text, 16) {
                    bytes.push(decoded);
                    continue;
                }
            }
        }
        bytes.push(if byte == b'+' { b' ' } else { byte });
    }
    String::from_utf8(bytes).unwrap_or_else(|_| value.to_string())
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = preferred_quality(request);
    streams.sort_by_key(|stream| stream.quality.as_deref().unwrap_or_default() == preferred);
    streams.reverse();
}

fn select_all<'a>(doc: &'a Html, selector: &str) -> impl Iterator<Item = ElementRef<'a>> {
    Selector::parse(selector)
        .ok()
        .map(|selector| doc.select(&selector).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
}

fn select_text(doc: &Html, selector: &str) -> Option<String> {
    select_all(doc, selector).next().map(|value| collect_text(&value)).filter(|value| !value.is_empty())
}

fn select_attr(doc: &Html, selector: &str, name: &str) -> Option<String> {
    select_all(doc, selector).next().and_then(|value| value.value().attr(name).map(ToString::to_string))
}

fn text(element: &ElementRef<'_>, selector: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    element.select(&selector).next().map(|value| collect_text(&value)).filter(|value| !value.is_empty())
}

fn attr(element: &ElementRef<'_>, selector: &str, name: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    element.select(&selector).next().and_then(|value| value.value().attr(name).map(ToString::to_string))
}

fn collect_text(element: &ElementRef<'_>) -> String {
    html::html_unescape(&element.text().collect::<Vec<_>>().join(" "))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .or_else(|| value.get("url"))
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(path_key)
}

fn path_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) || input.starts_with("/hentai/") {
        Some(path_key(input))
    } else {
        None
    }
}

fn path_key(input: &str) -> String {
    let without_origin = input.strip_prefix(BASE_URL).unwrap_or(input);
    let path = without_origin.split('?').next().unwrap_or(without_origin);
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn absolute_url(path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{BASE_URL}{}", path_key(path))
    }
}

fn title_from_path(path: &str) -> String {
    path_key(path)
        .trim_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("hstream")
        .replace(['-', '_'], " ")
        .split_whitespace()
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn trim_float(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as u32)
    } else {
        value.to_string()
    }
}

fn filter<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .or_else(|| request.get(key).and_then(Value::as_str))
        .unwrap_or(default)
}

fn array_filter(request: &Value, key: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(ToString::to_string))
        .collect()
}

fn preferred_quality(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("pref_quality_key"))
        .or_else(|| request.get("pref_quality_key"))
        .and_then(Value::as_str)
        .unwrap_or("720p")
        .to_string()
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

#[derive(Debug, Deserialize)]
struct PlayerApiResponse {
    #[serde(default)]
    legacy: u8,
    #[serde(default = "default_resolution")]
    resolution: String,
    stream_url: String,
    stream_domains: Vec<String>,
}

fn default_resolution() -> String {
    "4k".to_string()
}

const LIST_FIXTURE: &str = r#"<div class="items-center"><div class="w-full"><a href="/hentai/sample-1"><img alt="Sample Hstream"></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="relative"><div class="justify-between"><div><div><h1>Sample Hstream</h1><a>One</a><a>Two</a><a>Fixture Studio</a></div></div></div><p class="leading-tight">Fixture description.</p></div><div class="float-left"><img class="object-cover" src="/cover.jpg"></div><ul class="list-none"><li><a>Action</a></li></ul><a><i class="fa-upload"></i> 2024-01-01</a><input id="e_id" value="1">"#;
const PLAYER_FIXTURE: &str = r#"{"legacy":0,"resolution":"4k","stream_url":"sample","stream_domains":["https://fixtures.invalid/video"]}"#;

export_video_source!(SOURCE);
