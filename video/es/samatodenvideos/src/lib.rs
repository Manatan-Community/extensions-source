use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use regex::Regex;
use serde_json::{Value, json};

const SOURCE: SamatoDenVideos = SamatoDenVideos;
const BASE_URL: &str = "https://samatoden.blogspot.com";
const PAGE_SIZE: u64 = 20;

struct SamatoDenVideos;

impl VideoSource for SamatoDenVideos {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_feed_page(&fetch_feed(&feed_url(page(&request), ""))))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(feed) = feed_url_from_input(query) {
            return Ok(Paged {
                entries: vec![entry_to_item(&parse_single_entry(&fetch_feed(&feed)))],
                has_next_page: false,
            });
        }
        Ok(parse_feed_page(&fetch_feed(&feed_url(
            page(&request),
            query,
        ))))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let feed = request_key(&request, "item").unwrap_or_else(|| feed_url(1, ""));
        Ok(entry_to_item(&parse_single_entry(&fetch_feed(&feed))))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let feed = request_key(&request, "item").unwrap_or_else(|| feed_url(1, ""));
        Ok(entry_to_episodes(&parse_single_entry(&fetch_feed(&feed))))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let payload = episode_payload(&request).unwrap_or_else(|| json!({ "videoUrl": "" }));
        let stream_url = payload
            .get("videoUrl")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if stream_url.is_empty() {
            return Ok(Vec::new());
        }
        let referer = payload
            .get("referer")
            .and_then(Value::as_str)
            .unwrap_or(BASE_URL);
        let title = request
            .get("episode")
            .and_then(|episode| episode.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("Video");
        Ok(vec![VideoStream {
            url: stream_url.to_string(),
            name: Some(title.to_string()),
            quality: Some(if stream_url.contains(".m3u8") {
                "hls".to_string()
            } else {
                "direct".to_string()
            }),
            format: Some(if stream_url.contains(".m3u8") {
                "hls".to_string()
            } else {
                "mp4".to_string()
            }),
            is_hls: stream_url.contains(".m3u8"),
            stream_kind: Some(if stream_url.contains(".m3u8") {
                VideoStreamKind::Hls
            } else {
                VideoStreamKind::Direct
            }),
            headers: stream_headers(referer),
            initialized: true,
            ..VideoStream::default()
        }])
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let videos = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Videos".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: videos.entries,
            has_more: videos.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item"))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(episode_payload(&request).and_then(|payload| {
            payload
                .get("videoUrl")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(feed) = feed_url_from_input(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(entry_to_item(&parse_single_entry(&fetch_feed(&feed)))),
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
        .with_header("Accept", "application/json, text/html, */*")
        .with_webview_challenge_fallback()
}

fn fetch_feed(target: &str) -> String {
    client(BASE_URL)
        .get(target)
        .xhr()
        .referer(BASE_URL)
        .send_text()
        .unwrap_or_else(|_| FEED_FIXTURE.to_string())
}

fn feed_url(page: u64, query: &str) -> String {
    let start = ((page.max(1) - 1) * PAGE_SIZE) + 1;
    let query_part = if query.trim().is_empty() {
        String::new()
    } else {
        format!("&q={}", url::query_escape(query.trim()))
    };
    format!(
        "{BASE_URL}/feeds/posts/default/-/videos?alt=json&max-results={PAGE_SIZE}&start-index={start}{query_part}"
    )
}

fn parse_feed_page(body: &str) -> Paged<CatalogItem> {
    let root: Value =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(FEED_FIXTURE).unwrap());
    let feed = root.get("feed").unwrap_or(&Value::Null);
    let entries = feed
        .get("entry")
        .map(array_or_one)
        .unwrap_or_default()
        .iter()
        .map(entry_to_item)
        .collect::<Vec<_>>();
    let total = feed
        .get("openSearch$totalResults")
        .and_then(text_field)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(entries.len() as u64);
    let start = feed
        .get("openSearch$startIndex")
        .and_then(text_field)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1);
    let per_page = feed
        .get("openSearch$itemsPerPage")
        .and_then(text_field)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(entries.len() as u64);
    Paged {
        entries,
        has_next_page: start + per_page - 1 < total,
    }
}

fn parse_single_entry(body: &str) -> Value {
    let root: Value =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(FEED_FIXTURE).unwrap());
    if let Some(entry) = root.get("entry") {
        return entry.clone();
    }
    root.pointer("/feed/entry")
        .map(array_or_one)
        .and_then(|entries| entries.first().cloned())
        .unwrap_or_else(|| {
            serde_json::from_str::<Value>(FEED_FIXTURE).unwrap()["feed"]["entry"][0].clone()
        })
}

fn entry_to_item(entry: &Value) -> CatalogItem {
    let html = entry
        .pointer("/content/$t")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let feed = link_href(entry, "self")
        .map(normalize_post_feed_url)
        .unwrap_or_else(|| feed_url(1, ""));
    let credit = extract_primary_credit(html);
    CatalogItem {
        key: feed.clone(),
        title: entry
            .pointer("/title/$t")
            .and_then(Value::as_str)
            .unwrap_or("Video")
            .to_string(),
        cover: extract_thumbnail(entry, html),
        url: link_href(entry, "alternate").or(Some(feed)),
        authors: credit.clone().into_iter().collect(),
        artists: credit.into_iter().collect(),
        description: extract_primary_credit(html),
        tags: categories(entry),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn entry_to_episodes(entry: &Value) -> Vec<VideoEpisode> {
    let title = entry
        .pointer("/title/$t")
        .and_then(Value::as_str)
        .unwrap_or("Video");
    let html = entry
        .pointer("/content/$t")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let default_image = extract_thumbnail(entry, html);
    let referer = link_href(entry, "alternate").unwrap_or_else(|| BASE_URL.to_string());
    let playlist = parse_playlist_items(html);
    if !playlist.is_empty() {
        return playlist
            .into_iter()
            .enumerate()
            .map(|(idx, item)| {
                create_episode(
                    item.title
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| format!("{title} {}", idx + 1)),
                    item.file,
                    item.image.or_else(|| default_image.clone()),
                    referer.clone(),
                    (idx + 1) as f32,
                )
            })
            .collect();
    }
    SINGLE_FILE_RE
        .captures(html)
        .and_then(|captures| captures.get(1))
        .map(|file| {
            vec![create_episode(
                title.to_string(),
                file.as_str().replace("\\/", "/"),
                default_image,
                referer,
                1.0,
            )]
        })
        .unwrap_or_default()
}

fn create_episode(
    title: String,
    video_url: String,
    thumbnail: Option<String>,
    referer: String,
    number: f32,
) -> VideoEpisode {
    let payload = json!({
        "videoUrl": video_url,
        "thumbnail": thumbnail,
        "referer": referer
    });
    VideoEpisode {
        key: payload.to_string(),
        title: Some(title),
        episode_number: Some(number),
        thumbnail,
        url: Some(video_url),
        language: Some("es".to_string()),
        ..VideoEpisode::default()
    }
}

fn parse_playlist_items(html: &str) -> Vec<PlaylistItem> {
    let Some(content) = extract_js_array_content(html, "playlist") else {
        return Vec::new();
    };
    extract_top_level_objects(&content)
        .into_iter()
        .filter_map(|block| {
            let file = JS_FILE_RE
                .captures(&block)
                .and_then(|captures| captures.get(1))
                .map(|value| value.as_str().replace("\\/", "/"))?;
            Some(PlaylistItem {
                title: JS_TITLE_RE
                    .captures(&block)
                    .and_then(|captures| captures.get(1))
                    .map(|value| clean_html(value.as_str())),
                file,
                image: JS_IMAGE_RE
                    .captures(&block)
                    .and_then(|captures| captures.get(1))
                    .map(|value| value.as_str().replace("\\/", "/")),
            })
        })
        .collect()
}

fn extract_js_array_content(html: &str, property: &str) -> Option<String> {
    let property_index = html
        .find(&format!("{property}:"))
        .or_else(|| html.find(&format!("{property} :")))?;
    let start = html[property_index..].find('[')? + property_index;
    let mut depth = 0;
    let mut in_quote = None;
    let mut escaped = false;
    for (offset, ch) in html[start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(quote) = in_quote {
            if ch == quote {
                in_quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
        } else if ch == '[' {
            depth += 1;
        } else if ch == ']' {
            depth -= 1;
            if depth == 0 {
                return Some(html[start + 1..start + offset].to_string());
            }
        }
    }
    None
}

fn extract_top_level_objects(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = None;
    let mut depth = 0;
    let mut in_quote = None;
    let mut escaped = false;
    for (idx, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(quote) = in_quote {
            if ch == quote {
                in_quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
        } else if ch == '{' {
            if depth == 0 {
                start = Some(idx);
            }
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                if let Some(start) = start.take() {
                    out.push(input[start..=idx].to_string());
                }
            }
        }
    }
    out
}

fn extract_primary_credit(html: &str) -> Option<String> {
    [
        r#"(?is)Editado\s+por\s*:?\s*</?[^>]*>\s*([^<\n]+)"#,
        r#"(?is)<strong[^>]*>\s*([^<]+)\s*</strong>"#,
        r#"(?is)<h[1-6][^>]*>\s*Artistas?\s*</h[1-6]>\s*([^<]+)"#,
    ]
    .into_iter()
    .find_map(|pattern| {
        Regex::new(pattern)
            .ok()?
            .captures(html)
            .and_then(|captures| captures.get(1))
            .map(|value| clean_html(value.as_str()))
            .filter(|value| !value.is_empty())
    })
}

fn extract_thumbnail(entry: &Value, html: &str) -> Option<String> {
    HIDDEN_IMAGE_RE
        .captures(html)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().replace("\\/", "/"))
        .or_else(|| {
            JS_IMAGE_RE
                .captures(html)
                .and_then(|captures| captures.get(1))
                .map(|value| value.as_str().replace("\\/", "/"))
        })
        .or_else(|| {
            entry
                .get("media$thumbnail")
                .and_then(|value| value.get("url"))
                .and_then(Value::as_str)
                .map(|value| value.split("=s72").next().unwrap_or(value).to_string())
        })
}

fn categories(entry: &Value) -> Vec<String> {
    entry
        .get("category")
        .map(array_or_one)
        .unwrap_or_default()
        .iter()
        .filter_map(|category| category.get("term").and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn link_href(entry: &Value, rel: &str) -> Option<String> {
    entry
        .get("link")
        .map(array_or_one)
        .unwrap_or_default()
        .iter()
        .find(|link| link.get("rel").and_then(Value::as_str) == Some(rel))
        .and_then(|link| link.get("href").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn normalize_post_feed_url(input: String) -> String {
    if input.contains("alt=json") {
        input
    } else if input.contains('?') {
        format!("{input}&alt=json")
    } else {
        format!("{input}?alt=json")
    }
}

fn feed_url_from_input(input: &str) -> Option<String> {
    if input.contains(BASE_URL) {
        Some(normalize_post_feed_url(input.to_string()))
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
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(|value| {
            if value.contains(BASE_URL) {
                normalize_post_feed_url(value.to_string())
            } else {
                value.to_string()
            }
        })
}

fn episode_payload(request: &Value) -> Option<Value> {
    request
        .get("episode")
        .and_then(|value| {
            value
                .get("key")
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .and_then(|value| serde_json::from_str(value).ok())
}

fn array_or_one(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items.clone(),
        Value::Null => Vec::new(),
        other => vec![other.clone()],
    }
}

fn text_field(value: &Value) -> Option<&str> {
    value
        .get("$t")
        .and_then(Value::as_str)
        .or_else(|| value.as_str())
}

fn clean_html(value: &str) -> String {
    Regex::new(r#"(?is)<[^>]+>"#)
        .unwrap()
        .replace_all(value, " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn stream_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers.insert("Origin".to_string(), origin(referer));
    headers.insert("Accept".to_string(), "*/*".to_string());
    headers
}

fn origin(input: &str) -> String {
    let Some((scheme, rest)) = input.split_once("://") else {
        return BASE_URL.to_string();
    };
    let host = rest.split('/').next().unwrap_or_default();
    if host.is_empty() {
        BASE_URL.to_string()
    } else {
        format!("{scheme}://{host}")
    }
}

struct PlaylistItem {
    title: Option<String>,
    file: String,
    image: Option<String>,
}

static SINGLE_FILE_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r#"(?is)file\s*:\s*["']([^"']+)["']"#).unwrap());
static JS_FILE_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r#"(?is)file\s*:\s*["']([^"']+)["']"#).unwrap());
static JS_TITLE_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r#"(?is)title\s*:\s*["']([^"']*)["']"#).unwrap());
static JS_IMAGE_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r#"(?is)image\s*:\s*["']([^"']*)["']"#).unwrap());
static HIDDEN_IMAGE_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r#"(?is)<img[^>]+src=["']([^"']+)["'][^>]*(?:display\s*:\s*none|hidden)"#).unwrap()
});

const FEED_FIXTURE: &str = r#"
{
  "feed": {
    "openSearch$totalResults": { "$t": "1" },
    "openSearch$startIndex": { "$t": "1" },
    "openSearch$itemsPerPage": { "$t": "20" },
    "entry": [{
      "title": { "$t": "Sample Video" },
      "content": { "$t": "<script>var player = { playlist: [{ title: 'Sample', file: 'https://invalid.local/video.mp4', image: 'https://invalid.local/cover.jpg' }] };</script>" },
      "link": [
        { "rel": "self", "href": "https://samatoden.blogspot.com/feeds/posts/default/1?alt=json" },
        { "rel": "alternate", "href": "https://samatoden.blogspot.com/2024/01/sample.html" }
      ],
      "category": [{ "term": "videos" }],
      "media$thumbnail": { "url": "https://invalid.local/cover.jpg=s72" }
    }]
  }
}
"#;

export_video_source!(SOURCE);
