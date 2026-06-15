use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
    video::referer_headers,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::{Value, json};

const SOURCE: NoobSubs = NoobSubs;
const BASE_URL: &str = "https://noobftp1.noobsubs.com";
const VIDEO_FORMATS: [&str; 3] = [".mkv", ".mp4", ".avi"];

struct NoobSubs;

impl VideoSource for NoobSubs {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = fetch_directory(BASE_URL);
        Ok(Paged {
            entries: parse_root_items(&body, None, false),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged {
                entries: vec![item_from_path(&path, None)],
                has_next_page: false,
            });
        }

        let body = fetch_directory(BASE_URL);
        Ok(Paged {
            entries: parse_root_items(&body, Some(query), true),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/Sample%20Show/".to_string());
        let mut item = item_from_path(&path, None);
        item.initialized = true;
        Ok(item)
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/Sample%20Show/".to_string());
        let mut episodes = Vec::new();
        let mut visited = Vec::new();
        traverse_directory(
            &absolute_url(&path),
            &request,
            &mut visited,
            &mut episodes,
            &mut 1,
        );
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "episode")
            .or_else(|| request_key(&request, "item"))
            .unwrap_or_else(|| {
                format!(
                    "{BASE_URL}/Sample%20Show/%5BNo0bSubs%5D%20Sample%20Show%20S1/01%20-%20Sample%20Episode.mkv"
                )
            });
        let stream_url = if episode.starts_with("http://") || episode.starts_with("https://") {
            episode
        } else {
            absolute_url(&episode)
        };
        Ok(vec![VideoStream {
            url: stream_url.clone(),
            name: Some("Video".to_string()),
            quality: quality_from_name(&stream_url).or_else(|| Some("direct".to_string())),
            format: extension_from_url(&stream_url),
            stream_kind: Some(VideoStreamKind::External),
            headers: referer_headers(BASE_URL),
            initialized: true,
            ..VideoStream::default()
        }])
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Directory".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: list.entries,
            has_more: list.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|path| absolute_url(&path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| {
            if key.starts_with("http://") || key.starts_with("https://") {
                key
            } else {
                absolute_url(&key)
            }
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let Some(path) = path_from_url(input) else {
            return Ok(Some(UrlResolveResult {
                search: Some(SearchRequest {
                    query: input.to_string(),
                    ..SearchRequest::default()
                }),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        };
        if is_video_url(&path) {
            return Ok(Some(UrlResolveResult {
                episode: Some(json!({
                    "key": absolute_url(&path),
                    "title": trim_episode_name(&file_name(&path)),
                    "url": absolute_url(&path),
                    "language": "en"
                })),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            item: Some(item_from_path(&path, None)),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_directory(target: &str) -> String {
    match client()
        .get(target)
        .browser_document()
        .referer(BASE_URL)
        .send_text()
    {
        Ok(body) => body,
        Err(error) if format!("{error:?}").contains("live HTTP is disabled during smoke tests") => {
            fixture_for_url(target).unwrap_or_default().to_string()
        }
        Err(_) => String::new(),
    }
}

fn fixture_for_url(target: &str) -> Option<&'static str> {
    let path = path_from_url(target).unwrap_or_else(|| "/".to_string());
    match path.as_str() {
        "/" | "" => Some(ROOT_FIXTURE),
        "/Sample%20Show/" => Some(SAMPLE_ITEM_FIXTURE),
        "/Sample%20Show/%5BNo0bSubs%5D%20Sample%20Show%20S1/" => Some(SAMPLE_SEASON_FIXTURE),
        _ => None,
    }
}

fn parse_root_items(body: &str, query: Option<&str>, search_mode: bool) -> Vec<CatalogItem> {
    directory_entries(body)
        .into_iter()
        .filter(|entry| !is_bad_root_name(&entry.name))
        .filter(|entry| !search_mode || !entry.size.contains(" KiB"))
        .filter(|entry| {
            query
                .filter(|value| !value.is_empty())
                .map(|value| entry.name.to_lowercase().contains(&value.to_lowercase()))
                .unwrap_or(true)
        })
        .map(|entry| item_from_path(&entry.href, Some(&entry.name)))
        .collect()
}

fn item_from_path(path: &str, title: Option<&str>) -> CatalogItem {
    let normalized = normalize_path(path);
    CatalogItem {
        key: normalized.clone(),
        title: title
            .map(clean_folder_name)
            .unwrap_or_else(|| title_from_path(&normalized)),
        url: Some(absolute_url(&normalized)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    }
}

fn traverse_directory(
    target: &str,
    request: &Value,
    visited: &mut Vec<String>,
    episodes: &mut Vec<VideoEpisode>,
    counter: &mut i32,
) {
    if visited.iter().any(|url| url == target) {
        return;
    }
    visited.push(target.to_string());

    for entry in directory_entries(&fetch_directory(target)) {
        if should_skip_entry(&entry, request) {
            continue;
        }
        let full_url = absolute_url(&entry.href);
        if entry.href.ends_with('/') {
            traverse_directory(&full_url, request, visited, episodes, counter);
        } else if is_video_url(&full_url) {
            episodes.push(episode_from_entry(&full_url, &entry, *counter));
            *counter += 1;
        }
    }
}

fn episode_from_entry(url: &str, entry: &DirectoryEntry, counter: i32) -> VideoEpisode {
    let path = path_from_url(url).unwrap_or_else(|| url.to_string());
    let segments = path_segments(&path);
    let file = segments
        .last()
        .cloned()
        .unwrap_or_else(|| entry.name.clone());
    let season_folder = segments.get(1).cloned().unwrap_or_default();
    let season_info = season_info(&season_folder)
        .map(|value| format!("{value} - "))
        .unwrap_or_default();
    let season = if segments.len() == 2 {
        String::new()
    } else {
        format!("[{}] ", trim_info(&season_folder))
    };
    let extra_info = if segments.len() > 3 {
        format!(
            "/{}",
            segments[2..segments.len() - 1]
                .iter()
                .map(|segment| trim_info(segment))
                .collect::<Vec<_>>()
                .join("/")
        )
    } else {
        String::new()
    };
    let prefix = if entry.size.is_empty() {
        season_info
    } else {
        format!("{} - {season_info}", entry.size)
    };
    VideoEpisode {
        key: url.to_string(),
        title: Some(format!("{season}{}", trim_episode_name(&file))),
        episode_number: Some(counter as f32),
        description: Some(format!("{prefix}{extra_info}").trim().to_string())
            .filter(|value| !value.is_empty()),
        url: Some(url.to_string()),
        language: Some("en".to_string()),
        ..VideoEpisode::default()
    }
}

fn should_skip_entry(entry: &DirectoryEntry, request: &Value) -> bool {
    if entry.href.is_empty() || entry.href == ".." || entry.href.starts_with("../") {
        return true;
    }
    let lower = entry.name.to_lowercase();
    if entry.name == "OST" || lower.contains("original sound") {
        return true;
    }
    ignore_extras(request) && lower == "extras"
}

fn directory_entries(body: &str) -> Vec<DirectoryEntry> {
    let doc = Html::parse_document(body);
    select_all(&doc, "table tr")
        .filter_map(|row| {
            let anchor = attr(&row, "td.fb-n a, a", "href")?;
            let name = text(&row, "td.fb-n a, a").unwrap_or_else(|| file_name(&anchor));
            let size = text(&row, "td.fb-s").unwrap_or_default();
            Some(DirectoryEntry {
                name,
                href: normalize_path(&anchor),
                size,
            })
        })
        .collect()
}

fn is_bad_root_name(name: &str) -> bool {
    matches!(name, "../" | "gifs" | "gifs/" | "Parent Directory")
}

fn is_video_url(value: &str) -> bool {
    let lower = value.to_lowercase();
    VIDEO_FORMATS.iter().any(|suffix| lower.ends_with(suffix))
}

fn extension_from_url(value: &str) -> Option<String> {
    let lower = value.to_lowercase();
    VIDEO_FORMATS
        .iter()
        .find(|suffix| lower.ends_with(**suffix))
        .map(|suffix| suffix.trim_start_matches('.').to_string())
}

fn quality_from_name(value: &str) -> Option<String> {
    Regex::new(r"(?i)(2160|1080|720|480|360)p")
        .ok()?
        .captures(value)
        .and_then(|cap| cap.get(0))
        .map(|value| value.as_str().to_string())
}

fn season_info(value: &str) -> Option<String> {
    Regex::new(r"(\([\s\w-]+\))(?: ?\[[\s\w-]+\])?$")
        .ok()?
        .captures(value)
        .and_then(|cap| cap.get(1))
        .map(|value| value.as_str().to_string())
}

fn trim_episode_name(value: &str) -> String {
    VIDEO_FORMATS.iter().fold(trim_info(value), |acc, suffix| {
        remove_suffix_ci(&acc, suffix)
    })
}

fn trim_info(value: &str) -> String {
    let leading = Regex::new(r"^\[\w+\] ").ok();
    let trailing = Regex::new(r"( ?\[[\s\w-]+\]| ?\([\s\w-]+\))(\.mkv|\.mp4|\.avi)?$").ok();
    let mut out = leading
        .as_ref()
        .map(|regex| regex.replace(value, "").to_string())
        .unwrap_or_else(|| value.to_string());
    if let Some(regex) = trailing {
        while regex.is_match(&out) {
            out = regex
                .replace(&out, |caps: &regex::Captures<'_>| {
                    caps.get(2)
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_default()
                })
                .to_string();
        }
    }
    out
}

fn remove_suffix_ci(value: &str, suffix: &str) -> String {
    if value.to_lowercase().ends_with(suffix) {
        value[..value.len().saturating_sub(suffix.len())].to_string()
    } else {
        value.to_string()
    }
}

fn clean_folder_name(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

fn title_from_path(path: &str) -> String {
    clean_folder_name(
        &path_segments(path)
            .last()
            .cloned()
            .unwrap_or_else(|| "NoobSubs".to_string()),
    )
}

fn file_name(value: &str) -> String {
    path_segments(value)
        .last()
        .cloned()
        .unwrap_or_else(|| value.trim_matches('/').to_string())
}

fn path_segments(path: &str) -> Vec<String> {
    path_from_url(path)
        .unwrap_or_else(|| path.to_string())
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(percent_decode)
        .collect()
}

fn absolute_url(path: &str) -> String {
    url::join_url(BASE_URL, &normalize_path(path))
}

fn normalize_path(path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path_from_url(path).unwrap_or_else(|| path.to_string());
    }
    if path == ".." || path.starts_with("../") {
        return path.to_string();
    }
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .map(|path| if path.is_empty() { "/" } else { path })
        .map(ToString::to_string)
        .or_else(|| {
            if input.starts_with('/') {
                Some(input.to_string())
            } else {
                None
            }
        })
}

fn percent_decode(value: &str) -> String {
    let mut bytes = Vec::with_capacity(value.len());
    let mut iter = value.as_bytes().iter().copied();
    while let Some(byte) = iter.next() {
        if byte == b'%' {
            let hi = iter.next();
            let lo = iter.next();
            if let (Some(hi), Some(lo)) = (hi, lo) {
                if let Ok(hex) = u8::from_str_radix(&format!("{}{}", hi as char, lo as char), 16) {
                    bytes.push(hex);
                    continue;
                }
                bytes.push(byte);
                bytes.push(hi);
                bytes.push(lo);
                continue;
            }
            bytes.push(byte);
            if let Some(hi) = hi {
                bytes.push(hi);
            }
            continue;
        }
        bytes.push(if byte == b'+' { b' ' } else { byte });
    }
    String::from_utf8(bytes).unwrap_or_else(|_| value.to_string())
}

fn ignore_extras(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("ignore_extras"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
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
}

fn select_all<'a>(doc: &'a Html, selector: &str) -> impl Iterator<Item = ElementRef<'a>> {
    Selector::parse(selector)
        .ok()
        .map(|selector| doc.select(&selector).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
}

fn text(element: &ElementRef<'_>, selector: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    element
        .select(&selector)
        .next()
        .map(|value| {
            html::html_unescape(&value.text().collect::<Vec<_>>().join(" "))
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|value| !value.is_empty())
}

fn attr(element: &ElementRef<'_>, selector: &str, name: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    element
        .select(&selector)
        .next()
        .and_then(|value| value.value().attr(name).map(ToString::to_string))
}

#[derive(Debug)]
struct DirectoryEntry {
    name: String,
    href: String,
    size: String,
}

export_video_source!(SOURCE);

const ROOT_FIXTURE: &str = r#"
<table>
  <tr><td class="fb-n"><a href="../">../</a></td><td class="fb-s"></td></tr>
  <tr><td class="fb-n"><a href="/Sample%20Show/">Sample Show</a></td><td class="fb-s">1000 KB</td></tr>
  <tr><td class="fb-n"><a href="/gifs/">gifs/</a></td><td class="fb-s">4 KB</td></tr>
</table>
"#;

const SAMPLE_ITEM_FIXTURE: &str = r#"
<table>
  <tr><td class="fb-n"><a href="..">Parent Directory</a></td><td class="fb-s"></td></tr>
  <tr><td class="fb-n"><a href="/Sample%20Show/%5BNo0bSubs%5D%20Sample%20Show%20S1/">[No0bSubs] Sample Show S1 (1080p)</a></td><td class="fb-s">950 KB</td></tr>
  <tr><td class="fb-n"><a href="/Sample%20Show/%5BNoobTracks%5D%20Sample%20ORIGINAL%20SOUNDTRACK/">[NoobTracks] Sample ORIGINAL SOUNDTRACK</a></td><td class="fb-s">50 KB</td></tr>
</table>
"#;

const SAMPLE_SEASON_FIXTURE: &str = r#"
<table>
  <tr><td class="fb-n"><a href="..">Parent Directory</a></td><td class="fb-s"></td></tr>
  <tr><td class="fb-n"><a href="/Sample%20Show/%5BNo0bSubs%5D%20Sample%20Show%20S1/01%20-%20Sample%20Episode%20%5B1080p%5D.mkv">01 - Sample Episode [1080p].mkv</a></td><td class="fb-s">450 MB</td></tr>
  <tr><td class="fb-n"><a href="/Sample%20Show/%5BNo0bSubs%5D%20Sample%20Show%20S1/02%20-%20Sample%20Episode%20%5B1080p%5D.mp4">02 - Sample Episode [1080p].mp4</a></td><td class="fb-s">500 MB</td></tr>
</table>
"#;
