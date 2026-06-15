use base64::{Engine, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

const SOURCE: JavGuru = JavGuru;
const BASE_URL: &str = "https://jav.guru";

struct JavGuru;

impl VideoSource for JavGuru {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "popular" {
            let body = client()
                .get(format!("{BASE_URL}/most-watched-rank/"))
                .browser_document()
                .send_text()
                .unwrap_or_default();
            return Ok(parse_rank(&body, page));
        }
        let target = if page > 1 {
            format!("{BASE_URL}/page/{page}/")
        } else {
            format!("{BASE_URL}/")
        };
        let body = client()
            .get(target)
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
        if let Some(slug) = slug_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&slug)],
                has_next_page: false,
            });
        }
        if let Some(id) = query.strip_prefix("id:").filter(|id| id.parse::<u64>().is_ok()) {
            return Ok(Paged {
                entries: vec![fetch_details(id)],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(with_listing(&request, "latest"));
        }
        let mut target = format!("{BASE_URL}/?s={}", url::query_escape(query));
        if page(&request) > 1 {
            target = format!("{BASE_URL}/page/{}/?s={}", page(&request), url::query_escape(query));
        }
        let body = client()
            .get(target)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        Ok(fetch_details(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = normalize_key(&request_key(&request, "item").unwrap_or_default());
        let body = client()
            .get(item_url(&key))
            .browser_document()
            .send_text()
            .unwrap_or_default();
        let date_text = body
            .split("thedate")
            .nth(1)
            .and_then(|chunk| html::text_between(chunk, ">", "</span>"))
            .map(|text| html::strip_tags(&text).replace("Posted:", "").trim().to_string());
        Ok(vec![VideoEpisode {
            key: key.clone(),
            title: Some("Episode".to_string()),
            description: date_text,
            episode_number: Some(1.0),
            url: Some(item_url(&key)),
            language: Some("all".to_string()),
            ..VideoEpisode::default()
        }])
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let key = request_key(&request, "episode").unwrap_or_default();
        let target = item_url(&key);
        let body = client()
            .get(&target)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_iframe_hosters(&body, &target))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = request_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = key.splitn(2, '|');
        let name = parts.next().unwrap_or("Hoster");
        let iframe = parts.next().unwrap_or_default();
        let target = resolve_hoster_url(iframe).unwrap_or_else(|| iframe.to_string());
        Ok(vec![external_stream(&target, name, iframe, &request)])
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let hosters = self.hosters(request.clone())?;
        let mut streams = Vec::new();
        for hoster in hosters {
            let mut resolved = self.resolve_hoster(json!({
                "hoster": { "key": hoster.key },
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null),
            }))?;
            for stream in &mut resolved {
                stream.hoster = Some(hoster.clone());
            }
            streams.extend(resolved);
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Most Watched Rank".to_string(),
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
        Ok(request_key(&request, "item").map(|key| item_url(&key)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| item_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = slug_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&slug)),
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
        .with_header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .with_header("Accept-Language", "en-GB,en-US;q=0.9,en;q=0.8")
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn parse_rank(body: &str, page: u64) -> Paged<CatalogItem> {
    let all = body
        .split("rank-item")
        .skip(1)
        .filter_map(parse_rank_item)
        .collect::<Vec<_>>();
    let start = ((page.saturating_sub(1)) * 20) as usize;
    let end = usize::min(start + 20, all.len());
    Paged {
        entries: all.get(start..end).unwrap_or(&[]).to_vec(),
        has_next_page: end < all.len(),
    }
}

fn parse_rank_item(block: &str) -> Option<CatalogItem> {
    let href = html::attr_after(block, "rank-title", "href")
        .or_else(|| html::attr_after(block, "<a", "href"))?;
    let key = slug_from_url(&href)?;
    let title = html::text_between(block, "rank-title", "</a>")
        .map(|text| html::strip_tags(&text))
        .filter(|value| !value.is_empty())
        .or_else(|| html::attr_after(block, "<img", "alt"))
        .unwrap_or_else(|| key.replace('-', " "));
    Some(CatalogItem {
        key: normalize_key(&key),
        title,
        cover: html::attr_after(block, "<img", "src"),
        url: Some(item_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("inside-article")
        .skip(1)
        .filter(|block| !block.contains("nothing"))
        .filter_map(parse_listing_item)
        .collect();
    let current = page_number(body).unwrap_or(1);
    let last = body
        .split("wp-pagenavi")
        .last()
        .and_then(page_number)
        .unwrap_or(current);
    Paged {
        entries,
        has_next_page: current < last,
    }
}

fn parse_listing_item(block: &str) -> Option<CatalogItem> {
    let href = html::attr_after(block, "<a", "href")?;
    let key = slug_from_url(&href)?;
    let title = html::text_between(block, "<h2", "</h2>")
        .map(|text| html::strip_tags(&text))
        .filter(|value| !value.is_empty())
        .or_else(|| html::attr_after(block, "<img", "alt"))
        .unwrap_or_else(|| key.replace('-', " "));
    Some(CatalogItem {
        key: normalize_key(&key),
        title,
        cover: html::attr_after(block, "<img", "src"),
        url: Some(item_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn fetch_details(key: &str) -> CatalogItem {
    let key = normalize_key(key);
    let body = client()
        .get(item_url(&key))
        .browser_document()
        .send_text()
        .unwrap_or_default();
    let title = html::text_between(&body, "class=\"titl", "</")
        .map(|text| html::strip_tags(&text))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| key.replace('-', " "));
    let info = body.split("infoleft").nth(1).unwrap_or(&body);
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(&body, "large-screenshot", "src")
            .or_else(|| html::attr_after(&body, "<img", "src")),
        description: Some(description(info)),
        tags: links_after(info, "tag"),
        authors: links_after(info, "studio"),
        artists: links_after(info, "label"),
        url: Some(item_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_iframe_hosters(body: &str, page_url: &str) -> Vec<VideoHoster> {
    let mut out = Vec::new();
    for encoded in body.split("\"iframe_url\":\"").skip(1) {
        let encoded = encoded.split('"').next().unwrap_or_default();
        let Ok(bytes) = STANDARD.decode(encoded) else {
            continue;
        };
        let Ok(iframe) = String::from_utf8(bytes) else {
            continue;
        };
        let name = host_name(&iframe);
        out.push(VideoHoster {
            key: format!("{name}|{iframe}"),
            name,
            url: Some(page_url.to_string()),
            lazy: true,
            video_count: Some(1),
            ..VideoHoster::default()
        });
    }
    out
}

fn resolve_hoster_url(iframe_url: &str) -> Option<String> {
    if iframe_url.is_empty() {
        return None;
    }
    if let Some(token) = query_param(iframe_url, "xd") {
        let base = iframe_url.split('?').next().unwrap_or(iframe_url);
        let final_url = format!("{base}?xr={}", token.chars().rev().collect::<String>());
        return redirect_location(&final_url, iframe_url);
    }
    let body = client()
        .get(iframe_url)
        .browser_document()
        .referer(BASE_URL)
        .send_text()
        .ok()?;
    let script = body.split("cfg").nth(1)?;
    let cid = between_any(script, "cid:", &[",", "\n"])?;
    let base = between_any(script, "base:", &[",", "\n"])?;
    let rtype = between_any(script, "rtype:", &[",", "\n"]).unwrap_or_else(|| "x".to_string());
    let keys = script
        .split("keys:")
        .nth(1)?
        .split(']')
        .next()?
        .split(',')
        .map(|key| key.trim().trim_matches('[').trim_matches('"').trim_matches('\''))
        .filter(|key| !key.is_empty())
        .collect::<Vec<_>>();
    let element = body.split(&format!("id=\"{cid}\"")).nth(1)?;
    let mut token = String::new();
    for key in keys {
        token.push_str(&html::attr(element, key).unwrap_or_default());
    }
    if token.is_empty() {
        return None;
    }
    let base = resolve_url(iframe_url, &base);
    let final_url = format!("{base}?{}r={}", rtype.trim_matches('"').trim_matches('\''), token.chars().rev().collect::<String>());
    redirect_location(&final_url, iframe_url)
}

fn redirect_location(target: &str, referer: &str) -> Option<String> {
    let response = client()
        .get(target)
        .referer(referer)
        .send()
        .ok()?;
    response
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("location"))
        .map(|(_, value)| value.to_string())
        .or_else(|| {
            if response.final_url != target {
                Some(response.final_url)
            } else {
                None
            }
        })
}

fn external_stream(target: &str, name: &str, referer: &str, request: &Value) -> VideoStream {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    VideoStream {
        url: target.to_string(),
        name: Some(name.to_string()),
        quality: Some(name.to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        preferred: pref(request, "preferred_quality").is_none(),
        headers,
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = pref(request, "preferred_quality").unwrap_or_else(|| "1080".to_string());
    streams.sort_by_key(|stream| {
        if stream
            .quality
            .as_deref()
            .unwrap_or_default()
            .contains(&preferred)
        {
            0
        } else {
            1
        }
    });
}

fn links_after(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .take_while(|chunk| !chunk.contains("<li"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .collect()
}

fn description(info: &str) -> String {
    ["code", "director", "studio", "label", "actor", "actress", "release date"]
        .iter()
        .filter_map(|marker| {
            info.split(marker)
                .nth(1)
                .and_then(|chunk| html::text_between(chunk, ">", "</li>"))
                .map(|text| html::strip_tags(&text))
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn between_any(input: &str, marker: &str, ends: &[&str]) -> Option<String> {
    let value = input.split(marker).nth(1)?;
    let end = ends
        .iter()
        .filter_map(|end| value.find(end))
        .min()
        .unwrap_or(value.len());
    Some(
        value[..end]
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string(),
    )
}

fn query_param(input: &str, name: &str) -> Option<String> {
    input
        .split('?')
        .nth(1)?
        .split('&')
        .find_map(|part| {
            let (key, value) = part.split_once('=')?;
            (key == name).then(|| value.to_string())
        })
}

fn resolve_url(base: &str, path: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else {
        let origin = base
            .split("//")
            .nth(1)
            .and_then(|tail| tail.split('/').next())
            .map(|host| format!("https://{host}"))
            .unwrap_or_else(|| BASE_URL.to_string());
        url::join_url(&origin, path)
    }
}

fn host_name(input: &str) -> String {
    input
        .split("//")
        .nth(1)
        .and_then(|tail| tail.split('/').next())
        .unwrap_or("Hoster")
        .replace("www.", "")
}

fn page_number(input: &str) -> Option<u64> {
    input.split("/page/").nth(1)?.split('/').next()?.parse().ok()
}

fn slug_from_url(input: &str) -> Option<String> {
    if input.trim().is_empty() {
        return None;
    }
    let clean = input.split('?').next().unwrap_or(input).trim_end_matches('/');
    if clean.starts_with("http") && !clean.contains("jav.guru") {
        return None;
    }
    clean
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty() && *value != "jav.guru")
        .map(ToString::to_string)
}

fn normalize_key(key: &str) -> String {
    slug_from_url(key).unwrap_or_else(|| key.trim_matches('/').to_string())
}

fn item_url(key: &str) -> String {
    if key.starts_with("http") {
        key.to_string()
    } else {
        format!("{BASE_URL}/{}/", key.trim_matches('/'))
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
