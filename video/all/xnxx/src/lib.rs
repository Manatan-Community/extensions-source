use manatan_extension::{
    abi::ExtensionResult, export_video_source, source::VideoSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode, VideoStream,
    VideoStreamKind,
};
use manatan_shared::{
    html,
    sdk::{http::HttpClient, Context, SearchRequest},
    url,
};
use serde_json::Value;

const SOURCE: Xnxx = Xnxx;
const BASE_URL: &str = "https://www.xnxx.com";

struct Xnxx;

impl VideoSource for Xnxx {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = client()
            .get(format!(
                "{BASE_URL}/best/2026-06/{}",
                page(&request).saturating_sub(1)
            ))
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged {
                entries: vec![details_for(&path)],
                has_next_page: false,
            });
        }
        let tag = filter_str(&request, "tag", "");
        let page = page(&request).saturating_sub(1);
        let target = if !query.is_empty() {
            format!("{BASE_URL}/search/hits/{}/{page}", url::query_escape(query))
        } else if !tag.is_empty() {
            format!("{BASE_URL}/search/hits/{}/{page}", url::query_escape(tag))
        } else {
            format!("{BASE_URL}/best/2026-06/{page}")
        };
        let body = client()
            .get(target)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        Ok(details_for(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        let url = item_url(&key);
        Ok(vec![VideoEpisode {
            key: url.clone(),
            title: Some("Video".to_string()),
            episode_number: Some(1.0),
            date_uploaded: Some(1_780_272_000),
            url: Some(url),
            language: Some("all".to_string()),
            ..VideoEpisode::default()
        }])
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let target = request_key(&request, "episode").unwrap_or_default();
        let body = client()
            .get(item_url(&target))
            .browser_document()
            .send_text()
            .unwrap_or_default();
        let mut streams = parse_streams(&body);
        prefer_quality(&mut streams, pref_str(&request, "preferred_quality", "HLS"));
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Best".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: page.entries,
            has_more: page.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| item_url(&key)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| item_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_for(&path)),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("thumb-block")
        .skip(1)
        .filter_map(parse_card)
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("class=\"next\"") || body.contains("class='next'"),
    }
}

fn parse_card(block: &str) -> Option<CatalogItem> {
    let href = html::attr_after(block, "thumb", "href").or_else(|| html::attr(block, "href"))?;
    let key = path_from_url(&href).unwrap_or(href);
    let title = html::text_between(block, "<p", "</p>")
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .or_else(|| html::attr_after(block, "<img", "alt"))
        .unwrap_or_else(|| key.replace('/', " "));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(block, "<img", "data-src")
            .or_else(|| html::attr_after(block, "<img", "src")),
        url: Some(item_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn details_for(key: &str) -> CatalogItem {
    let key = path_from_url(key).unwrap_or_else(|| key.to_string());
    let body = client()
        .get(item_url(&key))
        .browser_document()
        .send_text()
        .unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(&body, "<strong", "</strong>")
            .map(|text| html::strip_tags(&text))
            .filter(|text| !text.is_empty())
            .or_else(|| {
                html::text_between(&body, "<title", "</title>").map(|text| html::strip_tags(&text))
            })
            .unwrap_or_else(|| key.replace('/', " ")),
        cover: html::attr_after(&body, "<meta property=\"og:image\"", "content"),
        url: Some(item_url(&key)),
        authors: html::text_between(&body, "clear-infobar", "</div>")
            .map(|text| vec![html::strip_tags(&text)])
            .unwrap_or_default(),
        description: html::text_between(&body, "#video-content-metadata", "</p>")
            .map(|text| html::strip_tags(&text))
            .filter(|text| !text.is_empty()),
        tags: parse_tags(&body),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_streams(body: &str) -> Vec<VideoStream> {
    [
        ("Low", "VideoUrlLow('", VideoStreamKind::Direct, false),
        ("HLS", "setVideoHLS('", VideoStreamKind::Hls, true),
        ("High", "VideoUrlHigh('", VideoStreamKind::Direct, false),
    ]
    .into_iter()
    .filter_map(|(quality, needle, kind, hls)| {
        let stream_url = extract_js_arg(body, needle)?;
        Some(VideoStream {
            url: stream_url.clone(),
            name: Some(quality.to_string()),
            quality: Some(quality.to_string()),
            format: Some(if hls { "hls" } else { "mp4" }.to_string()),
            is_hls: hls,
            stream_kind: Some(kind),
            headers: referer_headers(BASE_URL),
            initialized: true,
            ..VideoStream::default()
        })
    })
    .collect()
}

fn parse_tags(body: &str) -> Vec<String> {
    body.split("video-tags")
        .nth(1)
        .unwrap_or_default()
        .split("</div>")
        .next()
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .map(html::strip_tags)
        .filter(|tag| !tag.is_empty())
        .collect()
}

fn extract_js_arg(body: &str, needle: &str) -> Option<String> {
    let raw = body.split(needle).nth(1)?.split("')").next()?;
    let decoded = raw
        .replace("\\/", "/")
        .replace("\\u0026", "&")
        .replace("\\x26", "&");
    (!decoded.is_empty()).then_some(decoded)
}

fn prefer_quality(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by_key(|stream| stream.quality.as_deref() != Some(preferred));
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn item_url(key: &str) -> String {
    let path = path_from_url(key).unwrap_or_else(|| key.to_string());
    url::join_url(BASE_URL, &path)
}

fn path_from_url(input: &str) -> Option<String> {
    if input.contains("/video-") || input.starts_with("/video-") {
        let path = if let Some(index) = input.find("/video-") {
            &input[index..]
        } else {
            input
        };
        Some(path.trim_end_matches('/').to_string())
    } else {
        None
    }
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
        .map(ToString::to_string)
        .or_else(|| {
            request
                .get("key")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

fn pref_str<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .or_else(|| request.get("prefs"))
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

fn filter_str<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

export_video_source!(SOURCE);
