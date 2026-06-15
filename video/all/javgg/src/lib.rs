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

const SOURCE: Javgg = Javgg;
const BASE_URL: &str = "https://javgg.net";

struct Javgg;

impl VideoSource for Javgg {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let path = if listing == "latest" {
            format!("{BASE_URL}/new-post/page/{page}")
        } else {
            format!("{BASE_URL}/trending/page/{page}")
        };
        let body = client()
            .get(path)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_cards(&body, listing == "latest"))
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
        if query.is_empty() {
            return self.list(request);
        }
        let target = format!(
            "{BASE_URL}/jav/page/{}?s={}",
            page(&request),
            url::query_escape(query)
        );
        let body = client()
            .get(target)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_search_cards(&body))
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
        if !body.contains("dooplay_player_option") && !body.contains("source-player") {
            return Ok(Vec::new());
        }
        Ok(vec![VideoEpisode {
            key: key.clone(),
            title: Some("Episode 1".to_string()),
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
        Ok(parse_hosters(&body, &target))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(hoster_key) = request_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = hoster_key.splitn(3, '|');
        let server = parts.next().unwrap_or("Hoster");
        let embed = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(BASE_URL);
        if embed.is_empty() {
            return Ok(Vec::new());
        }
        if server.eq_ignore_ascii_case("TurboPlay") {
            let body = client()
                .get(embed)
                .browser_document()
                .referer(referer)
                .send_text()
                .unwrap_or_default();
            if let Some(master) = html::attr_after(&body, "id=\"video_player\"", "data-hash")
                .filter(|value| !value.is_empty())
            {
                return Ok(vec![stream(
                    &master,
                    "TurboPlay",
                    "hls",
                    Some(embed.to_string()),
                    Some(VideoStreamKind::Hls),
                )]);
            }
        }
        Ok(vec![stream(
            embed,
            server,
            "external",
            Some(referer.to_string()),
            Some(VideoStreamKind::External),
        )])
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
                title: "Trending".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "New Post".to_string(),
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
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn parse_cards(body: &str, _latest: bool) -> Paged<CatalogItem> {
    let entries = body
        .split("article")
        .filter(|block| block.contains("post-") && block.contains("data h3"))
        .filter_map(parse_card)
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("id=\"nextpagination\"") || body.contains("nextpagination"),
    }
}

fn parse_search_cards(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("result-item")
        .skip(1)
        .filter_map(parse_card)
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("id=\"nextpagination\"") || body.contains("nextpagination"),
    }
}

fn parse_card(block: &str) -> Option<CatalogItem> {
    let href = html::attr_after(block, "<a", "href")?;
    let key = slug_from_url(&href)?;
    let title = html::text_between(block, "<h3", "</h3>")
        .or_else(|| html::text_between(block, "class=\"title", "</div>"))
        .map(|text| html::strip_tags(&text))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| key.replace('-', " "));
    Some(CatalogItem {
        key: normalize_key(&key),
        title,
        cover: image_url(block),
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
    let title = html::text_between(&body, "<h1", "</h1>")
        .or_else(|| html::text_between(&body, "<h3", "</h3>"))
        .map(|text| html::strip_tags(&text))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| key.replace('-', " "));
    let info = body.split("class=\"data").nth(1).unwrap_or(&body);
    CatalogItem {
        key: key.clone(),
        title,
        cover: image_url(&body),
        description: html::text_between(&body, "id=\"cover", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: linked_values(info, "Genres:"),
        artists: linked_values(info, "Cast:"),
        authors: linked_values(info, "Maker:"),
        url: Some(item_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_hosters(body: &str, page_url: &str) -> Vec<VideoHoster> {
    body.split("id=\"source-player-")
        .skip(1)
        .filter_map(|block| {
            let num = block.split('"').next().unwrap_or_default();
            let embed = html::attr_after(block, "<iframe", "src")?;
            let marker = format!("data-nume=\"{num}\"");
            let server = body
                .split(&marker)
                .nth(1)
                .and_then(|chunk| html::attr_after(chunk, "class=\"server", "data-text"))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| host_name(&embed));
            Some(VideoHoster {
                key: format!("{server}|{embed}|{page_url}"),
                name: server,
                url: Some(page_url.to_string()),
                lazy: true,
                video_count: Some(1),
                ..VideoHoster::default()
            })
        })
        .collect()
}

fn stream(
    target: &str,
    name: &str,
    format: &str,
    referer: Option<String>,
    kind: Option<VideoStreamKind>,
) -> VideoStream {
    let mut headers = Context::new();
    if let Some(referer) = referer {
        headers.insert("Referer".to_string(), referer);
    }
    VideoStream {
        url: target.to_string(),
        name: Some(name.to_string()),
        quality: Some(name.to_string()),
        format: Some(format.to_string()),
        is_hls: matches!(kind, Some(VideoStreamKind::Hls)),
        stream_kind: kind,
        headers,
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred_server = pref(request, "preferred_server").unwrap_or_else(|| "StreamWish".to_string());
    streams.sort_by_key(|stream| {
        let name = stream.name.as_deref().unwrap_or_default().to_ascii_lowercase();
        if name.contains(&preferred_server.to_ascii_lowercase()) {
            0
        } else {
            1
        }
    });
}

fn linked_values(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .take_while(|chunk| !chunk.contains("boxye2"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .collect()
}

fn image_url(block: &str) -> Option<String> {
    for attr in ["src", "data-src", "href"] {
        if let Some(value) = html::attr_after(block, "<img", attr)
            .or_else(|| html::attr_after(block, "<a", attr))
            .filter(|value| value.contains(".jpg") || value.contains(".png") || value.contains(".webp"))
        {
            return Some(value);
        }
    }
    None
}

fn host_name(embed: &str) -> String {
    embed
        .split("//")
        .nth(1)
        .and_then(|tail| tail.split('/').next())
        .unwrap_or("Hoster")
        .replace("www.", "")
}

fn slug_from_url(input: &str) -> Option<String> {
    if input.trim().is_empty() {
        return None;
    }
    let clean = input.split('?').next().unwrap_or(input).trim_end_matches('/');
    if clean.starts_with("http") && !clean.contains("javgg.net") {
        return None;
    }
    clean
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty() && *value != "javgg.net" && *value != "jav")
        .map(ToString::to_string)
}

fn normalize_key(key: &str) -> String {
    slug_from_url(key).unwrap_or_else(|| key.trim_matches('/').to_string())
}

fn item_url(key: &str) -> String {
    if key.starts_with("http") {
        key.to_string()
    } else {
        format!("{BASE_URL}/jav/{}/", key.trim_matches('/'))
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
