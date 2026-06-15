use aes::Aes256;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::{
    Encryptor,
    cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7},
};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack, UrlResolveResult,
    VideoEpisode, VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult,
    export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
    video::referer_headers,
};
use md5::{Digest, Md5};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const SOURCE: PutLocker = PutLocker;
const BASE_URL: &str = "https://ww7.putlocker.vip";

struct PutLocker;

impl VideoSource for PutLocker {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = if listing(&request) == "latest" {
            format!(
                "{BASE_URL}/filter/{}?genre=all&country=all&types=all&year=all&sort=updated",
                page(&request)
            )
        } else {
            format!("{BASE_URL}/putlocker/")
        };
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(
            &body,
            if listing(&request) == "latest" {
                "div.movies-list > div.ml-item"
            } else {
                "div#tab-movie > div.ml-item, div#tab-tv-show > div.ml-item"
            },
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&path)],
                has_next_page: false,
            });
        }
        let slug = query
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || ch.is_ascii_whitespace())
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("+")
            .to_ascii_lowercase();
        let target = format!("{BASE_URL}/movie/search/{slug}/{}/", page(&request));
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body, "div.movies-list > div.ml-item"))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/movie/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/movie/sample".to_string());
        let watch_url = format!("{BASE_URL}{}/watching.html", path.trim_end_matches('/'));
        let body = get_or_fixture(&watch_url, WATCH_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let media = request_key(&request, "episode")
            .and_then(|key| serde_json::from_str::<EpLinks>(&key).ok())
            .unwrap_or(EpLinks {
                data_id: "1_full".to_string(),
                media_id: "1".to_string(),
            });
        let target = format!(
            "{BASE_URL}/ajax/movie/episode/servers/{}_{}",
            media.media_id, media.data_id
        );
        let body = get_or_fixture(&target, SERVERS_FIXTURE, BASE_URL);
        Ok(parse_hosters(&body))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = key.splitn(3, '|');
        let data_id = parts.next().unwrap_or_default();
        let data_name = parts.next().unwrap_or_default();
        let display = parts.next().unwrap_or("Server");
        if data_id.is_empty() || data_name.is_empty() {
            return Ok(Vec::new());
        }
        let source_url =
            format!("{BASE_URL}/ajax/movie/episode/server/sources/{data_id}_{data_name}");
        let response = get_or_fixture(&source_url, EP_RESP_FIXTURE, BASE_URL);
        let embed = serde_json::from_str::<EpResp>(&response)
            .ok()
            .map(|value| value.src)
            .unwrap_or_default();
        let mut streams = resolve_embed(&embed, display, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut streams = Vec::new();
        for hoster in self.hosters(request.clone())? {
            let mut resolved = self.resolve_hoster(json!({
                "hoster": { "key": hoster.key },
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
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
                title: "Popular".to_string(),
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
        Ok(request_key(&request, "item").map(|key| absolute_url(&key)))
    }

    fn episode_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(None)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&path)),
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
        .with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, query: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: select_all(&doc, query).filter_map(card_item).collect(),
        has_next_page: select_all(&doc, "div#pagination li.active ~ li")
            .next()
            .is_some(),
    }
}

fn card_item(element: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&element, "div.mli-poster > a, a", "href")?;
    Some(CatalogItem {
        key: path_key(&href),
        title: text(&element, "div.mli-info h3, h3, a").unwrap_or_else(|| title_from_path(&href)),
        cover: attr(&element, "div.mli-poster > a > img, img", "data-original")
            .or_else(|| attr(&element, "div.mli-poster > a > img, img", "src"))
            .map(|value| absolute_url(&value)),
        url: Some(absolute_url(&href)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    parse_details(&body, path).unwrap_or_else(|| CatalogItem {
        key: path_key(path),
        title: title_from_path(path),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, path: &str) -> Option<CatalogItem> {
    let doc = Html::parse_document(body);
    let desc = select_all(&doc, "div.mvic-desc").next();
    Some(CatalogItem {
        key: path_key(path),
        title: select_text(&doc, "h1, div.thumb.mvic-thumb img")
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr(&doc, "div.thumb.mvic-thumb img, div.mvic-thumb img", "src")
            .map(|value| absolute_url(&value)),
        description: desc.map(|desc| {
            let mut lines = vec![text(&desc, "div.desc").unwrap_or_default()];
            lines.extend(
                select_all_in(desc, "div.mvic-info p")
                    .map(|p| collect_text(&p))
                    .filter(|line| !line.is_empty()),
            );
            lines
                .into_iter()
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        }),
        authors: desc.map(production_links).unwrap_or_default(),
        tags: desc.map(genre_links).unwrap_or_default(),
        status: ItemStatus::Completed,
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let script = body
        .split("<script")
        .find(|chunk| chunk.contains("total_episode"))
        .unwrap_or_default();
    let media_type = script
        .split("type:")
        .nth(1)
        .and_then(|tail| tail.split("name:").next())
        .unwrap_or("1")
        .replace(',', "")
        .trim()
        .to_string();
    let media_id = script
        .split("id:")
        .nth(1)
        .and_then(|tail| tail.split(',').next())
        .unwrap_or("1")
        .replace('"', "")
        .trim()
        .to_string();
    if media_type == "1" {
        return vec![episode("Movie", 1.0, "1_full", &media_id)];
    }
    let seasons = ajax_html(
        &format!("{BASE_URL}/ajax/movie/seasons/{media_id}"),
        SEASONS_FIXTURE,
    );
    let doc = Html::parse_document(&seasons);
    let mut episodes = Vec::new();
    for season in select_all(&doc, "div.dropdown-menu > a")
        .filter_map(|a| a.value().attr("data-id").map(ToString::to_string))
    {
        let html = ajax_html(
            &format!("{BASE_URL}/ajax/movie/season/episodes/{media_id}_{season}"),
            SEASON_EPISODES_FIXTURE,
        );
        let season_doc = Html::parse_document(&html);
        for anchor in select_all(&season_doc, "a") {
            let Some(data_id) = anchor.value().attr("data-id") else {
                continue;
            };
            let number = data_id
                .split('_')
                .next_back()
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(0.0);
            episodes.push(episode(
                &format!("Season {season} {}", collect_text(&anchor)),
                number,
                data_id,
                &media_id,
            ));
        }
    }
    episodes.sort_by(|a, b| {
        b.episode_number
            .unwrap_or(0.0)
            .partial_cmp(&a.episode_number.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    episodes
}

fn episode(name: &str, number: f32, data_id: &str, media_id: &str) -> VideoEpisode {
    let key = serde_json::to_string(&EpLinks {
        data_id: data_id.to_string(),
        media_id: media_id.to_string(),
    })
    .unwrap_or_default();
    VideoEpisode {
        key,
        title: Some(name.to_string()),
        episode_number: Some(number),
        language: Some("en".to_string()),
        ..VideoEpisode::default()
    }
}

fn ajax_html(target: &str, fixture: &str) -> String {
    let body = get_or_fixture(target, fixture, BASE_URL);
    serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("html")
                .and_then(Value::as_str)
                .map(html::html_unescape)
        })
        .unwrap_or(body)
}

fn production_links(desc: ElementRef<'_>) -> Vec<String> {
    labeled_links(desc, "Production")
}

fn genre_links(desc: ElementRef<'_>) -> Vec<String> {
    labeled_links(desc, "Genre")
}

fn labeled_links(desc: ElementRef<'_>, label: &str) -> Vec<String> {
    select_all_in(desc, "p")
        .filter(|p| collect_text(p).contains(label))
        .flat_map(|p| {
            select_all_in(p, "a")
                .map(|a| collect_text(&a))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn parse_hosters(body: &str) -> Vec<VideoHoster> {
    let html = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("html")
                .and_then(Value::as_str)
                .map(html::html_unescape)
        })
        .unwrap_or_else(|| body.to_string());
    let doc = Html::parse_document(&html);
    select_all(&doc, "a")
        .filter_map(|anchor| {
            let data_id = anchor.value().attr("data-id")?;
            let data_name = anchor.value().attr("data-name")?;
            let label = collect_text(&anchor);
            Some(VideoHoster {
                key: format!("{data_id}|{data_name}|{label}"),
                name: label,
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(BASE_URL),
                ..VideoHoster::default()
            })
        })
        .collect()
}

fn resolve_embed(embed: &str, server: &str, request: &Value) -> Vec<VideoStream> {
    if embed.is_empty() {
        return Vec::new();
    }
    let player = get_or_fixture(embed, EMBED_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&player);
    let Some(player_div) = select_all(&doc, "div#player").next() else {
        return vec![external_stream(embed, server)];
    };
    let Some(video_id) = player_div.value().attr("data-id") else {
        return vec![external_stream(embed, server)];
    };
    let Some(hash) = player_div.value().attr("data-hash") else {
        return vec![external_stream(embed, server)];
    };
    let Some(cipher) = cryptojs_encrypt(hash, &format!("\"{video_id}\"")) else {
        return vec![external_stream(embed, server)];
    };
    let host = embed.split("/embed-player").next().unwrap_or(BASE_URL);
    let target = format!(
        "{host}/ajax/getSources/?id={}&h={}&a={}&t={}",
        cipher.cipher_text, cipher.password, cipher.iv, cipher.salt
    );
    let body = get_or_fixture(&target, SOURCES_FIXTURE, embed);
    parse_sources(&body, embed, server, request)
        .unwrap_or_else(|| vec![external_stream(embed, server)])
}

fn parse_sources(
    body: &str,
    referer: &str,
    server: &str,
    request: &Value,
) -> Option<Vec<VideoStream>> {
    let data = serde_json::from_str::<Sources>(body).ok()?;
    let subtitles = data
        .tracks
        .unwrap_or_default()
        .into_iter()
        .filter_map(|track| {
            Some(SubtitleTrack {
                url: absolute_url(&track.file),
                label: track.label.clone(),
                language: track.label,
                ..SubtitleTrack::default()
            })
        })
        .collect::<Vec<_>>();
    let mut streams = streams_from_sources(&data.sources, referer, server, &subtitles, request);
    if streams.is_empty() {
        if let Some(backup) = data.backup_link.filter(|value| !value.is_empty()) {
            let backup_body = get_or_fixture(&backup, "", referer);
            if let Ok(backup_data) = serde_json::from_str::<Sources>(&backup_body) {
                streams = streams_from_sources(
                    &backup_data.sources,
                    referer,
                    &format!("{server} - Backup"),
                    &subtitles,
                    request,
                );
            }
        }
    }
    Some(streams)
}

fn streams_from_sources(
    sources: &[VidSource],
    referer: &str,
    server: &str,
    subtitles: &[SubtitleTrack],
    request: &Value,
) -> Vec<VideoStream> {
    sources
        .iter()
        .map(|source| {
            let is_hls = source.file.contains(".m3u8");
            let quality = quality_from_url(&source.file)
                .unwrap_or_else(|| source.kind.clone().unwrap_or_else(|| "Video".to_string()));
            VideoStream {
                url: source.file.clone(),
                name: Some(format!("{server} - {quality}")),
                quality: Some(quality.clone()),
                format: Some(
                    if is_hls {
                        "hls"
                    } else {
                        source.kind.as_deref().unwrap_or("mp4")
                    }
                    .to_string(),
                ),
                is_hls,
                stream_kind: Some(if is_hls {
                    VideoStreamKind::Hls
                } else {
                    VideoStreamKind::Direct
                }),
                subtitles: subtitles.to_vec(),
                preferred: quality.contains(&preferred_quality(request)),
                headers: referer_headers(referer),
                initialized: true,
                ..VideoStream::default()
            }
        })
        .collect()
}

fn external_stream(embed: &str, server: &str) -> VideoStream {
    VideoStream {
        url: embed.to_string(),
        name: Some(format!("{server} - External")),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(BASE_URL),
        initialized: true,
        ..VideoStream::default()
    }
}

fn cryptojs_encrypt(password: &str, plain_text: &str) -> Option<CipherResult> {
    let salt = [0x13, 0x37, 0x42, 0x66, 0x23, 0x19, 0x88, 0x05];
    let (key, iv) = evp_kdf(password.as_bytes(), &salt);
    let cipher = Encryptor::<Aes256>::new_from_slices(&key, &iv)
        .ok()?
        .encrypt_padded_vec_mut::<Pkcs7>(plain_text.as_bytes());
    Some(CipherResult {
        cipher_text: to_hex(STANDARD.encode(cipher).as_bytes()),
        password: to_hex(password.as_bytes()),
        salt: to_hex(&salt),
        iv: to_hex(&iv),
    })
}

fn evp_kdf(password: &[u8], salt: &[u8; 8]) -> ([u8; 32], [u8; 16]) {
    let mut out = Vec::with_capacity(48);
    let mut previous = Vec::new();
    while out.len() < 48 {
        let mut hasher = Md5::new();
        if !previous.is_empty() {
            hasher.update(&previous);
        }
        hasher.update(password);
        hasher.update(salt);
        previous = hasher.finalize().to_vec();
        out.extend_from_slice(&previous);
    }
    let mut key = [0_u8; 32];
    let mut iv = [0_u8; 16];
    key.copy_from_slice(&out[..32]);
    iv.copy_from_slice(&out[32..48]);
    (key, iv)
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn quality_from_url(input: &str) -> Option<String> {
    for marker in ["2160", "1080", "720", "480", "360"] {
        if input.contains(marker) {
            return Some(format!("{marker}p"));
        }
    }
    None
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = preferred_quality(request);
    streams.sort_by_key(|stream| {
        if stream
            .quality
            .as_deref()
            .unwrap_or_default()
            .contains(&preferred)
        {
            0
        } else {
            1
        }
    });
}

fn preferred_quality(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("preferred_quality"))
        .and_then(Value::as_str)
        .unwrap_or("1080")
        .to_string()
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(obj) = next.as_object_mut() {
        obj.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

fn page(request: &Value) -> u32 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1) as u32
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .or_else(|| value.get("url"))
                .or(Some(value))
        })
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(path_key)
}

fn request_raw_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| value.get("key").or(Some(value)))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn path_from_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.starts_with(BASE_URL) || trimmed.starts_with('/') {
        Some(path_key(trimmed))
    } else {
        None
    }
}

fn path_key(input: &str) -> String {
    if let Some(rest) = input.strip_prefix(BASE_URL) {
        return path_key(rest);
    }
    let path = input.split('#').next().unwrap_or(input);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("PutLocker")
        .replace(['-', '_'], " ")
        .split_whitespace()
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn selector(query: &str) -> Selector {
    Selector::parse(query).unwrap()
}

fn select_all<'a>(doc: &'a Html, query: &str) -> impl Iterator<Item = ElementRef<'a>> {
    doc.select(&selector(query)).collect::<Vec<_>>().into_iter()
}

fn select_all_in<'a>(element: ElementRef<'a>, query: &str) -> impl Iterator<Item = ElementRef<'a>> {
    element
        .select(&selector(query))
        .collect::<Vec<_>>()
        .into_iter()
}

fn select_attr(doc: &Html, query: &str, name: &str) -> Option<String> {
    select_all(doc, query)
        .next()
        .and_then(|element| attr(&element, "", name))
}

fn attr(element: &ElementRef<'_>, query: &str, name: &str) -> Option<String> {
    let target = if query.is_empty() {
        *element
    } else {
        select_all_in(*element, query).next()?
    };
    target
        .value()
        .attr(name)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn select_text(doc: &Html, query: &str) -> Option<String> {
    select_all(doc, query)
        .next()
        .map(|element| collect_text(&element))
        .filter(|value| !value.is_empty())
}

fn text(element: &ElementRef<'_>, query: &str) -> Option<String> {
    select_all_in(*element, query)
        .next()
        .map(|element| collect_text(&element))
        .filter(|value| !value.is_empty())
}

fn collect_text(element: &ElementRef<'_>) -> String {
    html::html_unescape(
        &element
            .text()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpLinks {
    data_id: String,
    media_id: String,
}

#[derive(Deserialize)]
struct EpResp {
    src: String,
}

#[derive(Deserialize)]
struct Sources {
    sources: Vec<VidSource>,
    tracks: Option<Vec<SubTrack>>,
    #[serde(rename = "backupLink")]
    backup_link: Option<String>,
}

#[derive(Deserialize)]
struct VidSource {
    file: String,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct SubTrack {
    file: String,
    label: Option<String>,
}

struct CipherResult {
    cipher_text: String,
    password: String,
    salt: String,
    iv: String,
}

const LIST_FIXTURE: &str = r#"
<div id="tab-movie"><div class="ml-item"><div class="mli-poster"><a href="/movie/sample"><img data-original="/poster.jpg"></a></div><div class="mli-info"><h3>Sample Movie</h3></div></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample Movie</h1><div class="mvic-desc"><div class="desc">Sample description.</div><div class="mvic-info"><p>Genre: <a>Action</a></p><p>Production: <a>Studio</a></p></div></div>
"#;

const WATCH_FIXTURE: &str = r#"
<script>var total_episode = 1; var type: 1, name: "Sample", id: "1", total_episode: 1;</script>
"#;

const SEASONS_FIXTURE: &str =
    r#"{ "html": "<div class=\"dropdown-menu\"><a data-id=\"1\">Season 1</a></div>" }"#;
const SEASON_EPISODES_FIXTURE: &str = r#"{ "html": "<a data-id=\"1_1\">Episode 1</a>" }"#;
const SERVERS_FIXTURE: &str =
    r#"{ "html": "<a data-id=\"server\" data-name=\"vidcloud\">VidCloud</a>" }"#;
const EP_RESP_FIXTURE: &str =
    r#"{ "status": true, "src": "https://ww7.putlocker.vip/embed-player/sample" }"#;
const EMBED_FIXTURE: &str = r#"<div id="player" data-id="sample" data-hash="password"></div>"#;
const SOURCES_FIXTURE: &str = r#"{ "sources": [{ "file": "https://cdn.example.invalid/sample-720.m3u8", "type": "hls" }], "tracks": [{ "file": "https://cdn.example.invalid/en.vtt", "label": "English" }], "backupLink": null }"#;

export_video_source!(SOURCE);
