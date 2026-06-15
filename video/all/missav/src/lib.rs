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

const SOURCE: MissAv = MissAv;
const DEFAULT_URL: &str = "https://missav.live";

struct MissAv;

impl VideoSource for MissAv {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let path = if listing == "latest" {
            "en/new"
        } else {
            "en/today-hot"
        };
        let body = client(&base)
            .get(format!("{base}/{path}?page={}", page(&request)))
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_cards(&body, &base))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some((slug, found_base)) = slug_from_url(query) {
            let base = found_base.unwrap_or(base);
            return Ok(Paged {
                entries: vec![fetch_details(&slug, &base, &request)],
                has_next_page: false,
            });
        }
        let mut target = if !query.is_empty() {
            format!(
                "{base}/en/search/{}?page={}",
                url::query_escape(query),
                page(&request)
            )
        } else if let Some(genre) = filter(&request, "genre").filter(|value| !value.is_empty()) {
            format!("{base}/{}?page={}", genre.trim_matches('/'), page(&request))
        } else {
            format!("{base}/en/new?page={}", page(&request))
        };
        if let Some(sort) = filter(&request, "sort").filter(|value| !value.is_empty()) {
            target.push_str("&sort=");
            target.push_str(&url::query_escape(&sort));
        }
        let body = client(&base)
            .get(target)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_cards(&body, &base))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = request_key(&request, "item").unwrap_or_default();
        Ok(fetch_details(&key, &base, &request))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base = base_url(&request);
        let key = normalize_key(&request_key(&request, "item").unwrap_or_default()).0;
        Ok(vec![VideoEpisode {
            key: key.clone(),
            title: Some("Episode".to_string()),
            episode_number: Some(1.0),
            url: Some(item_url(&key, &base)),
            language: Some("all".to_string()),
            ..VideoEpisode::default()
        }])
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let base = base_url(&request);
        let key = normalize_key(&request_key(&request, "episode").unwrap_or_default()).0;
        let target = item_url(&key, &base);
        let body = client(&base)
            .get(&target)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        let Some(master) = find_hls(&body) else {
            return Ok(Vec::new());
        };
        let mut headers = Context::new();
        headers.insert("Referer".to_string(), format!("{base}/"));
        Ok(vec![VideoStream {
            url: master,
            name: Some("Default".to_string()),
            quality: Some(
                pref(&request, "preferred_quality").unwrap_or_else(|| "auto".to_string()),
            ),
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
                title: "Today Hot".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "New".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "item").map(|key| item_url(&normalize_key(&key).0, &base)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "episode").map(|key| item_url(&normalize_key(&key).0, &base)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some((slug, found_base)) = slug_from_url(input) {
            let base = found_base.unwrap_or_else(|| base_url(&request));
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&slug, &base, &request)),
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
        .with_origin(base)
        .with_referer(format!("{base}/"))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn parse_cards(body: &str, base: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("div class=\"thumbnail")
            .skip(1)
            .filter_map(|block| parse_card(block, base))
            .collect(),
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn parse_card(block: &str, base: &str) -> Option<CatalogItem> {
    let href = block
        .split("<a")
        .find(|chunk| chunk.contains("text-secondary") || chunk.contains("/en/"))
        .and_then(|chunk| html::attr(chunk, "href"))?;
    let (key, found_base) = slug_from_url(&href)?;
    let title = html::text_between(block, "text-secondary", "</a>")
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .or_else(|| html::attr_after(block, "<img", "alt"))
        .unwrap_or_else(|| key.clone());
    let base = found_base.as_deref().unwrap_or(base);
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(block, "<img", "data-src")
            .or_else(|| html::attr_after(block, "<img", "src"))
            .map(|value| url::join_url(base, &value)),
        url: Some(item_url(&key, base)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn fetch_details(key: &str, base: &str, _request: &Value) -> CatalogItem {
    let (key, found_base) = normalize_key(key);
    let base = found_base.unwrap_or_else(|| base.to_string());
    let body = client(&base)
        .get(item_url(&key, &base))
        .browser_document()
        .send_text()
        .unwrap_or_default();
    let title = html::text_between(&body, "<h1", "</h1>")
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| key.clone());
    let description = body
        .split("div class=\"mb-1")
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, ">", "</div>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty());
    let jp_title = body
        .split("span")
        .find(|chunk| chunk.to_ascii_lowercase().contains("title"))
        .and_then(|chunk| {
            html::strip_tags(chunk)
                .split(':')
                .last()
                .map(str::trim)
                .map(ToString::to_string)
        });
    CatalogItem {
        key: key.clone(),
        title,
        alternate_titles: jp_title
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect(),
        cover: html::attr_after(&body, "video", "data-poster")
            .or_else(|| html::attr_after(&body, "<meta property=\"og:image\"", "content"))
            .map(|value| url::join_url(&base, &value)),
        url: Some(item_url(&key, &base)),
        description,
        tags: linked_values(&body, "/genres/"),
        authors: {
            let mut values = linked_values(&body, "/directors/");
            values.extend(linked_values(&body, "/makers/"));
            values
        },
        artists: linked_values(&body, "/actresses/"),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn linked_values(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .collect()
}

fn find_hls(body: &str) -> Option<String> {
    for marker in ["source=\"", "source = \"", "source:'", "source: '"] {
        if let Some(value) = body.split(marker).nth(1).and_then(|tail| {
            let quote = if marker.ends_with('\'') { '\'' } else { '"' };
            tail.split(quote).next()
        }) {
            if value.contains(".m3u8") {
                return Some(value.to_string());
            }
        }
    }
    if let Some(script) = body
        .split("<script")
        .find(|chunk| chunk.contains("function(p,a,c,k,e,d)"))
    {
        let unpacked = unpack_packer(script)?;
        for marker in ["source=\"", "source = \"", "source:'", "source: '"] {
            if let Some(value) = unpacked.split(marker).nth(1).and_then(|tail| {
                let quote = if marker.ends_with('\'') { '\'' } else { '"' };
                tail.split(quote).next()
            }) {
                if value.contains(".m3u8") {
                    return Some(value.to_string());
                }
            }
        }
    }
    body.split(['"', '\''])
        .find(|part| part.contains(".m3u8"))
        .map(ToString::to_string)
}

fn unpack_packer(input: &str) -> Option<String> {
    let start = input.find("}(").map(|index| index + 2)?;
    let args = &input[start
        ..input[start..]
            .rfind("))")
            .map(|end| start + end)
            .unwrap_or(input.len())];
    let parts = split_js_args(args);
    if parts.len() < 4 {
        return None;
    }
    let mut payload = unquote(&parts[0])?;
    let radix = parts[1].trim().parse::<u32>().ok()?;
    let words_src = parts[3].split(".split").next().unwrap_or(&parts[3]);
    let words = unquote(words_src)?
        .split('|')
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let mut out = String::new();
    let mut token = String::new();
    for ch in payload.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            token.push(ch);
        } else {
            push_decoded(&mut out, &mut token, radix, &words);
            out.push(ch);
        }
    }
    push_decoded(&mut out, &mut token, radix, &words);
    payload.clear();
    Some(out)
}

fn split_js_args(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut depth = 0i32;
    for ch in input.chars() {
        if let Some(q) = quote {
            current.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                current.push(ch);
            }
            '(' | '[' | '{' => {
                depth += 1;
                current.push(ch);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                out.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

fn unquote(input: &str) -> Option<String> {
    let input = input.trim();
    let quote = input.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let end = input.rfind(quote)?;
    Some(
        input[1..end]
            .replace("\\'", "'")
            .replace("\\\"", "\"")
            .replace("\\/", "/")
            .replace("\\\\", "\\"),
    )
}

fn push_decoded(out: &mut String, token: &mut String, radix: u32, words: &[String]) {
    if token.is_empty() {
        return;
    }
    if let Ok(index) = u32::from_str_radix(token, radix) {
        if let Some(word) = words.get(index as usize).filter(|word| !word.is_empty()) {
            out.push_str(word);
            token.clear();
            return;
        }
    }
    out.push_str(token);
    token.clear();
}

fn base_url(request: &Value) -> String {
    pref(request, "preferred_domain")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn slug_from_url(input: &str) -> Option<(String, Option<String>)> {
    if input.trim().is_empty() {
        return None;
    }
    let found_base = if input.contains("missav.live") {
        Some("https://missav.live".to_string())
    } else if input.contains("missav.ai") {
        Some("https://missav.ai".to_string())
    } else if input.contains("missav.ws") {
        Some("https://missav.ws".to_string())
    } else if input.starts_with("http") {
        return None;
    } else {
        None
    };
    let clean = input
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    clean
        .split("/en/")
        .nth(1)
        .or_else(|| clean.rsplit('/').next())
        .filter(|value| !value.is_empty() && !matches!(*value, "new" | "today-hot" | "search"))
        .map(|slug| (slug.to_string(), found_base))
}

fn normalize_key(key: &str) -> (String, Option<String>) {
    slug_from_url(key).unwrap_or_else(|| (key.trim_matches('/').to_string(), None))
}

fn item_url(key: &str, base: &str) -> String {
    if key.starts_with("http") {
        key.to_string()
    } else {
        format!("{base}/en/{}", key.trim_matches('/'))
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
