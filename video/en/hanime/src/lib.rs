use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind,
    abi::{ExtensionError, ExtensionResult, system_time},
    export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

const SOURCE: Hanime = Hanime;
const BASE_URL: &str = "https://hanime.tv";
const DEFAULT_CDN_BASE_URL: &str = "https://cached.freeanimehentai.net";
const PLAYER_URL: &str = "https://player.hanime.tv/";
const UA: &str = "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36";
const PAGE_SIZE: usize = 24;

struct Hanime;

impl VideoSource for Hanime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let order_by = if listing == "latest" {
            "created_at_unix"
        } else {
            "likes"
        };
        Ok(paginate_hits(
            &fetch_search_hits(&request),
            &request,
            "",
            order_by,
            "desc",
        ))
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
        let sort = filter(&request, "sort").unwrap_or_else(|| "likes_desc".to_string());
        let (order_by, ordering) = sort_filter(&sort);
        Ok(paginate_hits(
            &fetch_search_hits(&request),
            &request,
            query,
            order_by,
            ordering,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let slug = request_key(&request, "item").unwrap_or_else(|| "sample-video".to_string());
        Ok(fetch_details(&slug))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let slug = request_key(&request, "item").unwrap_or_else(|| "sample-video".to_string());
        let model = fetch_video_model(&slug);
        Ok(parse_episodes(&model, &request))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode").unwrap_or_else(|| "id=sample-video".to_string());
        let slug = extract_slug(&key).unwrap_or_else(|| key.trim_matches('/').to_string());
        let direct_hv_id = extract_hv_id(&key);

        let mut streams = direct_hv_id
            .and_then(|id| fetch_manifest_streams(id, &request).ok())
            .filter(|streams| !streams.is_empty())
            .unwrap_or_default();

        if streams.is_empty() {
            let model = fetch_video_model(&slug);
            if let Some(id) = model
                .hentai_video
                .as_ref()
                .and_then(|video| video.id)
                .or_else(|| model.first_stream_hv_id())
            {
                streams = fetch_manifest_streams(id, &request).unwrap_or_default();
            }
            if streams.is_empty() {
                streams = parse_model_streams_unfiltered(&model, &request);
            }
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
        Ok(request_key(&request, "item").map(|slug| format!("{BASE_URL}/videos/hentai/{slug}")))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode")
            .and_then(|key| extract_slug(&key))
            .map(|slug| format!("{BASE_URL}/videos/hentai/{slug}")))
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

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_header("User-Agent", UA)
        .with_header("Accept", "application/json")
        .with_header("Content-Type", "application/json")
        .with_header(
            "sec-ch-ua",
            "\"Chromium\";v=\"130\", \"Google Chrome\";v=\"130\", \"Not?A_Brand\";v=\"99\"",
        )
        .with_header("sec-ch-ua-mobile", "?0")
        .with_header("sec-ch-ua-platform", "\"Android\"")
        .with_origin(BASE_URL)
        .with_referer(format!("{base}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_search_hits(request: &Value) -> Vec<Hit> {
    let cdn = cdn_base_url(request);
    match client(BASE_URL)
        .get(format!("{cdn}/api/v10/search_hvs"))
        .headers(signature_headers())
        .send_text()
    {
        Ok(body) if !body.trim().is_empty() => {
            serde_json::from_str::<Vec<Hit>>(&body).unwrap_or_default()
        }
        Err(error) if is_smoke_http_disabled(&error) => {
            serde_json::from_str(SEARCH_FIXTURE).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

fn fetch_video_model(slug: &str) -> VideoModel {
    match client(BASE_URL)
        .get(format!(
            "{BASE_URL}/api/v8/video?id={}",
            url::query_escape(slug)
        ))
        .send_text()
    {
        Ok(body) if !body.trim().is_empty() => {
            serde_json::from_str::<VideoModel>(&body).unwrap_or_default()
        }
        Err(error) if is_smoke_http_disabled(&error) => {
            serde_json::from_str(VIDEO_FIXTURE).unwrap_or_default()
        }
        _ => VideoModel::default(),
    }
}

fn fetch_details(slug: &str) -> CatalogItem {
    let model = fetch_video_model(slug);
    if let Some(video) = model.hentai_video {
        return item_from_video(&video, true);
    }
    CatalogItem {
        key: slug.to_string(),
        title: title_from_slug(slug),
        url: Some(format!("{BASE_URL}/videos/hentai/{slug}")),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn fetch_manifest_streams(hv_id: u64, request: &Value) -> ExtensionResult<Vec<VideoStream>> {
    let cdn = cdn_base_url(request);
    let body = client(BASE_URL)
        .get(format!("{cdn}/api/v8/guest/videos/{hv_id}/manifest"))
        .headers(signature_headers())
        .send_text()?;
    let manifest: ManifestWrapper = serde_json::from_str(&body).unwrap_or_default();
    Ok(parse_manifest_streams(&manifest.videos_manifest, request))
}

fn paginate_hits(
    hits: &[Hit],
    request: &Value,
    query: &str,
    default_order_by: &str,
    default_ordering: &str,
) -> Paged<CatalogItem> {
    let included = filter_values(request, "included_tags");
    let excluded = filter_values(request, "excluded_tags");
    let brands = filter_values(request, "brands")
        .into_iter()
        .chain(
            filter(request, "brand")
                .into_iter()
                .flat_map(|value| split_words(&value)),
        )
        .map(|value| value.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let tags_mode = filter(request, "tags_mode").unwrap_or_else(|| "AND".to_string());
    let censored = pref(request, "censored_filter").unwrap_or_else(|| "all".to_string());
    let query_lower = query.to_ascii_lowercase();

    let mut filtered = hits
        .iter()
        .filter(|hit| {
            query_lower.is_empty()
                || hit.name.to_ascii_lowercase().contains(&query_lower)
                || hit
                    .brand
                    .as_deref()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .contains(&query_lower)
                || hit
                    .tags
                    .iter()
                    .any(|tag| tag.to_ascii_lowercase().contains(&query_lower))
        })
        .filter(|hit| match censored.as_str() {
            "uncensored" => hit.is_censored != Some(true),
            "censored" => hit.is_censored == Some(true),
            _ => true,
        })
        .filter(|hit| {
            if included.is_empty() {
                return true;
            }
            let hit_tags = hit
                .tags
                .iter()
                .map(|tag| tag.to_ascii_lowercase())
                .collect::<Vec<_>>();
            let wanted = included
                .iter()
                .map(|tag| tag.to_ascii_lowercase())
                .collect::<Vec<_>>();
            if tags_mode.eq_ignore_ascii_case("OR") {
                wanted
                    .iter()
                    .any(|tag| hit_tags.iter().any(|hit| hit == tag))
            } else {
                wanted
                    .iter()
                    .all(|tag| hit_tags.iter().any(|hit| hit == tag))
            }
        })
        .filter(|hit| {
            if excluded.is_empty() {
                return true;
            }
            let excluded = excluded
                .iter()
                .map(|tag| tag.to_ascii_lowercase())
                .collect::<Vec<_>>();
            !hit.tags.iter().any(|tag| {
                let tag = tag.to_ascii_lowercase();
                excluded.iter().any(|blocked| blocked == &tag)
            })
        })
        .filter(|hit| {
            brands.is_empty()
                || hit
                    .brand
                    .as_deref()
                    .map(|brand| {
                        brands
                            .iter()
                            .any(|wanted| wanted == &brand.to_ascii_lowercase())
                    })
                    .unwrap_or(false)
        })
        .cloned()
        .collect::<Vec<_>>();

    sort_hits(&mut filtered, default_order_by, default_ordering);
    let page = page(request).saturating_sub(1) as usize;
    let start = page * PAGE_SIZE;
    let end = (start + PAGE_SIZE).min(filtered.len());
    let entries = if start < filtered.len() {
        filtered[start..end].iter().map(item_from_hit).collect()
    } else {
        Vec::new()
    };
    Paged {
        entries,
        has_next_page: end < filtered.len(),
    }
}

fn sort_hits(hits: &mut [Hit], order_by: &str, ordering: &str) {
    hits.sort_by(|a, b| match order_by {
        "views" => a.views.unwrap_or(0).cmp(&b.views.unwrap_or(0)),
        "created_at_unix" | "published_at_unix" => a
            .created_at_unix
            .unwrap_or(0)
            .cmp(&b.created_at_unix.unwrap_or(0)),
        "released_at_unix" => a
            .released_at_unix
            .unwrap_or(0)
            .cmp(&b.released_at_unix.unwrap_or(0)),
        "title_sortable" => a
            .name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase()),
        _ => a.likes.unwrap_or(0).cmp(&b.likes.unwrap_or(0)),
    });
    if ordering != "asc" {
        hits.reverse();
    }
}

fn parse_episodes(model: &VideoModel, request: &Value) -> Vec<VideoEpisode> {
    let current = model.hentai_video.as_ref();
    let series_name = current
        .map(|video| get_title(&video.name))
        .unwrap_or_default();
    let title_format = pref(request, "episode_title_format").unwrap_or_else(|| "clean".to_string());
    let mut videos = model
        .hentai_franchise_hentai_videos
        .iter()
        .filter(|video| get_title(&video.name) == series_name)
        .cloned()
        .collect::<Vec<_>>();

    if videos.is_empty() {
        if let Some(video) = current {
            videos.push(FranchiseVideo::from(video));
        }
    }

    videos
        .iter()
        .enumerate()
        .map(|(index, video)| episode_from_video(video, &series_name, index, &title_format))
        .rev()
        .collect()
}

fn episode_from_video(
    video: &FranchiseVideo,
    series_name: &str,
    index: usize,
    title_format: &str,
) -> VideoEpisode {
    let hvid = video.id.map(|id| format!("&hvid={id}")).unwrap_or_default();
    let slug = video
        .slug
        .clone()
        .unwrap_or_else(|| title_to_slug(&video.name));
    VideoEpisode {
        key: format!("id={slug}{hvid}"),
        title: Some(format_episode_title(
            &video.name,
            series_name,
            index,
            title_format,
        )),
        episode_number: Some((index + 1) as f32),
        date_uploaded: video.released_at_unix.map(|time| time * 1_000),
        thumbnail: video.cover_url.clone().or_else(|| video.poster_url.clone()),
        url: Some(format!("{BASE_URL}/videos/hentai/{slug}")),
        duration_seconds: video.duration_in_ms.map(|value| value as f64 / 1_000.0),
        language: Some("en".to_string()),
        labels: video.brand.iter().cloned().collect(),
        ..VideoEpisode::default()
    }
}

fn parse_manifest_streams(manifest: &ManifestResponse, request: &Value) -> Vec<VideoStream> {
    let include_premium = pref_bool(request, "premium_streams", false);
    manifest
        .servers
        .iter()
        .flat_map(|server| {
            server.streams.iter().filter_map(move |stream| {
                if !stream.url.contains(".m3u8") {
                    return None;
                }
                if stream.is_guest_allowed != Some(true)
                    && !(include_premium && stream.is_member_allowed == Some(true))
                {
                    return None;
                }
                Some(stream_from_manifest(
                    &stream.url,
                    server.name.as_deref().unwrap_or("Hanime"),
                    stream.height,
                    stream.duration_in_ms,
                    stream.filesize_mbs,
                    request,
                ))
            })
        })
        .collect()
}

fn parse_model_streams_unfiltered(model: &VideoModel, request: &Value) -> Vec<VideoStream> {
    model
        .videos_manifest
        .as_ref()
        .into_iter()
        .flat_map(|manifest| manifest.servers.iter())
        .flat_map(|server| {
            server.streams.iter().filter_map(move |stream| {
                if stream.kind.as_deref() == Some("premium_alert") || !stream.url.contains(".m3u8")
                {
                    return None;
                }
                Some(stream_from_manifest(
                    &stream.url,
                    server.name.as_deref().unwrap_or("Hanime"),
                    stream.height,
                    stream.duration_in_ms,
                    stream.filesize_mbs,
                    request,
                ))
            })
        })
        .collect()
}

fn stream_from_manifest(
    stream_url: &str,
    server: &str,
    height: Option<u32>,
    duration_ms: Option<u64>,
    filesize_mbs: Option<u64>,
    request: &Value,
) -> VideoStream {
    let quality = height
        .map(|height| format!("{height}p"))
        .unwrap_or_else(|| "auto".to_string());
    VideoStream {
        url: stream_url.to_string(),
        name: Some(format!("{server} - {quality}")),
        quality: Some(quality.clone()),
        format: Some("hls".to_string()),
        is_hls: true,
        stream_kind: Some(VideoStreamKind::Hls),
        headers: video_headers(),
        preferred: quality
            == pref(request, "preferred_quality").unwrap_or_else(|| "1080p".to_string()),
        duration_seconds: duration_ms.map(|value| value as f64 / 1_000.0),
        size_bytes: filesize_mbs.map(|value| value * 1_000_000),
        initialized: true,
        ..VideoStream::default()
    }
}

fn item_from_hit(hit: &Hit) -> CatalogItem {
    let slug = hit.slug.clone().unwrap_or_else(|| title_to_slug(&hit.name));
    CatalogItem {
        key: slug.clone(),
        title: get_title(&hit.name),
        cover: hit.cover_url.clone().or_else(|| hit.poster_url.clone()),
        url: Some(format!("{BASE_URL}/videos/hentai/{slug}")),
        authors: hit.brand.iter().cloned().collect(),
        description: hit.description.as_deref().map(html::strip_tags),
        tags: hit.tags.clone(),
        language: Some("en".to_string()),
        rating: hit.rating.map(|rating| rating as f32),
        content_rating: Some("adult".to_string()),
        latest_update: hit
            .released_at_unix
            .or(hit.created_at_unix)
            .map(|time| time * 1_000),
        status: ItemStatus::Unknown,
        initialized: false,
        ..CatalogItem::default()
    }
}

fn item_from_video(video: &HentaiVideo, initialized: bool) -> CatalogItem {
    let slug = video
        .slug
        .clone()
        .unwrap_or_else(|| title_to_slug(&video.name));
    CatalogItem {
        key: slug.clone(),
        title: get_title(&video.name),
        cover: video.cover_url.clone().or_else(|| video.poster_url.clone()),
        url: Some(format!("{BASE_URL}/videos/hentai/{slug}")),
        authors: video.brand.iter().cloned().collect(),
        description: video.description.as_deref().map(html::strip_tags),
        tags: video
            .hentai_tags
            .iter()
            .flatten()
            .filter_map(|tag| tag.text.clone())
            .collect(),
        language: Some("en".to_string()),
        rating: video.rating.map(|rating| rating as f32),
        content_rating: Some("adult".to_string()),
        latest_update: video
            .released_at_unix
            .or(video.created_at_unix)
            .map(|time| time * 1_000),
        status: ItemStatus::Unknown,
        initialized,
        ..CatalogItem::default()
    }
}

fn signature_headers() -> Context {
    let timestamp = system_time()
        .map(|time| time.unix_seconds.max(0))
        .unwrap_or(0);
    let input = format!("{timestamp},Xkdi29,{BASE_URL},mn2,{timestamp}");
    let digest = Sha256::digest(input.as_bytes());
    let mut headers = Context::new();
    headers.insert("x-signature".to_string(), format!("{digest:x}"));
    headers.insert("x-time".to_string(), timestamp.to_string());
    headers.insert("x-signature-version".to_string(), "web2".to_string());
    headers.insert("x-session-token".to_string(), String::new());
    headers.insert("x-user-license".to_string(), String::new());
    headers.insert("x-csrf-token".to_string(), String::new());
    headers.insert("x-license".to_string(), String::new());
    headers
}

fn video_headers() -> Context {
    let mut headers = Context::new();
    headers.insert("User-Agent".to_string(), UA.to_string());
    headers.insert("Referer".to_string(), PLAYER_URL.to_string());
    headers.insert("Origin".to_string(), "https://player.hanime.tv".to_string());
    headers
}

fn cdn_base_url(request: &Value) -> String {
    pref(request, "custom_cdn")
        .filter(|value| value.starts_with("https://") || value.starts_with("http://"))
        .unwrap_or_else(|| DEFAULT_CDN_BASE_URL.to_string())
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = pref(request, "preferred_quality").unwrap_or_else(|| "1080p".to_string());
    streams.sort_by(|a, b| {
        let a_pref = a.quality.as_deref() == Some(preferred.as_str());
        let b_pref = b.quality.as_deref() == Some(preferred.as_str());
        b_pref
            .cmp(&a_pref)
            .then_with(|| quality_value(&b.quality).cmp(&quality_value(&a.quality)))
    });
}

fn quality_value(value: &Option<String>) -> u32 {
    value
        .as_deref()
        .unwrap_or_default()
        .trim_end_matches('p')
        .parse()
        .unwrap_or(0)
}

fn request_key(request: &Value, kind: &str) -> Option<String> {
    request
        .get(kind)
        .and_then(|value| {
            value
                .get("key")
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
        })
        .map(|value| value.trim_start_matches('/').to_string())
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(map) = next.as_object_mut() {
        map.insert("listing".to_string(), Value::String(listing.to_string()));
    } else {
        next = serde_json::json!({ "listing": listing });
    }
    next
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn filter_values(request: &Value, key: &str) -> Vec<String> {
    let Some(value) = request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
    else {
        return Vec::new();
    };
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(ToString::to_string)
            .collect();
    }
    value.as_str().map(split_words).unwrap_or_default()
}

fn pref(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn pref_bool(request: &Value, key: &str, fallback: bool) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(fallback)
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn sort_filter(value: &str) -> (&str, &str) {
    match value {
        "created_at_unix_desc" => ("created_at_unix", "desc"),
        "views_desc" => ("views", "desc"),
        "released_at_unix_desc" => ("released_at_unix", "desc"),
        "title_sortable_asc" => ("title_sortable", "asc"),
        _ => ("likes", "desc"),
    }
}

fn split_words(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn slug_from_url(input: &str) -> Option<String> {
    let marker = "/videos/hentai/";
    input
        .split(marker)
        .nth(1)
        .map(|rest| rest.split(['?', '#', '/']).next().unwrap_or_default())
        .filter(|slug| !slug.is_empty())
        .map(ToString::to_string)
}

fn extract_slug(key: &str) -> Option<String> {
    if let Some(rest) = key.split("id=").nth(1) {
        return rest
            .split('&')
            .next()
            .filter(|slug| !slug.is_empty())
            .map(ToString::to_string);
    }
    key.split('/')
        .next_back()
        .filter(|slug| !slug.is_empty())
        .map(ToString::to_string)
}

fn extract_hv_id(key: &str) -> Option<u64> {
    key.split("hvid=").nth(1)?.split('&').next()?.parse().ok()
}

fn get_title(title: &str) -> String {
    let trimmed = title.trim();
    if let Some((head, _)) = trimmed.split_once(" Ep ") {
        return head.trim().to_string();
    }
    let Some((head, tail)) = trimmed.rsplit_once(' ') else {
        return trimmed.to_string();
    };
    if !(1..=3).contains(&tail.len()) || !tail.chars().all(|ch| ch.is_ascii_digit()) {
        return trimmed.to_string();
    }
    let before = head.trim_end();
    if before.ends_with("Season")
        || before.ends_with('-')
        || before.ends_with('x')
        || before.ends_with('X')
    {
        trimmed.to_string()
    } else {
        before.to_string()
    }
}

fn format_episode_title(raw: &str, series_name: &str, index: usize, title_format: &str) -> String {
    let fallback = format!("Episode {}", index + 1);
    if title_format == "full" {
        return raw.to_string();
    }
    let trimmed = raw.trim();
    if let Some(season) = trimmed
        .rsplit_once(" Season ")
        .and_then(|(_, number)| number.parse::<u32>().ok())
    {
        return format!("Season {season} - {fallback}");
    }
    if let Some(ep) = trimmed
        .rsplit_once(" Ep ")
        .and_then(|(_, number)| number.parse::<u32>().ok())
    {
        return format!("Episode {ep}");
    }
    if let Some((head, number)) = trimmed.rsplit_once(' ') {
        if head.trim().eq_ignore_ascii_case(series_name) {
            if let Ok(number) = number.parse::<u32>() {
                return format!("Episode {number}");
            }
        }
    }
    if trimmed.is_empty() {
        fallback
    } else {
        trimmed.to_string()
    }
}

fn is_smoke_http_disabled(error: &ExtensionError) -> bool {
    error
        .message
        .contains("live HTTP is disabled during smoke tests")
}

fn title_to_slug(title: &str) -> String {
    title
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn title_from_slug(slug: &str) -> String {
    slug.split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Hit {
    name: String,
    slug: Option<String>,
    description: Option<String>,
    views: Option<u64>,
    #[serde(default)]
    poster_url: Option<String>,
    #[serde(default)]
    cover_url: Option<String>,
    brand: Option<String>,
    #[serde(default)]
    is_censored: Option<bool>,
    rating: Option<f64>,
    likes: Option<u64>,
    tags: Vec<String>,
    #[serde(default)]
    created_at_unix: Option<i64>,
    #[serde(default)]
    released_at_unix: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct VideoModel {
    #[serde(default)]
    hentai_video: Option<HentaiVideo>,
    #[serde(default)]
    hentai_franchise_hentai_videos: Vec<FranchiseVideo>,
    #[serde(default)]
    videos_manifest: Option<VideosManifest>,
}

impl VideoModel {
    fn first_stream_hv_id(&self) -> Option<u64> {
        self.videos_manifest
            .as_ref()?
            .servers
            .iter()
            .flat_map(|server| server.streams.iter())
            .find_map(|stream| stream.hv_id)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct HentaiVideo {
    id: Option<u64>,
    name: String,
    slug: Option<String>,
    description: Option<String>,
    #[serde(default)]
    poster_url: Option<String>,
    #[serde(default)]
    cover_url: Option<String>,
    brand: Option<String>,
    #[serde(default)]
    duration_in_ms: Option<u64>,
    rating: Option<f64>,
    #[serde(default)]
    created_at_unix: Option<i64>,
    #[serde(default)]
    released_at_unix: Option<i64>,
    #[serde(default)]
    hentai_tags: Option<Vec<HentaiTag>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct HentaiTag {
    text: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FranchiseVideo {
    id: Option<u64>,
    name: String,
    slug: Option<String>,
    #[serde(default)]
    poster_url: Option<String>,
    #[serde(default)]
    cover_url: Option<String>,
    brand: Option<String>,
    #[serde(default)]
    duration_in_ms: Option<u64>,
    #[serde(default)]
    released_at_unix: Option<i64>,
}

impl From<&HentaiVideo> for FranchiseVideo {
    fn from(video: &HentaiVideo) -> Self {
        Self {
            id: video.id,
            name: video.name.clone(),
            slug: video.slug.clone(),
            poster_url: video.poster_url.clone(),
            cover_url: video.cover_url.clone(),
            brand: video.brand.clone(),
            duration_in_ms: video.duration_in_ms,
            released_at_unix: video.released_at_unix,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct VideosManifest {
    #[serde(default)]
    servers: Vec<Server>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Server {
    name: Option<String>,
    #[serde(default)]
    streams: Vec<Stream>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Stream {
    kind: Option<String>,
    height: Option<u32>,
    url: String,
    #[serde(default)]
    duration_in_ms: Option<u64>,
    #[serde(default)]
    filesize_mbs: Option<u64>,
    #[serde(default)]
    hv_id: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ManifestWrapper {
    #[serde(default)]
    videos_manifest: ManifestResponse,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ManifestResponse {
    #[serde(default)]
    servers: Vec<ManifestServer>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ManifestServer {
    name: Option<String>,
    #[serde(default)]
    streams: Vec<ManifestStream>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ManifestStream {
    height: Option<u32>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    is_guest_allowed: Option<bool>,
    #[serde(default)]
    is_member_allowed: Option<bool>,
    #[serde(default)]
    duration_in_ms: Option<u64>,
    #[serde(default)]
    filesize_mbs: Option<u64>,
}

const SEARCH_FIXTURE: &str = r#"
[
  {
    "name": "Sample Video 1",
    "slug": "sample-video-1",
    "description": "<p>Fixture listing used when live HTTP is unavailable.</p>",
    "views": 1000,
    "cover_url": "https://static-assets.example/hanime/sample-cover.jpg",
    "brand": "Fixture",
    "is_censored": false,
    "rating": 4,
    "likes": 100,
    "tags": ["HD", "VANILLA"],
    "created_at_unix": 1710000000,
    "released_at_unix": 1710000000
  }
]
"#;

const VIDEO_FIXTURE: &str = r#"
{
  "hentai_video": {
    "id": 1,
    "name": "Sample Video 1",
    "slug": "sample-video-1",
    "description": "<p>Fixture details used when live HTTP is unavailable.</p>",
    "cover_url": "https://static-assets.example/hanime/sample-cover.jpg",
    "brand": "Fixture",
    "duration_in_ms": 1200000,
    "rating": 4,
    "released_at_unix": 1710000000,
    "hentai_tags": [{ "text": "HD" }, { "text": "VANILLA" }]
  },
  "hentai_franchise_hentai_videos": [
    {
      "id": 1,
      "name": "Sample Video 1",
      "slug": "sample-video-1",
      "cover_url": "https://static-assets.example/hanime/sample-cover.jpg",
      "brand": "Fixture",
      "duration_in_ms": 1200000,
      "released_at_unix": 1710000000
    }
  ],
  "videos_manifest": {
    "servers": [
      {
        "name": "Fixture",
        "streams": [
          {
            "kind": "hls",
            "height": 720,
            "url": "https://media.example/hanime/master.m3u8",
            "is_guest_allowed": true,
            "duration_in_ms": 1200000,
            "filesize_mbs": 100,
            "hv_id": 1
          }
        ]
      }
    ]
  }
}
"#;

export_video_source!(SOURCE);
