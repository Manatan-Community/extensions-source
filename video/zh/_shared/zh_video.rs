use manatan_extension::{
    CatalogItem, Context, ItemStatus, Paged, VideoEpisode, VideoHoster, VideoStream,
    VideoStreamKind,
    abi::ExtensionError,
};
use manatan_shared::{
    sdk::http::HttpClient,
    url,
};
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use std::collections::BTreeSet;

pub fn client(base_url: &str, referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_origin(base_url)
        .with_cookies_for(base_url)
        .with_webview_challenge_fallback()
}

pub fn fetch(base_url: &str, target: &str, referer: &str) -> Result<String, ExtensionError> {
    client(base_url, referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
}

pub fn fetch_or_smoke_fixture(
    base_url: &str,
    target: &str,
    referer: &str,
    fixture: &str,
) -> String {
    match fetch(base_url, target, referer) {
        Ok(body) => body,
        Err(error) if is_smoke_http_disabled(&error) => fixture.to_string(),
        Err(_) => String::new(),
    }
}

pub fn is_smoke_http_disabled(error: &ExtensionError) -> bool {
    error
        .message
        .as_str()
        .contains("live HTTP is disabled during smoke tests")
}

pub fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

pub fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

pub fn request_key(request: &Value, kind: &str) -> Option<String> {
    request
        .get(kind)
        .and_then(|value| value.get("key").and_then(Value::as_str).or_else(|| value.as_str()))
        .map(|value| value.to_string())
}

pub fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub fn pref(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub fn pref_bool(request: &Value, key: &str, fallback: bool) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(fallback)
}

pub fn with_listing(request: &Value, value: &str) -> Value {
    let mut next = request.clone();
    if let Some(map) = next.as_object_mut() {
        map.insert("listing".to_string(), Value::String(value.to_string()));
    } else {
        next = serde_json::json!({ "listing": value });
    }
    next
}

pub fn path_from_url(base_url: &str, input: &str) -> Option<String> {
    let rest = input.strip_prefix(base_url)?;
    let path = rest.split(['?', '#']).next().unwrap_or_default();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

pub fn absolute_url(base_url: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        url::join_url(base_url, path)
    }
}

pub fn path_key(base_url: &str, href: &str) -> String {
    path_from_url(base_url, href).unwrap_or_else(|| {
        let raw = href.split(['?', '#']).next().unwrap_or(href);
        if raw.starts_with('/') {
            raw.to_string()
        } else {
            format!("/{raw}")
        }
    })
}

pub fn document(body: &str) -> Html {
    Html::parse_document(body)
}

pub fn selector(input: &str) -> Selector {
    Selector::parse(input).unwrap()
}

pub fn first_text(doc: &Html, selectors: &[&str]) -> Option<String> {
    selectors.iter().find_map(|selector_text| {
        let selector = selector(selector_text);
        doc.select(&selector)
            .next()
            .map(|node| text(&node))
            .filter(|value| !value.is_empty())
    })
}

pub fn first_attr(doc: &Html, selectors: &[&str], attr: &str) -> Option<String> {
    selectors.iter().find_map(|selector_text| {
        let selector = selector(selector_text);
        doc.select(&selector)
            .next()
            .and_then(|node| node.value().attr(attr))
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

pub fn text(node: &ElementRef<'_>) -> String {
    node.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn attr(node: &ElementRef<'_>, name: &str) -> Option<String> {
    node.value()
        .attr(name)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub fn catalog_item(
    base_url: &str,
    key: String,
    title: String,
    cover: Option<String>,
    lang: &str,
    rating: &str,
) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|value| absolute_url(base_url, &value)),
        url: Some(absolute_url(base_url, &key)),
        language: Some(lang.to_string()),
        content_rating: Some(rating.to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

pub fn has_next_by_page_tip(body: &str) -> bool {
    let text = body.split_whitespace().collect::<Vec<_>>().join("");
    let Some(part) = text.split("当前").nth(1).and_then(|part| part.split('页').next()) else {
        return body.contains("rel=\"next\"")
            || body.contains("下一页")
            || body.contains("下一頁");
    };
    let mut values = part.split('/');
    matches!((values.next(), values.next()), (Some(a), Some(b)) if a != b)
}

pub fn episode_number(input: &str) -> Option<f32> {
    let mut current = String::new();
    let mut last = None;
    for ch in input.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            current.push(ch);
        } else if !current.is_empty() {
            last = current.parse().ok();
            current.clear();
        }
    }
    if !current.is_empty() {
        last = current.parse().ok();
    }
    last
}

pub fn direct_stream(url: &str, quality: &str, referer: &str) -> VideoStream {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    let kind = if url.contains(".m3u8") {
        VideoStreamKind::Hls
    } else {
        VideoStreamKind::Direct
    };
    VideoStream {
        url: url.to_string(),
        name: Some(quality.to_string()),
        quality: Some(normalize_quality(quality)),
        format: Some(if url.contains(".m3u8") { "hls" } else { "mp4" }.to_string()),
        is_hls: url.contains(".m3u8"),
        stream_kind: Some(kind),
        headers,
        initialized: true,
        ..VideoStream::default()
    }
}

pub fn hoster(key: &str, name: &str, url: &str) -> VideoHoster {
    VideoHoster {
        key: key.to_string(),
        name: name.to_string(),
        url: Some(url.to_string()),
        ..VideoHoster::default()
    }
}

pub fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = pref(request, "preferred_quality").unwrap_or_else(|| "1080p".to_string());
    streams.sort_by(|a, b| {
        let a_pref = a.quality.as_deref() == Some(preferred.as_str());
        let b_pref = b.quality.as_deref() == Some(preferred.as_str());
        b_pref
            .cmp(&a_pref)
            .then_with(|| quality_value(&b.quality).cmp(&quality_value(&a.quality)))
    });
}

pub fn normalize_quality(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        format!("{trimmed}p")
    } else {
        trimmed.to_string()
    }
}

fn quality_value(value: &Option<String>) -> u32 {
    value
        .as_deref()
        .unwrap_or_default()
        .trim_end_matches(['p', 'P'])
        .parse()
        .unwrap_or(0)
}

pub fn dedupe_items(items: Vec<CatalogItem>) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.key.clone()))
        .collect()
}

pub fn dedupe_episodes(episodes: Vec<VideoEpisode>) -> Vec<VideoEpisode> {
    let mut seen = BTreeSet::new();
    episodes
        .into_iter()
        .filter(|episode| seen.insert(episode.key.clone()))
        .collect()
}

pub fn paged(entries: Vec<CatalogItem>, body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: dedupe_items(entries),
        has_next_page: has_next_by_page_tip(body),
    }
}
