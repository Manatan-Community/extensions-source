use base64::{Engine, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, TorrentInfo, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind,
    abi::{ExtensionError, ExtensionResult},
    export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;
use sha1::{Digest, Sha1};

const SOURCE: PTorrent = PTorrent;
const BASE_URL: &str = "https://www.ptorrents.com";
const TRACKERS_URL: &str = "https://raw.githubusercontent.com/ngosang/trackerslist/refs/heads/master/trackers_all_http.txt";

struct PTorrent;

impl VideoSource for PTorrent {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = client()
            .get(format!("{BASE_URL}/page/{}", page(&request)))
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_list(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = query.strip_prefix("id:") {
            return Ok(Paged {
                entries: vec![fetch_details(id)],
                has_next_page: false,
            });
        }
        if let Some(id) = id_from_url(query) {
            return self.search(with_query(&request, &format!("id:{id}")));
        }
        let target = if query.is_empty() {
            let cat = filter(&request, "category").unwrap_or_else(|| "0".to_string());
            format!("{BASE_URL}/catalog/{cat}/page/{}", page(&request))
        } else {
            format!(
                "{BASE_URL}/s.php?search={}&page={}",
                url::query_escape(query).replace("%20", "+"),
                page(&request)
            )
        };
        let body = client()
            .get(target)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_list(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        Ok(fetch_details(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        let page_url = item_url(&key);
        let body = client()
            .get(&page_url)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        let download_path = html::attr_after(&body, "download-container", "href")
            .or_else(|| html::attr_after(&body, "download-button", "href"))
            .ok_or_else(|| error("No torrent download page found"))?;
        let download_page_url = url::join_url(BASE_URL, &download_path);
        let download_body = client()
            .get(&download_page_url)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        let torrent_url = html::attr_after(&download_body, "download-button", "href")
            .map(|href| url::join_url(BASE_URL, &href))
            .ok_or_else(|| error("No torrent file found"))?;
        let mut torrent = parse_torrent(&fetch_bytes(&torrent_url)?)?;
        torrent.trackers.extend(fetch_trackers());
        torrent.trackers.sort();
        torrent.trackers.dedup();
        Ok(torrent_episodes(&torrent, &request))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        Ok(vec![magnet_stream(&request)])
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Home".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: page.entries,
            has_more: page.has_next_page,
            ..HomeSection::default()
        }])
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
        if let Some(id) = id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&id)),
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

fn parse_list(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("image-wrapper")
            .skip(1)
            .filter_map(parse_card)
            .collect(),
        has_next_page: body.contains("pagination") && body.contains("Next"),
    }
}

fn parse_card(block: &str) -> Option<CatalogItem> {
    let href = html::attr_after(block, "overlay", "href")
        .or_else(|| html::attr_after(block, "<a", "href"))?;
    let key = id_from_url(&href)?;
    let title = html::text_between(block, "overlay", "</a>")
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .or_else(|| html::attr_after(block, "<img", "alt"))
        .unwrap_or_else(|| key.clone());
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(block, "<img", "src"),
        url: Some(item_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(key: &str) -> CatalogItem {
    let key = id_from_url(key).unwrap_or_else(|| key.to_string());
    let body = client()
        .get(item_url(&key))
        .browser_document()
        .send_text()
        .unwrap_or_default();
    let title = html::attr_after(&body, "<meta property=\"og:title\"", "content")
        .or_else(|| html::text_between(&body, "<h1", "</h1>").map(|text| html::strip_tags(&text)))
        .unwrap_or_else(|| key.clone());
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(&body, "<meta property=\"og:image\"", "content")
            .or_else(|| html::attr_after(&body, "<img", "src")),
        description: html::text_between(&body, "article-content", "</div>")
            .map(|text| html::strip_tags(&text))
            .filter(|text| !text.is_empty()),
        url: Some(item_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_bytes(target: &str) -> ExtensionResult<Vec<u8>> {
    let response = client().get(target).referer(BASE_URL).send()?;
    response
        .body_base64
        .and_then(|value| STANDARD.decode(value).ok())
        .or_else(|| response.text.map(|text| text.into_bytes()))
        .ok_or_else(|| error("Empty torrent response"))
}

fn fetch_trackers() -> Vec<String> {
    HttpClient::browser()
        .get(TRACKERS_URL)
        .send_text()
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn torrent_episodes(torrent: &ParsedTorrent, request: &Value) -> Vec<VideoEpisode> {
    let filename_only = pref_bool(request, "filename", false);
    let mut episodes = torrent
        .files
        .iter()
        .enumerate()
        .filter(|(_, file)| is_video_file(&file.path))
        .map(|(index, file)| {
            let title = if filename_only {
                file.path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&file.path)
                    .to_string()
            } else {
                file.path
                    .replace('[', "(")
                    .replace(']', ")")
                    .replace('/', " / ")
            };
            let magnet = torrent.magnet_for(file, index as u32);
            VideoEpisode {
                key: magnet.clone(),
                title: Some(title),
                episode_number: Some((index + 1) as f32),
                url: Some(magnet),
                release_group: Some(format_bytes(file.size)),
                size_bytes: Some(file.size),
                language: Some("all".to_string()),
                ..VideoEpisode::default()
            }
        })
        .collect::<Vec<_>>();
    episodes.reverse();
    episodes
}

fn magnet_stream(request: &Value) -> VideoStream {
    let magnet = request_key(request, "episode")
        .or_else(|| {
            request
                .get("key")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_default();
    let title = request
        .get("episode")
        .and_then(|episode| episode.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("Magnet")
        .to_string();
    let size = request
        .get("episode")
        .and_then(|episode| episode.get("sizeBytes"))
        .and_then(Value::as_u64);
    VideoStream {
        url: magnet.clone(),
        name: Some(title.clone()),
        quality: Some("magnet".to_string()),
        format: Some("magnet".to_string()),
        stream_kind: Some(VideoStreamKind::Magnet),
        torrent: Some(TorrentInfo {
            magnet_url: Some(magnet.clone()),
            file_index: query_param(&magnet, "index").and_then(|value| value.parse().ok()),
            file_name: Some(title),
            size_bytes: size,
            ..TorrentInfo::default()
        }),
        initialized: true,
        ..VideoStream::default()
    }
}

#[derive(Debug)]
struct ParsedTorrent {
    info_hash: String,
    name: String,
    trackers: Vec<String>,
    files: Vec<TorrentFile>,
}

#[derive(Debug)]
struct TorrentFile {
    path: String,
    size: u64,
}

#[derive(Debug)]
enum BValue {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<BValue>),
    Dict(Vec<(Vec<u8>, BValue)>),
}

fn parse_torrent(bytes: &[u8]) -> ExtensionResult<ParsedTorrent> {
    let (info_start, info_end) =
        info_slice(bytes).ok_or_else(|| error("Torrent missing info dictionary"))?;
    let info_hash = hex(&Sha1::digest(&bytes[info_start..info_end]));
    let (root, _) = parse_value(bytes, 0).ok_or_else(|| error("Invalid torrent bencode"))?;
    let BValue::Dict(root) = root else {
        return Err(error("Invalid torrent root"));
    };
    let info = dict_value(&root, b"info").ok_or_else(|| error("Torrent missing info"))?;
    let BValue::Dict(info_dict) = info else {
        return Err(error("Torrent info is not a dictionary"));
    };
    let name = dict_bytes(info_dict, b"name")
        .and_then(bytes_to_string)
        .unwrap_or_else(|| "torrent".to_string());
    let mut trackers = Vec::new();
    if let Some(announce) = dict_bytes(&root, b"announce").and_then(bytes_to_string) {
        trackers.push(announce);
    }
    if let Some(BValue::List(tiers)) = dict_value(&root, b"announce-list") {
        for tier in tiers {
            if let BValue::List(values) = tier {
                for value in values {
                    if let BValue::Bytes(bytes) = value {
                        if let Some(tracker) = bytes_to_string(bytes) {
                            trackers.push(tracker);
                        }
                    }
                }
            }
        }
    }
    let files = if let Some(BValue::List(list)) = dict_value(info_dict, b"files") {
        list.iter()
            .filter_map(|value| {
                let BValue::Dict(file_dict) = value else {
                    return None;
                };
                let size = dict_int(file_dict, b"length")? as u64;
                let path = match dict_value(file_dict, b"path")? {
                    BValue::List(parts) => parts
                        .iter()
                        .filter_map(|part| match part {
                            BValue::Bytes(bytes) => bytes_to_string(bytes),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("/"),
                    _ => return None,
                };
                Some(TorrentFile { path, size })
            })
            .collect()
    } else {
        vec![TorrentFile {
            path: name.clone(),
            size: dict_int(info_dict, b"length").unwrap_or_default() as u64,
        }]
    };
    Ok(ParsedTorrent {
        info_hash,
        name,
        trackers,
        files,
    })
}

fn info_slice(bytes: &[u8]) -> Option<(usize, usize)> {
    if bytes.first().copied()? != b'd' {
        return None;
    }
    let mut pos = 1;
    while pos < bytes.len() && bytes[pos] != b'e' {
        let (key, next) = parse_bytes_raw(bytes, pos)?;
        pos = next;
        let start = pos;
        let end = skip_value(bytes, pos)?;
        if key == b"info" {
            return Some((start, end));
        }
        pos = end;
    }
    None
}

fn parse_value(bytes: &[u8], pos: usize) -> Option<(BValue, usize)> {
    match *bytes.get(pos)? {
        b'i' => {
            let end = bytes[pos + 1..].iter().position(|byte| *byte == b'e')? + pos + 1;
            let value = std::str::from_utf8(&bytes[pos + 1..end])
                .ok()?
                .parse()
                .ok()?;
            Some((BValue::Int(value), end + 1))
        }
        b'l' => {
            let mut pos = pos + 1;
            let mut out = Vec::new();
            while *bytes.get(pos)? != b'e' {
                let (value, next) = parse_value(bytes, pos)?;
                out.push(value);
                pos = next;
            }
            Some((BValue::List(out), pos + 1))
        }
        b'd' => {
            let mut pos = pos + 1;
            let mut out = Vec::new();
            while *bytes.get(pos)? != b'e' {
                let (key, next) = parse_bytes_raw(bytes, pos)?;
                let (value, after_value) = parse_value(bytes, next)?;
                out.push((key.to_vec(), value));
                pos = after_value;
            }
            Some((BValue::Dict(out), pos + 1))
        }
        b'0'..=b'9' => {
            let (value, next) = parse_bytes_raw(bytes, pos)?;
            Some((BValue::Bytes(value.to_vec()), next))
        }
        _ => None,
    }
}

fn skip_value(bytes: &[u8], pos: usize) -> Option<usize> {
    parse_value(bytes, pos).map(|(_, next)| next)
}

fn parse_bytes_raw(bytes: &[u8], pos: usize) -> Option<(&[u8], usize)> {
    let colon = bytes[pos..].iter().position(|byte| *byte == b':')? + pos;
    let len = std::str::from_utf8(&bytes[pos..colon])
        .ok()?
        .parse::<usize>()
        .ok()?;
    let start = colon + 1;
    let end = start.checked_add(len)?;
    (end <= bytes.len()).then_some((&bytes[start..end], end))
}

fn dict_value<'a>(dict: &'a [(Vec<u8>, BValue)], key: &[u8]) -> Option<&'a BValue> {
    dict.iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, value)| value)
}

fn dict_bytes<'a>(dict: &'a [(Vec<u8>, BValue)], key: &[u8]) -> Option<&'a Vec<u8>> {
    match dict_value(dict, key)? {
        BValue::Bytes(bytes) => Some(bytes),
        _ => None,
    }
}

fn dict_int(dict: &[(Vec<u8>, BValue)], key: &[u8]) -> Option<i64> {
    match dict_value(dict, key)? {
        BValue::Int(value) => Some(*value),
        _ => None,
    }
}

impl ParsedTorrent {
    fn magnet_for(&self, file: &TorrentFile, index: u32) -> String {
        let mut out = format!(
            "magnet:?xt=urn:btih:{}&dn={}",
            self.info_hash,
            url::query_escape(&self.name)
        );
        for tracker in &self.trackers {
            out.push_str("&tr=");
            out.push_str(&url::query_escape(tracker));
        }
        out.push_str("&index=");
        out.push_str(&index.to_string());
        out.push_str("&file=");
        out.push_str(&url::query_escape(&file.path));
        out
    }
}

fn id_from_url(input: &str) -> Option<String> {
    if input.trim().is_empty() {
        return None;
    }
    if input.starts_with("http") && !input.contains("ptorrents.com") {
        return None;
    }
    input
        .split("/anime/")
        .nth(1)
        .or_else(|| input.rsplit('/').next())
        .map(|value| value.trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && value != "ptorrents.com")
}

fn item_url(key: &str) -> String {
    if key.starts_with("http") {
        key.to_string()
    } else {
        format!("{BASE_URL}/anime/{}", key.trim_matches('/'))
    }
}

fn is_video_file(path: &str) -> bool {
    matches!(
        path.rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "mp4"
            | "mov"
            | "avi"
            | "wmv"
            | "mkv"
            | "flv"
            | "webm"
            | "mpeg"
            | "mpg"
            | "mts"
            | "vob"
            | "ts"
    )
}

fn bytes_to_string(bytes: &Vec<u8>) -> Option<String> {
    String::from_utf8(bytes.clone()).ok()
}

fn query_param(input: &str, name: &str) -> Option<String> {
    input.split('?').nth(1)?.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

fn format_bytes(bytes: u64) -> String {
    let kb = bytes as f64 / 1024.0;
    let mb = kb / 1024.0;
    let gb = mb / 1024.0;
    if gb >= 1.0 {
        format!("{gb:.2} GB")
    } else if mb >= 1.0 {
        format!("{mb:.2} MB")
    } else {
        format!("{kb:.2} KB")
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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

fn pref_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
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

fn with_query(request: &Value, query: &str) -> Value {
    let mut next = request.clone();
    next["query"] = Value::String(query.to_string());
    next
}

fn error(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.to_string(),
    }
}

export_video_source!(SOURCE);
