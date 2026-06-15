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

const SOURCE: NyaaTorrent = NyaaTorrent;
const DEFAULT_URL: &str = "https://nyaa.si";
const SUKEBEI_URL: &str = "https://sukebei.nyaa.si";

struct NyaaTorrent;

impl VideoSource for NyaaTorrent {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let mut target = format!(
            "{base}/?f=0&c={}&p={}",
            default_category(&base),
            page(&request)
        );
        if listing == "popular" {
            target.push_str("&s=seeders&o=desc");
        }
        let body = client(&base)
            .get(target)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_list(&body, &base))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = query.strip_prefix("id:") {
            return Ok(Paged {
                entries: vec![fetch_details(id, &base)],
                has_next_page: false,
            });
        }
        if let Some((id, found_base)) = id_from_url(query) {
            return self.search(with_query(
                &with_domain(&request, &found_base.unwrap_or(base)),
                &format!("id:{id}"),
            ));
        }
        let target = format!(
            "{base}/?f={}&c={}&s={}&o={}&q={}&p={}",
            filter(&request, "filter").unwrap_or_else(|| "0".to_string()),
            filter(&request, "category").unwrap_or_else(|| default_category(&base).to_string()),
            filter(&request, "sort").unwrap_or_else(|| "id".to_string()),
            filter(&request, "direction").unwrap_or_else(|| "desc".to_string()),
            url::query_escape(query),
            page(&request)
        );
        let body = client(&base)
            .get(target)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_list(&body, &base))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = request_key(&request, "item").unwrap_or_default();
        Ok(fetch_details(&key, &base))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base = base_url(&request);
        let key = request_key(&request, "item").unwrap_or_default();
        let body = client(&base)
            .get(item_url(&key, &base))
            .browser_document()
            .send_text()
            .unwrap_or_default();
        let torrent_path = html::attr_after(&body, "panel-footer", "href")
            .or_else(|| attr_containing(&body, ".torrent"))
            .ok_or_else(|| error("No torrent file found"))?;
        let torrent_url = url::join_url(&base, &torrent_path);
        let torrent = parse_torrent(&fetch_bytes(&torrent_url, &base)?)?;
        Ok(torrent_episodes(&torrent, &request))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        Ok(vec![magnet_stream(&request)])
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Seeders".to_string(),
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
        let base = base_url(&request);
        Ok(request_key(&request, "item").map(|key| item_url(&key, &base)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode"))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some((id, found_base)) = id_from_url(input) {
            let base = found_base.unwrap_or_else(|| base_url(&request));
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&id, &base)),
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
        .with_referer(base)
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn parse_list(body: &str, base: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<tr")
            .filter(|block| block.contains("/view/") && block.contains("td"))
            .filter_map(|block| parse_card(block, base))
            .collect(),
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn parse_card(block: &str, base: &str) -> Option<CatalogItem> {
    let href = block
        .split("<a")
        .filter(|chunk| chunk.contains("/view/") && !chunk.contains("comments"))
        .find_map(|chunk| html::attr(chunk, "href"))?;
    let (key, _) = id_from_url(&href)?;
    let title = block
        .split("<a")
        .filter(|chunk| chunk.contains("/view/") && !chunk.contains("comments"))
        .find_map(|chunk| html::attr(chunk, "title"))
        .filter(|value| !value.is_empty())
        .or_else(|| html::text_between(block, "href", "</a>").map(|text| html::strip_tags(&text)))
        .unwrap_or_else(|| key.clone());
    Some(CatalogItem {
        key: key.clone(),
        title,
        url: Some(item_url(&key, base)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(key: &str, base: &str) -> CatalogItem {
    let (key, found_base) = id_from_url(key).unwrap_or_else(|| (key.to_string(), None));
    let base = found_base.unwrap_or_else(|| base.to_string());
    let body = client(&base)
        .get(item_url(&key, &base))
        .browser_document()
        .send_text()
        .unwrap_or_default();
    let title = html::text_between(&body, "<h3", "</h3>")
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .or_else(|| html::attr_after(&body, "<meta property=\"og:title\"", "content"))
        .unwrap_or_else(|| key.clone());
    let desc = html::text_between(&body, "id=\"torrent-description\"", "</div>")
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty());
    let tags = panel_values(&body);
    CatalogItem {
        key: key.clone(),
        title,
        cover: image_from_text(desc.as_deref().unwrap_or_default()),
        description: desc,
        tags,
        authors: linked_values(&body, "title=\"User\""),
        url: Some(item_url(&key, &base)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_bytes(target: &str, base: &str) -> ExtensionResult<Vec<u8>> {
    let response = client(base).get(target).referer(base).send()?;
    response
        .body_base64
        .and_then(|value| STANDARD.decode(value).ok())
        .or_else(|| response.text.map(|text| text.into_bytes()))
        .ok_or_else(|| error("Empty torrent response"))
}

fn torrent_episodes(torrent: &ParsedTorrent, request: &Value) -> Vec<VideoEpisode> {
    let filename_only = pref_bool(request, "filename", false);
    let mut episode_number = 1.0;
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
            let current = episode_number;
            episode_number += 1.0;
            VideoEpisode {
                key: magnet.clone(),
                title: Some(title),
                episode_number: Some(current),
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
    trackers.sort();
    trackers.dedup();
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

fn base_url(request: &Value) -> String {
    pref(request, "domain")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn default_category(base: &str) -> &'static str {
    if base.contains("sukebei") {
        "1_1"
    } else {
        "1_0"
    }
}

fn id_from_url(input: &str) -> Option<(String, Option<String>)> {
    if input.trim().is_empty() {
        return None;
    }
    let base = if input.contains("sukebei.nyaa.si") {
        Some(SUKEBEI_URL.to_string())
    } else if input.contains("nyaa.si") {
        Some(DEFAULT_URL.to_string())
    } else if input.starts_with("http") {
        return None;
    } else {
        None
    };
    input
        .split("/view/")
        .nth(1)
        .or_else(|| input.rsplit('/').next())
        .map(|value| value.trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit()))
        .map(|id| (id, base))
}

fn item_url(key: &str, base: &str) -> String {
    if key.starts_with("http") {
        key.to_string()
    } else {
        format!("{base}/view/{}", key.trim_matches('/'))
    }
}

fn attr_containing(body: &str, needle: &str) -> Option<String> {
    body.split("<a").find_map(|chunk| {
        let href = html::attr(chunk, "href")?;
        href.contains(needle).then_some(href)
    })
}

fn panel_values(body: &str) -> Vec<String> {
    let labels = ["Category", "Seeders", "Leechers", "File size"];
    labels
        .iter()
        .filter_map(|label| {
            body.split(label)
                .nth(1)
                .and_then(|chunk| html::text_between(chunk, "<div", "</div>"))
                .map(|value| format!("{label}: {}", html::strip_tags(&value)))
        })
        .collect()
}

fn linked_values(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .collect()
}

fn image_from_text(input: &str) -> Option<String> {
    input
        .split_whitespace()
        .find(|part| {
            let lower = part.to_ascii_lowercase();
            (lower.starts_with("http://") || lower.starts_with("https://"))
                && ["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff"]
                    .iter()
                    .any(|ext| lower.contains(&format!(".{ext}")))
        })
        .map(|value| value.trim_matches(['"', '\'', ',', ')', '(']).to_string())
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
            | "ogg"
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

fn with_domain(request: &Value, domain: &str) -> Value {
    let mut next = request.clone();
    next["domain"] = Value::String(domain.to_string());
    next
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    next["listing"] = Value::String(listing.to_string());
    next
}

fn error(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.to_string(),
    }
}

export_video_source!(SOURCE);
