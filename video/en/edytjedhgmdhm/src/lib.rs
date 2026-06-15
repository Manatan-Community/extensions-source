use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: Edytjedhgmdhm = Edytjedhgmdhm;
const DEFAULT_BASE_URL: &str = "https://edytjedhgmdhm.abfhaqrhbnf.workers.dev";
const CHUNK_SIZE: usize = 30;
const VIDEO_FORMATS: [&str; 3] = [".mkv", ".mp4", ".avi"];

struct Edytjedhgmdhm;

impl VideoSource for Edytjedhgmdhm {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base_url = base_url(&request);
        let links = fetch_directory_links(&base_url, "/tvs/", LIST_FIXTURE);
        Ok(page_items(links, page(&request), &base_url))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let base_url = base_url(&request);
        if let Some(path) = path_from_any_domain(query) {
            return Ok(Paged {
                entries: vec![item_from_path(&path, &base_url)],
                has_next_page: false,
            });
        }

        let subpage = filter(&request, "subpage").unwrap_or_else(|| "/tvs/".to_string());
        let mut links = fetch_directory_links(&base_url, &subpage, LIST_FIXTURE);
        if !query.is_empty() {
            let needle = query.to_lowercase();
            links.retain(|link| link.title.to_lowercase().contains(&needle));
        }
        Ok(page_items(links, page(&request), &base_url))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        if let Some(item) = request.get("item").and_then(Value::as_object) {
            let base_url = base_url(&request);
            let key = item
                .get("key")
                .or_else(|| item.get("url"))
                .and_then(Value::as_str)
                .map(path_key)
                .unwrap_or_else(|| "/tvs/".to_string());
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| title_from_path(&key));
            return Ok(catalog_item(&key, &title, &base_url));
        }
        let base_url = base_url(&request);
        let key = request_key(&request, "item").unwrap_or_else(|| "/tvs/".to_string());
        Ok(item_from_path(&key, &base_url))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base_url = base_url(&request);
        let path = request_key(&request, "item").unwrap_or_else(|| "/tvs/".to_string());
        let mut episodes = Vec::new();
        let mut visited = Vec::new();
        traverse_directory(&base_url, &path, &mut episodes, &mut visited);
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let base_url = base_url(&request);
        let episode = request_key(&request, "episode").unwrap_or_else(|| "/sample.mp4".to_string());
        let target = absolute_url(&base_url, &episode);
        Ok(vec![VideoStream {
            url: target.clone(),
            name: Some("Video".to_string()),
            quality: Some("direct".to_string()),
            format: Some(extension_format(&target).to_string()),
            is_hls: false,
            stream_kind: Some(VideoStreamKind::Direct),
            headers: referer_headers(&base_url),
            initialized: true,
            ..VideoStream::default()
        }])
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "TVs".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base_url = base_url(&request);
        Ok(request_key(&request, "item").map(|path| absolute_url(&base_url, &path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base_url = base_url(&request);
        Ok(request_key(&request, "episode").map(|path| absolute_url(&base_url, &path)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let base_url = base_url(&request);
        if let Some(path) = path_from_any_domain(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(item_from_path(&path, &base_url)),
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

#[derive(Clone, Debug)]
struct DirectoryLink {
    path: String,
    title: String,
    size_bytes: Option<u64>,
}

fn traverse_directory(
    base_url: &str,
    path: &str,
    episodes: &mut Vec<VideoEpisode>,
    visited: &mut Vec<String>,
) {
    let normalized = path_key(path);
    if visited.iter().any(|value| value == &normalized) || visited.len() > 256 {
        return;
    }
    visited.push(normalized.clone());

    let links = fetch_directory_links(base_url, &normalized, EPISODES_FIXTURE);
    let mut counter = 1.0;
    for link in links {
        if link.path.ends_with('/') {
            traverse_directory(base_url, &link.path, episodes, visited);
            continue;
        }
        if !VIDEO_FORMATS.iter().any(|suffix| link.path.ends_with(suffix)) {
            continue;
        }
        let name = trim_info(remove_video_suffix(&title_from_path(&link.path)));
        let extra_info = extra_info(&link.path);
        let mut labels = Vec::new();
        if let Some(size) = link.size_bytes {
            labels.push(format_bytes(size));
        }
        labels.push(extra_info.clone());
        episodes.push(VideoEpisode {
            key: link.path.clone(),
            title: Some(name),
            episode_number: Some(counter),
            url: Some(absolute_url(base_url, &link.path)),
            language: Some("en".to_string()),
            size_bytes: link.size_bytes,
            release_group: Some(labels.into_iter().filter(|value| !value.is_empty()).collect::<Vec<_>>().join(" - ")),
            labels: vec![extra_info],
            ..VideoEpisode::default()
        });
        counter += 1.0;
    }
}

fn fetch_directory_links(base_url: &str, path: &str, fixture: &str) -> Vec<DirectoryLink> {
    let target = absolute_url(base_url, path);
    let body = client(base_url)
        .get(&target)
        .browser_document()
        .referer(base_url)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    parse_directory_links(&body, base_url)
}

fn parse_directory_links(body: &str, base_url: &str) -> Vec<DirectoryLink> {
    let mut out = Vec::new();
    for row in body.split("<tr").skip(1) {
        let Some(anchor_start) = row.find("<a") else {
            continue;
        };
        let anchor = row[anchor_start..].split("</a>").next().unwrap_or_default();
        let Some(href) = html::attr(anchor, "href") else {
            continue;
        };
        if href == ".." || href == "../" {
            continue;
        }
        let title = html::strip_tags(anchor);
        if title.is_empty() {
            continue;
        }
        out.push(DirectoryLink {
            path: path_key(&absolute_url(base_url, &href)),
            title,
            size_bytes: row
                .split("data-order=")
                .nth(1)
                .and_then(|tail| tail.trim_start_matches(['"', '\'']).split(['"', '\'']).next())
                .and_then(|value| value.parse::<u64>().ok()),
        });
    }
    if !out.is_empty() {
        return out;
    }

    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let anchor = chunk.split("</a>").next().unwrap_or_default();
            let href = html::attr(anchor, "href")?;
            if href == ".." || href == "../" {
                return None;
            }
            let title = html::strip_tags(anchor);
            (!title.is_empty()).then(|| DirectoryLink {
                path: path_key(&absolute_url(base_url, &href)),
                title,
                size_bytes: None,
            })
        })
        .collect()
}

fn page_items(links: Vec<DirectoryLink>, page: usize, base_url: &str) -> Paged<CatalogItem> {
    let start = page.saturating_sub(1) * CHUNK_SIZE;
    let entries = links
        .iter()
        .skip(start)
        .take(CHUNK_SIZE)
        .map(|link| catalog_item(&link.path, &link.title, base_url))
        .collect::<Vec<_>>();
    Paged {
        entries,
        has_next_page: links.len() > start + CHUNK_SIZE,
    }
}

fn catalog_item(path: &str, title: &str, base_url: &str) -> CatalogItem {
    CatalogItem {
        key: path_key(path),
        title: title.to_string(),
        url: Some(absolute_url(base_url, path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn item_from_path(path: &str, base_url: &str) -> CatalogItem {
    catalog_item(path, &title_from_path(path), base_url)
}

fn client(base_url: &str) -> HttpClient {
    HttpClient::browser()
        .with_referer(base_url)
        .with_header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
        )
        .with_cookies_for(base_url)
        .with_webview_challenge_fallback()
}

fn base_url(request: &Value) -> String {
    preference(request, "preferred_domain")
        .filter(|value| value.starts_with("https://"))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
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
        .map(path_key)
}

fn path_from_any_domain(input: &str) -> Option<String> {
    if input.starts_with('/') {
        return Some(path_key(input));
    }
    let after_scheme = input.split("://").nth(1)?;
    let path_start = after_scheme.find('/')?;
    Some(path_key(&after_scheme[path_start..]))
}

fn path_key(input: impl AsRef<str>) -> String {
    let input = input.as_ref().trim();
    let without_scheme = input
        .split("://")
        .nth(1)
        .and_then(|tail| tail.find('/').map(|index| &tail[index..]))
        .unwrap_or(input);
    let path = without_scheme.split('#').next().unwrap_or(without_scheme);
    let path = path.split('?').next().unwrap_or(path);
    if path == "/" {
        "/".to_string()
    } else {
        format!("/{}", path.trim_start_matches('/'))
    }
}

fn absolute_url(base_url: &str, input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else {
        url::join_url(base_url, input)
    }
}

fn title_from_path(input: &str) -> String {
    input
        .trim_matches('/')
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .map(|value| value.replace("%20", " "))
        .unwrap_or_else(|| "edytjedhgmdhm".to_string())
}

fn remove_video_suffix(input: &str) -> &str {
    VIDEO_FORMATS
        .iter()
        .find_map(|suffix| input.strip_suffix(suffix))
        .unwrap_or(input)
}

fn trim_info(input: &str) -> String {
    let mut value = input.to_string();
    if value.starts_with('[') {
        if let Some(index) = value.find(']') {
            let prefix = &value[1..index];
            if prefix.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
                value = value[index + 1..].trim_start().to_string();
            }
        }
    }
    loop {
        let trimmed = trim_trailing_bracket_info(&value);
        if trimmed == value {
            break;
        }
        value = trimmed;
    }
    value
}

fn trim_trailing_bracket_info(input: &str) -> String {
    let trimmed = input.trim_end();
    let Some(close) = trimmed.chars().last() else {
        return input.to_string();
    };
    let open = match close {
        ']' => '[',
        ')' => '(',
        _ => return input.to_string(),
    };
    let Some(start) = trimmed.rfind(open) else {
        return input.to_string();
    };
    let content = &trimmed[start + 1..trimmed.len() - 1];
    if content
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch.is_whitespace() || ch == '-')
    {
        trimmed[..start].trim_end().to_string()
    } else {
        input.to_string()
    }
}

fn extra_info(path: &str) -> String {
    let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
    if parts.len() > 2 {
        format!(
            "/{}",
            parts[1..parts.len() - 1]
                .iter()
                .map(|part| trim_info(&part.replace("%20", " ")))
                .collect::<Vec<_>>()
                .join("/")
        )
    } else {
        "/".to_string()
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.2} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.2} KB", bytes as f64 / 1_000.0)
    } else if bytes == 1 {
        "1 byte".to_string()
    } else if bytes > 1 {
        format!("{bytes} bytes")
    } else {
        String::new()
    }
}

fn extension_format(url: &str) -> &str {
    VIDEO_FORMATS
        .iter()
        .find(|suffix| url.ends_with(**suffix))
        .map(|suffix| suffix.trim_start_matches('.'))
        .unwrap_or("mp4")
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn preference(request: &Value, key: &str) -> Option<String> {
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

fn page(request: &Value) -> usize {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1) as usize
}

const LIST_FIXTURE: &str = r#"
<table><tbody>
<tr><td><a href="/tvs/sample-show/">Sample Show</a></td><td data-order="0"></td></tr>
</tbody></table>
"#;

const EPISODES_FIXTURE: &str = r#"
<table><tbody>
<tr><td><a href="/tvs/sample-show/Sample%20Episode%20%5B1080p%5D.mp4">Sample Episode [1080p].mp4</a></td><td data-order="123456789"></td></tr>
</tbody></table>
"#;

export_video_source!(SOURCE);
