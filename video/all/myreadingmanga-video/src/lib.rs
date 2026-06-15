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

const SOURCE: MyReadingManga = MyReadingManga;
const BASE_URL: &str = "https://myreadingmanga.info";

struct MyReadingManga;

impl VideoSource for MyReadingManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        login_if_needed(&request);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            let mut out = format!(
                "{BASE_URL}/?ep_filter_lang={}&ep_filter_category=video&s=",
                url::query_escape(&latest_language(&request))
            );
            if page(&request) > 1 {
                out.push_str("&paged=");
                out.push_str(&page(&request).to_string());
            }
            out
        } else {
            format!("{BASE_URL}/popular/popular-videos")
        };
        let body = client()
            .get(target)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(if listing == "latest" {
            parse_articles(&body)
        } else {
            parse_popular(&body)
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        login_if_needed(&request);
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
        let mut target = format!(
            "{BASE_URL}/page/{}/?ep_filter_category={}&s={}",
            page(&request),
            url::query_escape(&filter(&request, "category").unwrap_or_else(|| "video".to_string())),
            url::query_escape(query)
        );
        if pref_bool(&request, "enforce_language", true) {
            target.push_str("&ep_filter_lang=");
            target.push_str(&url::query_escape(&site_language(&request)));
        }
        append_filter(&mut target, &request, "sort", "ep_sort");
        append_filter(&mut target, &request, "genre", "ep_filter_genre");
        append_filter(&mut target, &request, "tag", "ep_filter_post_tag");
        append_filter(&mut target, &request, "artist", "ep_filter_artist");
        append_filter(&mut target, &request, "pairing", "ep_filter_pairing");
        append_filter(&mut target, &request, "status", "ep_filter_status");
        let body = client()
            .get(target)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_articles(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        Ok(fetch_details(&key, &request))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        login_if_needed(&request);
        let key = request_key(&request, "item").unwrap_or_default();
        let target = item_url(&key);
        let body = client()
            .get(&target)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        let last_page = body
            .split("page-numbers")
            .filter_map(|chunk| html::strip_tags(chunk).trim().parse::<u32>().ok())
            .max()
            .unwrap_or(1);
        let mut episodes = (1..=last_page)
            .map(|number| {
                let url = if number == 1 {
                    target.clone()
                } else {
                    format!("{}/{}", target.trim_end_matches('/'), number)
                };
                VideoEpisode {
                    key: url.clone(),
                    title: Some(format!("Ep. {number}")),
                    episode_number: Some(number as f32),
                    url: Some(url),
                    language: Some("all".to_string()),
                    ..VideoEpisode::default()
                }
            })
            .collect::<Vec<_>>();
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        login_if_needed(&request);
        let key = request_key(&request, "episode").unwrap_or_default();
        let body = client()
            .get(&key)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        let Some(video_url) = html::attr_after(&body, "video-container-ads", "src")
            .or_else(|| html::attr_after(&body, "<video", "src"))
            .filter(|value| !value.is_empty())
        else {
            return Ok(Vec::new());
        };
        let mut headers = Context::new();
        headers.insert("Referer".to_string(), key.clone());
        Ok(vec![VideoStream {
            url: url::join_url(BASE_URL, &video_url),
            name: Some("Default".to_string()),
            quality: Some("default".to_string()),
            format: Some(stream_format(&video_url).to_string()),
            stream_kind: Some(VideoStreamKind::Direct),
            headers,
            initialized: true,
            preferred: true,
            ..VideoStream::default()
        }])
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular Videos".to_string(),
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
        Ok(request_key(&request, "episode"))
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
        .with_header("X-Requested-With", "Manatan")
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn login_if_needed(request: &Value) {
    let username = pref(request, "username").unwrap_or_default();
    let password = pref(request, "password").unwrap_or_default();
    if username.trim().is_empty() || password.trim().is_empty() {
        return;
    }
    let _ = client()
        .post(format!("{BASE_URL}/wp-login.php"))
        .form(&[
            ("log", username.trim()),
            ("pwd", password.trim()),
            ("wp-submit", "Log In"),
            ("redirect_to", BASE_URL),
            ("testcookie", "1"),
        ])
        .send_text();
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<li")
            .filter(|block| block.contains("wpp-post-title") && block.contains("vlcsnap"))
            .filter_map(parse_popular_item)
            .collect(),
        has_next_page: false,
    }
}

fn parse_popular_item(block: &str) -> Option<CatalogItem> {
    let title_block = block.split("wpp-post-title").nth(1)?;
    let href = html::attr_after(title_block, "<a", "href")
        .or_else(|| html::attr_after(block, "<a", "href"))?;
    let key = slug_from_url(&href)?;
    let title = html::text_between(title_block, ">", "</a>")
        .map(|text| clean_title(&html::strip_tags(&text)))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| key.replace('-', " "));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_url(block),
        url: Some(item_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn parse_articles(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<article")
            .filter(|block| block.contains("category-video"))
            .filter_map(parse_article)
            .collect(),
        has_next_page: body.contains("pagination-next"),
    }
}

fn parse_article(block: &str) -> Option<CatalogItem> {
    let href = html::attr_after(block, "entry-title-link", "href")
        .or_else(|| html::attr_after(block, "<a", "href"))?;
    let key = slug_from_url(&href)?;
    let title = html::text_between(block, "entry-title-link", "</a>")
        .map(|text| clean_title(&html::strip_tags(&text)))
        .filter(|text| !text.is_empty())
        .or_else(|| html::attr_after(block, "<img", "alt").map(|value| clean_title(&value)))
        .unwrap_or_else(|| key.replace('-', " "));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_url(block),
        url: Some(item_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(key: &str, request: &Value) -> CatalogItem {
    login_if_needed(request);
    let key = slug_from_url(key).unwrap_or_else(|| key.trim_matches('/').to_string());
    let target = item_url(&key);
    let body = client()
        .get(&target)
        .browser_document()
        .send_text()
        .unwrap_or_default();
    let raw_title = html::text_between(&body, "<h1", "</h1>")
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| key.replace('-', " "));
    let tags = linked_values(&body, "href");
    let author = body
        .split("entry-terms")
        .find(|chunk| chunk.contains("artist"))
        .and_then(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty());
    let description = body
        .split("entry-content")
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, ">", "</div>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty());
    CatalogItem {
        key: key.clone(),
        title: clean_title(&raw_title),
        cover: image_url(&body),
        url: Some(target),
        description: description.or(Some(raw_title)),
        authors: author.iter().cloned().collect(),
        artists: author.into_iter().collect(),
        tags,
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: parse_status(&body),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn append_filter(target: &mut String, request: &Value, key: &str, uri_param: &str) {
    if let Some(value) = filter(request, key).filter(|value| !value.trim().is_empty()) {
        target.push('&');
        target.push_str(uri_param);
        target.push('=');
        target.push_str(&url::query_escape(&value));
    }
}

fn image_url(block: &str) -> Option<String> {
    for attr in ["data-src", "data-cfsrc", "src", "data-lazy-src"] {
        if let Some(value) = html::attr_after(block, "<img", attr).filter(|value| {
            value.contains(".jpg")
                || value.contains(".png")
                || value.contains(".jpeg")
                || value.contains(".webp")
        }) {
            return Some(clean_thumbnail(&url::join_url(BASE_URL, &value)));
        }
    }
    None
}

fn clean_thumbnail(input: &str) -> String {
    let Some((base, ext)) = input.rsplit_once('.') else {
        return input.to_string();
    };
    if let Some((clean, suffix)) = base.rsplit_once('-') {
        if suffix.chars().all(|ch| ch.is_ascii_digit() || ch == 'x') {
            return format!("{clean}.{ext}");
        }
    }
    input.to_string()
}

fn linked_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .filter(|chunk| {
            chunk.contains(marker)
                && (chunk.contains("genre") || chunk.contains("tag") || chunk.contains("category"))
        })
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .collect()
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains(">completed<") {
        ItemStatus::Completed
    } else if lower.contains(">ongoing<") {
        ItemStatus::Ongoing
    } else if lower.contains(">dropped<") || lower.contains(">discontinued<") {
        ItemStatus::Cancelled
    } else if lower.contains(">hiatus<") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn stream_format(input: &str) -> &'static str {
    match input
        .split('?')
        .next()
        .unwrap_or(input)
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "m3u8" => "hls",
        "mp4" => "mp4",
        "webm" => "webm",
        _ => "file",
    }
}

fn clean_title(title: &str) -> String {
    let mut out = title
        .split(": ")
        .last()
        .unwrap_or(title)
        .replace('\n', " ")
        .trim()
        .to_string();
    while let Some(start) = out.find('[') {
        if let Some(end) = out[start..].find(']') {
            out.replace_range(start..start + end + 1, " ");
        } else {
            break;
        }
    }
    let compact = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.ends_with(')') && compact.contains('(') {
        compact
            .rsplit_once('(')
            .map(|(head, _)| head.trim().to_string())
            .unwrap_or(compact)
    } else {
        compact
    }
}

fn slug_from_url(input: &str) -> Option<String> {
    if input.trim().is_empty() || input.contains("/page/") || input.contains("/search/") {
        return None;
    }
    if input.starts_with("http") && !input.contains("myreadingmanga.info") {
        return None;
    }
    let clean = input
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    clean
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty() && *value != "myreadingmanga.info")
        .map(ToString::to_string)
}

fn item_url(key: &str) -> String {
    if key.starts_with("http") {
        key.trim_end_matches('/').to_string()
    } else {
        format!("{BASE_URL}/{}/", key.trim_matches('/'))
    }
}

fn site_language(request: &Value) -> String {
    pref(request, "language").unwrap_or_else(|| "English".to_string())
}

fn latest_language(request: &Value) -> String {
    let latest = pref(request, "latest_language").unwrap_or_default();
    if latest.trim().is_empty() {
        site_language(request)
    } else {
        latest
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

fn pref_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get("filters").and_then(|filters| filters.get(key)))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
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
