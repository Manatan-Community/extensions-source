use base64::{Engine, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind,
    abi::{ExtensionError, ExtensionResult},
    export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const SOURCE: GoogleDriveIndex = GoogleDriveIndex;

struct GoogleDriveIndex;

impl VideoSource for GoogleDriveIndex {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let Some(base) = selected_domain(&request) else {
            return Ok(empty_page());
        };
        let response = post_index(&base, "", page(&request), None)?;
        Ok(parse_page(response, &base, &request, false))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if is_index_url(query) {
            return Ok(Paged {
                entries: vec![folder_item(query, &request)],
                has_next_page: false,
            });
        }
        let Some(base) = selected_domain(&request) else {
            return Ok(empty_page());
        };
        let response = post_index(&base, "", page(&request), Some(query))?;
        Ok(parse_page(response, &format!("{base}:search"), &request, true))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        let parsed = resolve_search_link(parse_link(&key))?;
        if parsed.link_type == "single" {
            return Ok(CatalogItem {
                key,
                title: file_name(&parsed.url).unwrap_or_else(|| "Video".to_string()),
                url: Some(parsed.url),
                language: Some("all".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Completed,
                initialized: true,
                ..CatalogItem::default()
            });
        }
        let mut item = folder_item(&parsed.url, &request);
        let mut token = String::new();
        let mut page_index = 0;
        while let Ok(response) = post_index(&parsed.url, &token, page_index + 1, None) {
            for file in response.data.files {
                if file.mime_type.starts_with("image/") && file.name.to_lowercase().starts_with("cover") {
                    item.cover = Some(join_url(&parsed.url, &file.name));
                }
                if file.name.eq_ignore_ascii_case("details.json") {
                    if let Ok(body) = client(&parsed.url)
                        .get(join_url(&parsed.url, &file.name))
                        .send_text()
                    {
                        if let Ok(details) = serde_json::from_str::<Details>(&body) {
                            if let Some(title) = details.title { item.title = title; }
                            item.authors = details.author.into_iter().collect();
                            item.artists = details.artist.into_iter().collect();
                            item.description = details.description;
                            item.tags = details.genre.unwrap_or_default();
                            item.status = details.status
                                .and_then(|value| value.parse::<u8>().ok())
                                .map(|value| if value == 2 { ItemStatus::Completed } else { ItemStatus::Unknown })
                                .unwrap_or(ItemStatus::Unknown);
                        }
                    }
                }
            }
            let Some(next) = response.next_page_token else {
                break;
            };
            token = next;
            page_index += 1;
        }
        item.initialized = true;
        Ok(item)
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        let parsed = resolve_search_link(parse_link(&key))?;
        if parsed.link_type == "single" {
            return Ok(vec![VideoEpisode {
                key: parsed.url.clone(),
                title: Some(if pref_bool(&request, "trim_episode_name", true) {
                    trim_info(&file_name(&parsed.url).unwrap_or_else(|| "Video".to_string()))
                } else {
                    file_name(&parsed.url).unwrap_or_else(|| "Video".to_string())
                }),
                episode_number: Some(1.0),
                url: Some(parsed.url),
                language: Some("all".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let mut out = Vec::new();
        let max_depth = parsed
            .fragment
            .as_deref()
            .and_then(|fragment| fragment.split(',').next())
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(2);
        traverse(&parsed.url, "", 0, max_depth, &request, &mut out)?;
        out.reverse();
        Ok(out)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let url = request_key(&request, "episode")
            .or_else(|| request.get("key").and_then(Value::as_str).map(ToString::to_string))
            .unwrap_or_default();
        let body = client(&url)
            .get(format!("{url}?a=view"))
            .browser_document()
            .send_text()
            .unwrap_or_default();
        let video_url = body
            .split("\"videodomain\":\"")
            .nth(1)
            .and_then(|tail| tail.split('"').next())
            .or_else(|| body.split("\"downloaddomain\":\"").nth(1).and_then(|tail| tail.split('"').next()))
            .filter(|domain| !domain.is_empty())
            .map(|domain| format!("{domain}{}", path_from_url(&url)))
            .unwrap_or_else(|| url.clone());
        let mut headers = Context::new();
        headers.insert("Referer".to_string(), url.clone());
        Ok(vec![VideoStream {
            url: video_url,
            name: Some("Video".to_string()),
            quality: Some("direct".to_string()),
            format: Some("external".to_string()),
            stream_kind: Some(VideoStreamKind::External),
            headers,
            initialized: true,
            ..VideoStream::default()
        }])
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Configured index".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: list.entries,
            has_more: list.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| parse_link(&key).url))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode"))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if is_index_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(folder_item(input, &request)),
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
        .with_header("Accept", "*/*")
        .with_referer(referer)
        .with_cookies_for(referer)
        .with_webview_challenge_fallback()
}

fn post_index(
    target: &str,
    page_token: &str,
    page: u64,
    query: Option<&str>,
) -> ExtensionResult<ResponseData> {
    let url = if query.filter(|value| !value.is_empty()).is_some() {
        format!("{}/search", target.trim_end_matches('/'))
    } else {
        target.to_string()
    };
    let body = if let Some(query) = query.filter(|value| !value.is_empty()) {
        format!(
            "q={}&page_token={page_token}&page_index={}",
            url::query_escape(query),
            page.saturating_sub(1)
        )
    } else {
        format!("password=&page_token={page_token}&page_index={}", page.saturating_sub(1))
    };
    let response = client(target)
        .post(&url)
        .xhr()
        .header("Content-Type", "application/x-www-form-urlencoded; charset=UTF-8")
        .origin(origin(target))
        .referer(referer(target))
        .body(body.into_bytes())
        .send_text()?;
    let decrypted = decrypt(&response).ok_or_else(|| error("Unable to decrypt index response"))?;
    serde_json::from_str(&decrypted).map_err(|_| error("Invalid index JSON"))
}

fn parse_page(response: ResponseData, base: &str, request: &Value, is_search: bool) -> Paged<CatalogItem> {
    let entries = response
        .data
        .files
        .into_iter()
        .filter_map(|file| {
            if file.mime_type.ends_with("folder") {
                let link = if is_search {
                    LinkData {
                        link_type: "search".to_string(),
                        url: serde_json::to_string(&IdUrl {
                            id: file.id,
                            url: base.trim_end_matches("search").trim_end_matches(':').to_string(),
                            referer: base.to_string(),
                            link_type: "multi".to_string(),
                        }).unwrap_or_default(),
                        info: None,
                        fragment: None,
                    }
                } else {
                    LinkData {
                        link_type: "multi".to_string(),
                        url: add_suffix(&join_url(base, &file.name), "/"),
                        info: None,
                        fragment: fragment(base),
                    }
                };
                Some(CatalogItem {
                    key: serde_json::to_string(&link).unwrap_or_default(),
                    title: if pref_bool(request, "trim_anime_name", false) { trim_info(&file.name) } else { file.name },
                    language: Some("all".to_string()),
                    content_rating: Some("safe".to_string()),
                    status: ItemStatus::Unknown,
                    url: Some(link.url),
                    ..CatalogItem::default()
                })
            } else if file.mime_type.starts_with("video/") && !(is_search && pref_bool(request, "ignore_folder", false)) {
                let file_url = join_url(base.trim_end_matches(":search"), &file.name);
                let size = file.size.and_then(|value| value.parse::<u64>().ok());
                let link = if is_search {
                    LinkData {
                        link_type: "search".to_string(),
                        url: serde_json::to_string(&IdUrl {
                            id: file.id,
                            url: base.trim_end_matches("search").trim_end_matches(':').to_string(),
                            referer: base.to_string(),
                            link_type: "single".to_string(),
                        }).unwrap_or_default(),
                        info: size.map(format_bytes),
                        fragment: fragment(base),
                    }
                } else {
                    LinkData {
                        link_type: "single".to_string(),
                        url: file_url.clone(),
                        info: size.map(format_bytes),
                        fragment: fragment(base),
                    }
                };
                Some(CatalogItem {
                    key: serde_json::to_string(&link).unwrap_or_default(),
                    title: if pref_bool(request, "trim_anime_name", false) { trim_info(&file.name) } else { file.name },
                    language: Some("all".to_string()),
                    content_rating: Some("safe".to_string()),
                    status: ItemStatus::Completed,
                    url: Some(file_url),
                    ..CatalogItem::default()
                })
            } else {
                None
            }
        })
        .collect();
    Paged {
        entries,
        has_next_page: response.next_page_token.is_some(),
    }
}

fn traverse(
    folder_url: &str,
    path: &str,
    depth: usize,
    max_depth: usize,
    request: &Value,
    out: &mut Vec<VideoEpisode>,
) -> ExtensionResult<()> {
    if depth >= max_depth {
        return Ok(());
    }
    let mut token = String::new();
    let mut page_index = 0;
    let mut counter = 1.0_f32;
    loop {
        let response = post_index(folder_url, &token, page_index + 1, None)?;
        for file in response.data.files {
            if file.mime_type.ends_with("folder") {
                let next_path = if path.is_empty() {
                    file.name.clone()
                } else {
                    format!("{path}/{}", file.name)
                };
                traverse(&add_suffix(&join_url(folder_url, &file.name), "/"), &next_path, depth + 1, max_depth, request, out)?;
            } else if file.mime_type.starts_with("video/") {
                let ep_url = join_url(folder_url, &file.name);
                let size = file.size.and_then(|value| value.parse::<u64>().ok());
                out.push(VideoEpisode {
                    key: ep_url.clone(),
                    title: Some(if pref_bool(request, "trim_episode_name", true) { trim_info(&file.name) } else { file.name }),
                    episode_number: Some(counter),
                    url: Some(ep_url),
                    release_group: Some(folder_label(path, size)),
                    size_bytes: size,
                    language: Some("all".to_string()),
                    ..VideoEpisode::default()
                });
                counter += 1.0;
            }
        }
        let Some(next) = response.next_page_token else {
            break;
        };
        token = next;
        page_index += 1;
    }
    Ok(())
}

fn selected_domain(request: &Value) -> Option<String> {
    pref(request, "domain_list")
        .and_then(|value| value.split(',').next().map(remove_name))
        .filter(|value| !value.trim().is_empty())
}

fn empty_page() -> Paged<CatalogItem> {
    Paged {
        entries: Vec::new(),
        has_next_page: false,
    }
}

fn folder_item(input: &str, _request: &Value) -> CatalogItem {
    let clean = remove_name(input);
    CatalogItem {
        key: serde_json::to_string(&LinkData {
            link_type: "multi".to_string(),
            url: clean.clone(),
            info: None,
            fragment: fragment(input),
        }).unwrap_or(clean.clone()),
        title: input
            .split(']')
            .next()
            .filter(|part| part.starts_with('['))
            .map(|part| part.trim_start_matches('[').to_string())
            .or_else(|| file_name(&clean))
            .unwrap_or_else(|| "Folder".to_string()),
        url: Some(clean),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_link(key: &str) -> LinkData {
    serde_json::from_str(key).unwrap_or_else(|_| LinkData {
        link_type: if key.ends_with('/') { "multi" } else { "single" }.to_string(),
        url: key.to_string(),
        info: None,
        fragment: None,
    })
}

fn resolve_search_link(parsed: LinkData) -> ExtensionResult<LinkData> {
    if parsed.link_type != "search" {
        return Ok(parsed);
    }
    let id_url: IdUrl = serde_json::from_str(&parsed.url).map_err(|_| error("Invalid search result link"))?;
    let slug = client(&id_url.referer)
        .post(format!("{}/id2path", id_url.url.trim_end_matches('/')))
        .xhr()
        .header("Content-Type", "application/x-www-form-urlencoded; charset=UTF-8")
        .origin(origin(&id_url.url))
        .referer(referer(&id_url.referer))
        .body(format!("id={}", url::query_escape(&id_url.id)).into_bytes())
        .send_text()?;
    Ok(LinkData {
        link_type: id_url.link_type,
        url: format!("{}{}", add_suffix(&id_url.url, "/"), slug.trim_start_matches('/')),
        info: parsed.info,
        fragment: parsed.fragment,
    })
}

fn decrypt(input: &str) -> Option<String> {
    if input.len() <= 44 {
        return None;
    }
    let reversed = input.chars().rev().collect::<String>();
    let payload = &reversed[24..reversed.len().saturating_sub(20)];
    STANDARD
        .decode(payload)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

fn trim_info(input: &str) -> String {
    let mut out = input.to_string();
    if out.starts_with('[') && out.contains(']') {
        out = out.split_once(']').map(|(_, tail)| tail.trim().to_string()).unwrap_or(out);
    }
    loop {
        let trimmed = out
            .trim_end()
            .trim_end_matches(".mkv")
            .trim_end_matches(".mp4")
            .trim_end()
            .to_string();
        let Some(start) = trimmed.rfind(['[', '(']) else {
            return out.trim().to_string();
        };
        let close = if trimmed.as_bytes()[start] == b'[' { ']' } else { ')' };
        if trimmed.ends_with(close) {
            out = trimmed[..start].trim().to_string();
        } else {
            return out.trim().to_string();
        }
    }
}

fn folder_label(path: &str, size: Option<u64>) -> String {
    let size = size.map(format_bytes).unwrap_or_default();
    if path.is_empty() {
        size
    } else if size.is_empty() {
        format!("/{path}")
    } else {
        format!("{size} - /{path}")
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.2} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.2} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} bytes")
    }
}

fn origin(input: &str) -> String {
    input
        .split("//")
        .nth(1)
        .and_then(|tail| tail.split('/').next())
        .map(|host| format!("https://{host}"))
        .unwrap_or_default()
}

fn referer(input: &str) -> String {
    url::query_escape(&format!("{}{}", origin(input), path_from_url(input)))
}

fn join_url(base: &str, path: &str) -> String {
    url::join_url(base.trim_end_matches(":search"), path)
}

fn add_suffix(input: &str, suffix: &str) -> String {
    if input.ends_with(suffix) { input.to_string() } else { format!("{input}{suffix}") }
}

fn path_from_url(input: &str) -> String {
    input
        .split("//")
        .nth(1)
        .and_then(|tail| tail.split_once('/').map(|(_, path)| format!("/{path}")))
        .unwrap_or_else(|| "/".to_string())
}

fn file_name(input: &str) -> Option<String> {
    input.trim_end_matches('/').rsplit('/').next().map(html::html_unescape)
}

fn fragment(input: &str) -> Option<String> {
    input.split('#').nth(1).map(ToString::to_string)
}

fn remove_name(input: &str) -> String {
    if input.starts_with('[') && input.contains(']') {
        input.split_once(']').map(|(_, tail)| tail.to_string()).unwrap_or_else(|| input.to_string())
    } else {
        input.to_string()
    }
}

fn is_index_url(input: &str) -> bool {
    input.starts_with("http://") || input.starts_with("https://")
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
    pref(request, key)
        .and_then(|value| value.parse::<bool>().ok())
        .unwrap_or(default)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn error(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.to_string(),
    }
}

#[derive(Debug, Deserialize)]
struct ResponseData {
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
    data: DataObject,
}

#[derive(Debug, Deserialize)]
struct DataObject {
    files: Vec<FileObject>,
}

#[derive(Debug, Deserialize)]
struct FileObject {
    #[serde(rename = "mimeType")]
    mime_type: String,
    id: String,
    name: String,
    size: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LinkData {
    #[serde(rename = "type")]
    link_type: String,
    url: String,
    #[serde(default)]
    info: Option<String>,
    #[serde(default)]
    fragment: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct IdUrl {
    id: String,
    url: String,
    referer: String,
    #[serde(rename = "type")]
    link_type: String,
}

#[derive(Debug, Deserialize)]
struct Details {
    title: Option<String>,
    author: Option<String>,
    artist: Option<String>,
    description: Option<String>,
    genre: Option<Vec<String>>,
    status: Option<String>,
}

export_video_source!(SOURCE);
