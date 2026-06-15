use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: Jable = Jable;
const BASE_URL: &str = "https://jable.tv";

struct Jable;

impl VideoSource for Jable {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let path = if listing == "latest" {
            "latest-updates"
        } else {
            "hot"
        };
        let block_id = if listing == "latest" {
            "list_videos_latest_videos_list"
        } else {
            "list_videos_common_videos_list"
        };
        let sort = if listing == "latest" {
            "post_date".to_string()
        } else {
            pref(&request, "sort_by").unwrap_or_else(|| "video_viewed_week".to_string())
        };
        let body = client()
            .get(search_url(path, page, &request, Some(block_id), Some(&sort), None))
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_items(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(slug) = slug_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&slug, &request)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let path = if query.is_empty() {
            "hot".to_string()
        } else {
            format!("search/{}", url::query_escape(query))
        };
        let body = client()
            .get(search_url(
                &path,
                page,
                &request,
                Some("list_videos_videos_list_search_result"),
                None,
                if query.is_empty() { None } else { Some(query) },
            ))
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_items(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        Ok(fetch_details(&key, &request))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        Ok(vec![VideoEpisode {
            key: key.clone(),
            title: Some("Episode".to_string()),
            episode_number: Some(1.0),
            url: Some(item_url(&key, &request)),
            language: Some(pref(&request, "language").unwrap_or_else(|| "en".to_string())),
            ..VideoEpisode::default()
        }])
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode").unwrap_or_default();
        let target = item_url(&key, &request);
        let body = client()
            .get(&target)
            .browser_document()
            .referer(BASE_URL)
            .send_text()
            .unwrap_or_default();
        let Some(video_url) = body
            .split("var hlsUrl = '")
            .nth(1)
            .and_then(|tail| tail.split('\'').next())
            .filter(|value| !value.is_empty())
        else {
            return Ok(Vec::new());
        };
        let mut headers = Context::new();
        headers.insert("Referer".to_string(), target);
        Ok(vec![VideoStream {
            url: video_url.to_string(),
            name: Some("Default".to_string()),
            quality: Some("auto".to_string()),
            format: Some("hls".to_string()),
            is_hls: true,
            stream_kind: Some(VideoStreamKind::Hls),
            headers,
            preferred: true,
            initialized: true,
            ..VideoStream::default()
        }])
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Hot".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest Updates".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| item_url(&key, &request)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| item_url(&key, &request)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = slug_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&slug, &request)),
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

fn search_url(
    path: &str,
    page: u64,
    request: &Value,
    block_id: Option<&str>,
    sort: Option<&str>,
    query: Option<&str>,
) -> String {
    let lang = match pref(request, "language").as_deref() {
        Some("ja") => "jp",
        Some("zh") => "zh",
        _ => "en",
    };
    let mut out = format!(
        "{BASE_URL}/{}/?lang={lang}&from={:02}&_={}",
        path.trim_matches('/'),
        page,
        1_700_000_000_000_u64 + page
    );
    if let Some(block_id) = block_id {
        out.push_str("&function=get_block&block_id=");
        out.push_str(block_id);
    }
    if let Some(sort) = sort.filter(|value| !value.is_empty()) {
        out.push_str("&sort_by=");
        out.push_str(sort);
    }
    if let Some(query) = query.filter(|value| !value.is_empty()) {
        out.push_str("&q=");
        out.push_str(&url::query_escape(query));
    }
    out
}

fn parse_items(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("video-img-box")
        .skip(1)
        .filter_map(parse_item_block)
        .collect();
    Paged {
        entries,
        has_next_page: true,
    }
}

fn parse_item_block(block: &str) -> Option<CatalogItem> {
    let href = html::attr_after(block, "<a", "href")?;
    let key = slug_from_url(&href)?;
    let title = html::text_between(block, "<h6", "</h6>")
        .or_else(|| html::text_between(block, "class=\"title", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| html::attr_after(block, "<img", "alt"))
        .unwrap_or_else(|| key.replace('-', " "));
    let cover = html::attr_after(block, "<img", "data-src")
        .or_else(|| html::attr_after(block, "<img", "src"));
    Some(CatalogItem {
        key: format!("/videos/{key}/"),
        title,
        cover,
        url: Some(item_url(&format!("/videos/{key}/"), &Value::Null)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn fetch_details(key: &str, request: &Value) -> CatalogItem {
    let target = item_url(key, request);
    let body = client()
        .get(&target)
        .browser_document()
        .send_text()
        .unwrap_or_default();
    let title = html::text_between(&body, "class=\"header-left", "</div>")
        .and_then(|block| html::text_between(&block, "<h4", "</h4>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| key.trim_matches('/').replace(['-', '/'], " "));
    let artists = body
        .split("class=\"model")
        .skip(1)
        .filter_map(|chunk| html::attr_after(chunk, "<span", "title"))
        .collect::<Vec<_>>();
    let tags = body
        .split("class=\"tags")
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect();
    let description = html::text_between(&body, "class=\"header-right", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    CatalogItem {
        key: normalize_key(key),
        title,
        cover: html::attr_after(&body, "<meta property=\"og:image\"", "content")
            .or_else(|| html::attr_after(&body, "<img", "data-src")),
        url: Some(target),
        description,
        artists,
        tags,
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn item_url(key: &str, request: &Value) -> String {
    let mut out = if key.starts_with("http") {
        key.to_string()
    } else {
        url::join_url(BASE_URL, key)
    };
    let lang = pref(request, "language").unwrap_or_else(|| "en".to_string());
    out.push_str(if out.contains('?') { "&lang=" } else { "?lang=" });
    out.push_str(if lang == "ja" { "jp" } else { &lang });
    out
}

fn slug_from_url(input: &str) -> Option<String> {
    if input.trim().is_empty() {
        return None;
    }
    let clean = input.split('?').next().unwrap_or(input).trim_end_matches('/');
    if clean.starts_with("http") && !clean.contains("jable.tv") {
        return None;
    }
    clean
        .split("/videos/")
        .nth(1)
        .or_else(|| clean.rsplit('/').next())
        .filter(|value| !value.is_empty() && *value != "jable.tv")
        .map(ToString::to_string)
}

fn normalize_key(key: &str) -> String {
    if key.starts_with("http") {
        format!("/videos/{}/", slug_from_url(key).unwrap_or_default())
    } else if key.starts_with('/') {
        key.split('?').next().unwrap_or(key).to_string()
    } else {
        format!("/videos/{}/", key.trim_matches('/'))
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
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn pref(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    next["listing"] = Value::String(listing.to_string());
    next
}

export_video_source!(SOURCE);
