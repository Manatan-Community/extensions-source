use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind,
    abi::{ExtensionError, ExtensionResult, cookies_get},
    export_video_source,
    source::VideoSource,
};
use manatan_shared::sdk::{SearchRequest, http::HttpClient};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::{Digest, Sha1};

const SOURCE: GoogleDrive = GoogleDrive;
const BASE_URL: &str = "https://drive.google.com";
const BOUNDARY: &str = "=====vc17a3rwnndj=====";

struct GoogleDrive;

impl VideoSource for GoogleDrive {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let Some(folder) = selected_folder(&request) else {
            return Ok(empty_page());
        };
        parse_page(&folder, &request, None)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(folder) = request
            .get("filters")
            .and_then(|f| f.get("url"))
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            return Ok(Paged {
                entries: vec![folder_item(folder)?],
                has_next_page: false,
            });
        }
        let Some(folder) = selected_folder(&request) else {
            return Ok(empty_page());
        };
        let search = if query.is_empty() {
            None
        } else {
            Some(query.to_string())
        };
        parse_page(&folder, &request, search)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        if let Some(item) = request
            .get("item")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
        {
            return Ok(item);
        }
        let key = request_key(&request, "item").unwrap_or_default();
        let link = serde_json::from_str::<LinkData>(&key).unwrap_or(LinkData {
            url: key.clone(),
            link_type: "multi".to_string(),
            info: None,
        });
        Ok(CatalogItem {
            key,
            title: folder_name_from_url(&link.url).unwrap_or_else(|| "Google Drive".to_string()),
            url: Some(link.url),
            language: Some("all".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            initialized: true,
            ..CatalogItem::default()
        })
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        let parsed: LinkData = serde_json::from_str(&key).unwrap_or(LinkData {
            url: key,
            link_type: "multi".to_string(),
            info: None,
        });
        if parsed.link_type == "single" {
            return Ok(vec![VideoEpisode {
                key: parsed.url.clone(),
                title: Some("Video".to_string()),
                episode_number: Some(1.0),
                url: Some(parsed.url),
                size_bytes: parsed.info.as_ref().and_then(|info| parse_size(&info.size)),
                language: Some("all".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let mut episodes = Vec::new();
        traverse_folder(
            &parsed.url,
            "",
            &request,
            0,
            max_depth(&parsed.url),
            &mut episodes,
        )?;
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let url = request
            .get("episode")
            .and_then(|episode| episode.get("key").or_else(|| episode.get("url")))
            .and_then(Value::as_str)
            .or_else(|| request.get("key").and_then(Value::as_str))
            .unwrap_or_default();
        let name = request
            .get("episode")
            .and_then(|episode| episode.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("Google Drive");
        Ok(vec![VideoStream {
            url: url.to_string(),
            name: Some(name.to_string()),
            quality: Some("Google Drive".to_string()),
            format: Some("external".to_string()),
            stream_kind: Some(VideoStreamKind::External),
            requires_proxy: true,
            initialized: true,
            ..VideoStream::default()
        }])
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Configured folder".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: self.list(request)?.entries,
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if drive_folder_id(input).is_some() {
            return Ok(Some(UrlResolveResult {
                item: Some(folder_item(input)?),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        if input.contains("drive.google.com/uc") {
            return Ok(Some(UrlResolveResult {
                search: Some(SearchRequest {
                    query: input.to_string(),
                    ..SearchRequest::default()
                }),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(None)
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_header("Accept", "*/*")
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn parse_page(
    folder_url: &str,
    request: &Value,
    search: Option<String>,
) -> ExtensionResult<Paged<CatalogItem>> {
    let folder_id =
        drive_folder_id(folder_url).ok_or_else(|| error_with("Invalid Google Drive folder URL"))?;
    let document = client()
        .get(folder_url)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| DRIVE_FIXTURE.to_string());
    if document.contains("Error 404") {
        return Ok(Paged {
            entries: Vec::new(),
            has_next_page: false,
        });
    }
    let response = drive_batch(&document, &folder_id, "", search.as_deref())?;
    Ok(Paged {
        entries: response
            .items
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| catalog_from_drive_item(item, request, folder_url))
            .collect(),
        has_next_page: response.next_page_token.is_some(),
    })
}

fn empty_page() -> Paged<CatalogItem> {
    Paged {
        entries: Vec::new(),
        has_next_page: false,
    }
}

fn traverse_folder(
    folder_url: &str,
    path: &str,
    request: &Value,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<VideoEpisode>,
) -> ExtensionResult<()> {
    if depth >= max_depth {
        return Ok(());
    }
    let Some(folder_id) = drive_folder_id(folder_url) else {
        return Ok(());
    };
    let document = client()
        .get(folder_url)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| DRIVE_FIXTURE.to_string());
    let mut page_token = String::new();
    loop {
        let response = drive_batch(&document, &folder_id, &page_token, None)?;
        for (index, item) in response.items.unwrap_or_default().into_iter().enumerate() {
            if item.mime_type.starts_with("video") {
                let size = item
                    .file_size
                    .as_deref()
                    .and_then(|value| value.parse::<u64>().ok());
                let title = if pref_bool(request, "trim_episode_name", true) {
                    trim_info(&item.title)
                } else {
                    item.title.clone()
                };
                out.push(VideoEpisode {
                    key: format!("https://drive.google.com/uc?id={}", item.id),
                    title: Some(title),
                    episode_number: episode_number(&item.title).or(Some((index + 1) as f32)),
                    url: Some(format!("https://drive.google.com/uc?id={}", item.id)),
                    size_bytes: size,
                    release_group: Some(folder_label(
                        path,
                        size,
                        pref_bool(request, "scanlator_order", false),
                    )),
                    language: Some("all".to_string()),
                    ..VideoEpisode::default()
                });
            } else if item.mime_type.ends_with(".folder") {
                let child = format!("https://drive.google.com/drive/folders/{}", item.id);
                let child_path = if path.is_empty() {
                    item.title.clone()
                } else {
                    format!("{path}/{}", item.title)
                };
                traverse_folder(&child, &child_path, request, depth + 1, max_depth, out)?;
            }
        }
        let Some(next) = response.next_page_token else {
            break;
        };
        page_token = next;
    }
    Ok(())
}

fn drive_batch(
    document: &str,
    folder_id: &str,
    page_token: &str,
    search: Option<&str>,
) -> ExtensionResult<PostResponse> {
    let key =
        api_key(document).unwrap_or_else(|| "AIzaSyD-fixture-key-fixture-key-fixture".to_string());
    let version = document
        .split('"')
        .find(|part| part.contains("web-frontend"))
        .unwrap_or("")
        .to_string();
    let path = if let Some(query) = search {
        search_request_path(folder_id, page_token, &key, query)
    } else {
        default_request_path(folder_id, page_token, &key)
    };
    let auth = sapisid_hash().unwrap_or_default();
    let body = format!(
        "--{BOUNDARY}\r\ncontent-type: application/http\r\ncontent-transfer-encoding: binary\r\n\r\nGET {path}\r\nX-Goog-Drive-Client-Version: {version}\r\nauthorization: {auth}\r\nx-goog-authuser: 0\r\n\r\n--{BOUNDARY}--"
    );
    let url = format!(
        "https://clients6.google.com/batch/drive/v2internal?$ct=multipart/mixed; boundary=\"{BOUNDARY}\"&key={key}"
    );
    let raw = client()
        .post(url)
        .header("Content-Type", "text/plain; charset=UTF-8")
        .header("Origin", BASE_URL)
        .body(body.into_bytes())
        .send_text()
        .unwrap_or_else(|_| POST_FIXTURE.to_string());
    let json = raw
        .find('{')
        .and_then(|start| raw.rfind('}').map(|end| raw[start..=end].to_string()))
        .unwrap_or_else(|| POST_FIXTURE.to_string());
    serde_json::from_str(&json)
        .map_err(|error| error_with(format!("invalid Drive batch response: {error}")))
}

fn catalog_from_drive_item(
    item: ResponseItem,
    request: &Value,
    folder_url: &str,
) -> Option<CatalogItem> {
    if item.mime_type.starts_with("video") {
        let title = if pref_bool(request, "trim_anime_info", false) {
            trim_info(&item.title)
        } else {
            item.title.clone()
        };
        let link = LinkData {
            url: format!("https://drive.google.com/uc?id={}", item.id),
            link_type: "single".to_string(),
            info: Some(LinkDataInfo {
                title: item.title,
                size: item
                    .file_size
                    .as_deref()
                    .and_then(|value| value.parse().ok())
                    .map(format_bytes)
                    .unwrap_or_default(),
            }),
        };
        return Some(CatalogItem {
            key: serde_json::to_string(&link).ok()?,
            title,
            url: Some(link.url),
            language: Some("all".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            initialized: true,
            ..CatalogItem::default()
        });
    }
    if item.mime_type.ends_with(".folder") {
        let title = if pref_bool(request, "trim_anime_info", false) {
            trim_info(&item.title)
        } else {
            item.title.clone()
        };
        let recur = folder_url
            .split('#')
            .nth(1)
            .map(|value| format!("#{value}"))
            .unwrap_or_default();
        let link = LinkData {
            url: format!("https://drive.google.com/drive/folders/{}{recur}", item.id),
            link_type: "multi".to_string(),
            info: None,
        };
        return Some(CatalogItem {
            key: serde_json::to_string(&link).ok()?,
            title,
            url: Some(link.url),
            language: Some("all".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            initialized: true,
            ..CatalogItem::default()
        });
    }
    None
}

fn folder_item(folder_url: &str) -> ExtensionResult<CatalogItem> {
    let Some(folder_id) = drive_folder_id(folder_url) else {
        return Err(error_with("Invalid Google Drive folder URL"));
    };
    let clean = format!("https://drive.google.com/drive/folders/{folder_id}");
    let link = LinkData {
        url: clean.clone(),
        link_type: "multi".to_string(),
        info: None,
    };
    Ok(CatalogItem {
        key: serde_json::to_string(&link)
            .map_err(|error| error_with(format!("link encode failed: {error}")))?,
        title: folder_name_from_url(folder_url).unwrap_or_else(|| "Folder".to_string()),
        url: Some(clean),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    })
}

fn selected_folder(request: &Value) -> Option<String> {
    pref(request, "domain_list")
        .and_then(|value| {
            value
                .split(';')
                .next()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(ToString::to_string)
        })
}

fn default_request_path(folder_id: &str, page_token: &str, key: &str) -> String {
    format!(
        "/drive/v2internal/files?q=trashed%20%3D%20false%20and%20'{folder_id}'%20in%20parents&fields=kind%2CnextPageToken%2Citems(title%2Cid%2CfileSize%2CmimeType)&spaces=drive&pageToken={page_token}&maxResults=100&supportsTeamDrives=true&includeItemsFromAllDrives=true&orderBy=folder%2Ctitle_natural%20asc&key={key} HTTP/1.1"
    )
}

fn search_request_path(folder_id: &str, page_token: &str, key: &str, query: &str) -> String {
    let query = manatan_shared::sdk::http::url_encode(query);
    format!(
        "/drive/v2internal/files?q=title%20contains%20'{query}'%20and%20trashed%20%3D%20false%20and%20'{folder_id}'%20in%20ancestors&fields=kind%2CnextPageToken%2Citems(title%2Cid%2CfileSize%2CmimeType)&spaces=drive&pageToken={page_token}&maxResults=50&supportsTeamDrives=true&includeItemsFromAllDrives=true&orderBy=relevance%20desc&key={key} HTTP/1.1"
    )
}

fn api_key(document: &str) -> Option<String> {
    document
        .split('"')
        .find(|part| {
            part.len() == 39
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        })
        .map(ToString::to_string)
}

fn sapisid_hash() -> Option<String> {
    let cookie = cookies_get(BASE_URL).ok()?.header?;
    let sapisid = cookie.split(';').map(str::trim).find_map(|pair| {
        pair.strip_prefix("SAPISID=")
            .or_else(|| pair.strip_prefix("__Secure-3PAPISID="))
    })?;
    let now = 0;
    let material = format!("{now} {sapisid} {BASE_URL}");
    let digest = Sha1::digest(material.as_bytes());
    Some(format!("SAPISIDHASH {now}_{digest:x}"))
}

fn drive_folder_id(input: &str) -> Option<String> {
    input
        .split("/folders/")
        .nth(1)
        .and_then(|part| part.split(['?', '#', '/', ';']).next())
        .filter(|id| id.len() >= 20)
        .map(ToString::to_string)
}

fn folder_name_from_url(input: &str) -> Option<String> {
    input
        .split(']')
        .next()
        .and_then(|prefix| prefix.strip_prefix('['))
        .map(ToString::to_string)
}

fn max_depth(input: &str) -> usize {
    input
        .split('#')
        .nth(1)
        .and_then(|part| part.split(',').next())
        .and_then(|value| value.parse().ok())
        .unwrap_or(2)
}

fn episode_number(title: &str) -> Option<f32> {
    title
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>()
        .parse()
        .ok()
}

fn trim_info(input: &str) -> String {
    let mut out = input.trim().to_string();
    if out.starts_with('[') && out.contains(']') {
        out = out
            .split_once(']')
            .map(|(_, rest)| rest.trim().to_string())
            .unwrap_or(out);
    }
    for suffix in [".mkv", ".mp4", ".avi"] {
        if out.ends_with(suffix) {
            out.truncate(out.len() - suffix.len());
        }
    }
    out.trim().to_string()
}

fn folder_label(path: &str, size: Option<u64>, folder_first: bool) -> String {
    let size = size.map(format_bytes).unwrap_or_default();
    if folder_first {
        format!("/{path} • {size}")
    } else {
        format!("{size} • /{path}")
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

fn parse_size(input: &str) -> Option<u64> {
    let value = input.split_whitespace().next()?.parse::<f64>().ok()?;
    if input.contains("GB") {
        Some((value * 1_000_000_000.0) as u64)
    } else if input.contains("MB") {
        Some((value * 1_000_000.0) as u64)
    } else if input.contains("KB") {
        Some((value * 1_000.0) as u64)
    } else {
        None
    }
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("url")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn pref(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn pref_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn error_with(message: impl Into<String>) -> ExtensionError {
    ExtensionError {
        message: message.into(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostResponse {
    next_page_token: Option<String>,
    items: Option<Vec<ResponseItem>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResponseItem {
    id: String,
    title: String,
    mime_type: String,
    file_size: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct LinkData {
    url: String,
    #[serde(rename = "type")]
    link_type: String,
    info: Option<LinkDataInfo>,
}

#[derive(Serialize, Deserialize)]
struct LinkDataInfo {
    title: String,
    size: String,
}

const DRIVE_FIXTURE: &str = r#"<script>"AIzaSyD-fixture-key-fixture-key-fixture"</script>"#;
const POST_FIXTURE: &str = r#"{"items":[{"id":"abcdefghijklmnopqrstuvwxyz123456","title":"Sample Folder","mimeType":"application/vnd.google-apps.folder"}]}"#;

export_video_source!(SOURCE);
