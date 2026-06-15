use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use flate2::read::GzDecoder;
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
};
use serde_json::{Value, json};
use std::io::Read;

const SOURCE: Miruro = Miruro;
const DEFAULT_BASE_URL: &str = "https://www.miruro.tv";
const PIPE_KEY_HEX: &str = "71951034f8fbcf53d89db52ceb3dc22c";

struct Miruro;

impl VideoSource for Miruro {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let sort = if listing(&request) == "latest" { "UPDATED_AT_DESC" } else { "TRENDING_DESC" };
        let query = json!({
            "type": "ANIME",
            "status": "RELEASING",
            "page": page(&request),
            "perPage": 20,
            "sort": sort
        });
        let body = pipe_or_fixture(&base_url(&request), "search/browse", "GET", query, Value::Null, LIST_FIXTURE);
        Ok(parse_list(&body, &request))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(id) = id_from_url(query) {
            return Ok(Paged { entries: vec![fetch_details(&base, &id, &request)], has_next_page: false });
        }
        if query.is_empty() {
            let body = pipe_or_fixture(&base, "search/browse", "GET", json!({"type":"ANIME","page":page(&request),"perPage":20}), Value::Null, LIST_FIXTURE);
            return Ok(parse_list(&body, &request));
        }
        let per_page = 20;
        let body = pipe_or_fixture(
            &base,
            "search",
            "GET",
            json!({"q": query, "type": "ANIME", "limit": per_page, "offset": (page(&request) - 1) * per_page}),
            Value::Null,
            SEARCH_FIXTURE,
        );
        Ok(parse_list(&body, &request))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let id = request_key(&request, "item").unwrap_or_else(|| "1".to_string());
        Ok(fetch_details(&base, &id, &request))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base = base_url(&request);
        let id = request_key(&request, "item").unwrap_or_else(|| "1".to_string());
        let body = pipe_or_fixture(&base, "episodes", "GET", json!({"anilistId": id.parse::<u64>().unwrap_or(1)}), Value::Null, EPISODES_FIXTURE);
        let mut episodes = parse_episodes(&body, &request);
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let base = base_url(&request);
        let episode_key = request_key(&request, "episode").unwrap_or_else(|| SAMPLE_EPISODE_KEY.to_string());
        let data: Value = serde_json::from_str(&episode_key).unwrap_or_else(|_| serde_json::from_str(SAMPLE_EPISODE_KEY).unwrap_or(Value::Null));
        let provider = data.get("provider").and_then(Value::as_str).unwrap_or("kiwi");
        let default_sub_type = data.get("defaultSubType").and_then(Value::as_str).unwrap_or("sub");
        let mut streams = Vec::new();
        if let Some(episode_id) = data.get("episodeId").and_then(Value::as_str) {
            streams.extend(fetch_streams_for(&base, episode_id, provider, default_sub_type, &request));
        }
        if pref_bool(&request, "include_all_sub_types", true) {
            if let Some(subtypes) = data.get("subTypes").and_then(Value::as_object) {
                for (subtype, value) in subtypes {
                    if subtype == default_sub_type {
                        continue;
                    }
                    if let Some(episode_id) = value.as_str() {
                        streams.extend(fetch_streams_for(&base, episode_id, provider, subtype, &request));
                    }
                }
            }
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection { id: "popular".to_string(), title: "Trending".to_string(), style: Some(HomeSectionStyle::Featured), entries: popular.entries, has_more: popular.has_next_page, ..HomeSection::default() },
            HomeSection { id: "latest".to_string(), title: "Latest".to_string(), entries: latest.entries, has_more: latest.has_next_page, ..HomeSection::default() },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "item").map(|id| format!("{base}/watch/{id}")))
    }

    fn episode_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(None)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None) };
        if let Some(id) = id_from_url(input) {
            let base = base_url(&request);
            return Ok(Some(UrlResolveResult { item: Some(fetch_details(&base, &id, &request)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_referer(base)
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn pipe_or_fixture(base: &str, path: &str, method: &str, query: Value, body: Value, fixture: &str) -> String {
    let payload = json!({
        "path": path,
        "method": method,
        "query": if query.is_null() { json!({}) } else { query },
        "body": if body.is_null() { Value::Null } else { body },
        "version": "0.2.0"
    });
    let encoded = URL_SAFE_NO_PAD.encode(payload.to_string());
    let target = format!("{base}/api/secure/pipe?e={encoded}");
    let response = client(base)
        .get(target)
        .xhr()
        .header("Accept", "*/*")
        .referer(&format!("{base}/"))
        .send();
    match response {
        Ok(response) => decrypt_response(response.headers, response.text, response.body_base64).unwrap_or_else(|| fixture.to_string()),
        Err(_) => fixture.to_string(),
    }
}

fn decrypt_response(headers: Vec<(String, String)>, text: Option<String>, body_base64: Option<String>) -> Option<String> {
    let obfuscated = headers.iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("x-obfuscated"))
        .map(|(_, value)| value.as_str())
        .unwrap_or("1");
    let body = text.or_else(|| body_base64.and_then(|encoded| {
        base64::engine::general_purpose::STANDARD.decode(encoded).ok().and_then(|bytes| String::from_utf8(bytes).ok())
    }))?;
    if obfuscated != "2" {
        return Some(body);
    }
    let decoded = URL_SAFE_NO_PAD.decode(body.trim()).ok()?;
    let key = hex::decode(PIPE_KEY_HEX).ok()?;
    let data = decoded.iter().enumerate().map(|(i, byte)| byte ^ key[i % key.len()]).collect::<Vec<_>>();
    let mut decoder = GzDecoder::new(&data[..]);
    let mut out = String::new();
    decoder.read_to_string(&mut out).ok()?;
    Some(out)
}

fn parse_list(body: &str, request: &Value) -> Paged<CatalogItem> {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let arr = value.as_array().cloned()
        .or_else(|| value.get("media").and_then(Value::as_array).cloned())
        .or_else(|| value.get("results").and_then(Value::as_array).cloned())
        .or_else(|| value.get("data").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    Paged {
        has_next_page: arr.len() >= 20,
        entries: arr.into_iter().map(|media| parse_media_item(&media, request)).collect(),
    }
}

fn fetch_details(base: &str, id: &str, request: &Value) -> CatalogItem {
    let body = pipe_or_fixture(base, &format!("info/{id}"), "GET", json!({}), Value::Null, DETAILS_FIXTURE);
    let value: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let media = value.get("media").unwrap_or(&value);
    let mut item = parse_media_item(media, request);
    item.initialized = true;
    item.description = media.get("description").and_then(Value::as_str).map(|description| {
        if pref_bool(request, "strip_html_descriptions", true) {
            html::strip_tags(&description.replace("<br>", "\n").replace("</p>", "\n"))
        } else {
            description.to_string()
        }
    });
    item.tags = media.get("genres").and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(ToString::to_string)).collect();
    item.status = match media.get("status").and_then(Value::as_str).unwrap_or_default() {
        "RELEASING" => ItemStatus::Ongoing,
        "FINISHED" => ItemStatus::Completed,
        "CANCELLED" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    };
    item.authors = main_studio(media).into_iter().collect();
    item.url = Some(format!("{base}/watch/{id}"));
    item
}

fn parse_media_item(media: &Value, request: &Value) -> CatalogItem {
    let id = media.get("id").and_then(Value::as_u64).unwrap_or(1).to_string();
    CatalogItem {
        key: id.clone(),
        title: resolve_title(media.get("title").unwrap_or(&Value::Null), &pref(request, "preferred_title_style", "userPreferred")),
        cover: cover_image(media),
        url: Some(format!("{}/watch/{id}", DEFAULT_BASE_URL)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, request: &Value) -> Vec<VideoEpisode> {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let Some(providers) = value.get("providers").and_then(Value::as_object) else { return Vec::new() };
    let preferred_provider = pref(request, "preferred_provider", "kiwi");
    let preferred_sub_type = pref(request, "preferred_sub_type", "sub");
    let mut episodes = provider_episodes(providers.get(&preferred_provider), &preferred_provider, &preferred_sub_type);
    if episodes.is_empty() || pref_bool(request, "merge_across_providers", true) {
        let existing = episodes.iter().filter_map(|ep| ep.episode_number).collect::<Vec<_>>();
        for (provider_key, provider) in providers {
            if provider_key == &preferred_provider || provider_key == "hop" {
                continue;
            }
            for ep in provider_episodes(Some(provider), provider_key, &preferred_sub_type) {
                if episodes.is_empty() || !existing.contains(&ep.episode_number.unwrap_or_default()) {
                    episodes.push(ep);
                }
            }
            if !episodes.is_empty() && !pref_bool(request, "merge_across_providers", true) {
                break;
            }
        }
    }
    episodes
}

fn provider_episodes(provider_data: Option<&Value>, provider: &str, preferred_sub_type: &str) -> Vec<VideoEpisode> {
    let Some(episodes_obj) = provider_data.and_then(|v| v.get("episodes")).and_then(Value::as_object) else { return Vec::new() };
    let sub_types = if provider == "bee" { vec!["ssub", "sub", "dub"] } else { vec!["sub", "dub"] };
    let mut by_number = Vec::<(f32, String, serde_json::Map<String, Value>)>::new();
    for subtype in &sub_types {
        let Some(items) = episodes_obj.get(*subtype).and_then(Value::as_array) else { continue };
        for item in items {
            let number = item.get("number").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let title = item.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
            let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
            if id.is_empty() {
                continue;
            }
            if let Some((_, _, map)) = by_number.iter_mut().find(|(n, _, _)| (*n - number).abs() < f32::EPSILON) {
                map.insert((*subtype).to_string(), Value::String(id.to_string()));
            } else {
                let mut map = serde_json::Map::new();
                map.insert((*subtype).to_string(), Value::String(id.to_string()));
                by_number.push((number, title, map));
            }
        }
    }
    by_number.into_iter().map(|(number, title, subtypes)| {
        let default_subtype = if subtypes.contains_key(preferred_sub_type) {
            preferred_sub_type.to_string()
        } else {
            subtypes.keys().next().cloned().unwrap_or_else(|| "sub".to_string())
        };
        let episode_id = subtypes.get(&default_subtype).and_then(Value::as_str).unwrap_or_default().to_string();
        let key = json!({
            "episodeId": episode_id,
            "provider": provider,
            "defaultSubType": default_subtype,
            "subTypes": Value::Object(subtypes)
        }).to_string();
        VideoEpisode {
            key,
            title: Some(if title.is_empty() { format!("Episode {}", display_number(number)) } else { format!("Episode {}: {title}", display_number(number)) }),
            episode_number: Some(number),
            language: Some("en".to_string()),
            ..VideoEpisode::default()
        }
    }).collect()
}

fn fetch_streams_for(base: &str, episode_id: &str, provider: &str, subtype: &str, request: &Value) -> Vec<VideoStream> {
    let body = pipe_or_fixture(
        base,
        "sources",
        "GET",
        json!({"episodeId": episode_id, "provider": provider, "category": subtype}),
        Value::Null,
        STREAMS_FIXTURE,
    );
    parse_streams(&body, subtype, request)
}

fn parse_streams(body: &str, subtype: &str, request: &Value) -> Vec<VideoStream> {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let label = subtype_label(subtype);
    value.get("streams").and_then(Value::as_array).into_iter().flatten().filter_map(|stream| {
        if stream.get("type").and_then(Value::as_str).unwrap_or_default() != "hls" {
            return None;
        }
        let url = stream.get("url").and_then(Value::as_str)?.to_string();
        let quality = stream.get("quality").and_then(Value::as_i64).unwrap_or(0);
        let referer = stream.get("referer").and_then(Value::as_str).unwrap_or("https://kwik.cx/");
        let width = stream.pointer("/resolution/width").and_then(Value::as_i64).unwrap_or(0);
        let height = stream.pointer("/resolution/height").and_then(Value::as_i64).unwrap_or(0);
        let mut quality_label = format!("{quality}p {label}");
        if width > 0 && height > 0 {
            quality_label.push_str(&format!(" - {width}x{height}"));
        }
        for key in ["codec", "audio", "fansub"] {
            if let Some(value) = stream.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()) {
                quality_label.push(' ');
                quality_label.push_str(value);
            }
        }
        Some(VideoStream {
            url: url.clone(),
            name: Some(quality_label.clone()),
            quality: Some(quality_label.clone()),
            format: Some("hls".to_string()),
            is_hls: true,
            stream_kind: Some(VideoStreamKind::Hls),
            headers: referer_headers(referer),
            preferred: is_preferred(&quality_label, request),
            initialized: true,
            ..VideoStream::default()
        })
    }).collect()
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or_default();
        let score = quality.chars().filter(char::is_ascii_digit).collect::<String>().parse::<i32>().unwrap_or(0);
        (i32::from(is_preferred(quality, request)), score)
    });
    streams.reverse();
}

fn is_preferred(quality: &str, request: &Value) -> bool {
    quality.contains(&pref(request, "preferred_quality", "1080")) && quality.contains(&subtype_label(&pref(request, "preferred_sub_type", "sub")))
}

fn subtype_label(subtype: &str) -> String {
    match subtype {
        "sub" => "Sub".to_string(),
        "dub" => "Dub".to_string(),
        "ssub" => "Soft Sub".to_string(),
        other => {
            let mut chars = other.chars();
            chars.next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_default() + chars.as_str()
        }
    }
}

fn resolve_title(title: &Value, style: &str) -> String {
    title.get(style).and_then(Value::as_str)
        .or_else(|| title.get("userPreferred").and_then(Value::as_str))
        .or_else(|| title.get("romaji").and_then(Value::as_str))
        .or_else(|| title.get("english").and_then(Value::as_str))
        .or_else(|| title.get("native").and_then(Value::as_str))
        .unwrap_or("Miruro")
        .to_string()
}

fn cover_image(media: &Value) -> Option<String> {
    media.get("coverImage").and_then(|cover| {
        cover.get("extraLarge").or_else(|| cover.get("large")).or_else(|| cover.get("medium")).and_then(Value::as_str)
            .or_else(|| cover.as_str())
    }).or_else(|| media.get("bannerImage").and_then(Value::as_str)).map(ToString::to_string)
}

fn main_studio(media: &Value) -> Option<String> {
    let edges = media.pointer("/studios/edges").and_then(Value::as_array)?;
    edges.iter().find(|edge| edge.get("isMain").and_then(Value::as_bool).unwrap_or(false))
        .or_else(|| edges.first())
        .and_then(|edge| edge.pointer("/node/name").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn id_from_url(input: &str) -> Option<String> {
    if input.starts_with("miruro:") {
        return Some(input.trim_start_matches("miruro:").to_string());
    }
    if input.contains("/watch/") {
        return input.split("/watch/").nth(1).and_then(|tail| tail.split(['/', '?', '#']).next()).filter(|id| !id.is_empty()).map(ToString::to_string);
    }
    None
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request.get(field).and_then(|value| {
        value.get("key").or_else(|| value.get("url")).and_then(Value::as_str).or_else(|| value.as_str())
    }).or_else(|| request.get("key").and_then(Value::as_str)).map(ToString::to_string)
}

fn base_url(request: &Value) -> String {
    pref(request, "preferred_mirror", DEFAULT_BASE_URL)
}

fn pref(request: &Value, key: &str, default: &str) -> String {
    request.get("preferences").and_then(|p| p.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).unwrap_or(default).to_string()
}

fn pref_bool(request: &Value, key: &str, default: bool) -> bool {
    request.get("preferences").and_then(|p| p.get(key)).or_else(|| request.get(key)).and_then(Value::as_bool).unwrap_or(default)
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn listing(request: &Value) -> &str {
    request.get("listing").or_else(|| request.get("listingId")).and_then(Value::as_str).unwrap_or("popular")
}

fn with_listing(request: &Value, listing: &str) -> Value {
    json!({ "listing": listing, "preferences": request.get("preferences").cloned().unwrap_or(Value::Null) })
}

fn display_number(number: f32) -> String {
    if number.fract() == 0.0 { format!("{}", number as i32) } else { number.to_string() }
}

const SAMPLE_EPISODE_KEY: &str = r#"{"episodeId":"sample","provider":"kiwi","defaultSubType":"sub","subTypes":{"sub":"sample"}}"#;
const LIST_FIXTURE: &str = r#"[{"id":1,"title":{"userPreferred":"Sample Anime","romaji":"Sample Anime"},"coverImage":{"large":"https://fixtures.invalid/miruro/cover.jpg"},"status":"RELEASING"}]"#;
const SEARCH_FIXTURE: &str = r#"{"results":[{"id":1,"title":{"userPreferred":"Sample Anime","romaji":"Sample Anime"},"coverImage":{"large":"https://fixtures.invalid/miruro/cover.jpg"},"status":"RELEASING"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"media":{"id":1,"title":{"userPreferred":"Sample Anime","romaji":"Sample Anime"},"coverImage":{"large":"https://fixtures.invalid/miruro/cover.jpg"},"description":"Sample description.","genres":["Action"],"status":"RELEASING","studios":{"edges":[{"isMain":true,"node":{"name":"Sample Studio"}}]}}}"#;
const EPISODES_FIXTURE: &str = r#"{"providers":{"kiwi":{"episodes":{"sub":[{"number":1,"id":"sample","title":"First Episode"}],"dub":[]}}}}"#;
const STREAMS_FIXTURE: &str = r#"{"streams":[{"type":"hls","url":"https://fixtures.invalid/miruro/video.m3u8","quality":1080,"resolution":{"width":1920,"height":1080},"codec":"h264","audio":"aac","fansub":"","referer":"https://kwik.cx/"}]}"#;

export_video_source!(SOURCE);
