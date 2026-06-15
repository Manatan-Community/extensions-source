use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack, UrlResolveResult,
    VideoEpisode, VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult,
    export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SOURCE: AllAnime = AllAnime;
const SITE_URL: &str = "https://allmanga.to";
const BASE_URL: &str = "https://allmanga.to/anime";
const API_URL: &str = "https://api.allanime.day";
const GRAPHQL_ORIGIN: &str = "https://youtu-chan.com";
const FALLBACK_PLAYER_DOMAIN: &str = "https://blog.allanime.day";
const PAGE_SIZE: u64 = 26;
const STREAM_HASH: &str = "d405d0edd690624b66baba3068e0edc3ac90f1597d898a1ec8db4e5c43c00fec";

struct AllAnime;

impl VideoSource for AllAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let body = if listing == "latest" {
            post_graphql(
                json!({
                    "variables": {
                        "search": { "allowAdult": true, "allowUnknown": true },
                        "limit": PAGE_SIZE,
                        "page": page,
                        "translationType": pref(&request, "preferred_sub", "sub"),
                        "countryOrigin": "ALL"
                    },
                    "query": SEARCH_QUERY
                }),
                SEARCH_FIXTURE,
            )
        } else {
            post_graphql(
                json!({
                    "variables": {
                        "type": "anime",
                        "size": PAGE_SIZE,
                        "dateRange": 7,
                        "page": page
                    },
                    "query": POPULAR_QUERY
                }),
                POPULAR_FIXTURE,
            )
        };
        Ok(if listing == "latest" {
            parse_search_result(&body)
        } else {
            parse_popular_result(&body)
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = id_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&id, None)],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(json!({ "listing": "popular" }));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = post_graphql(
            json!({
                "variables": {
                    "search": {
                        "query": query,
                        "allowAdult": true,
                        "allowUnknown": true
                    },
                    "limit": PAGE_SIZE,
                    "page": page,
                    "translationType": pref(&request, "preferred_sub", "sub"),
                    "countryOrigin": "ALL"
                },
                "query": SEARCH_QUERY
            }),
            SEARCH_FIXTURE,
        );
        Ok(parse_search_result(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(|| "sample".to_string());
        let title = request
            .get("item")
            .and_then(|item| item.get("title"))
            .and_then(Value::as_str);
        Ok(fetch_details(&key, title))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_else(|| "sample".to_string());
        let id = show_id(&key);
        let body = post_graphql(
            json!({
                "variables": { "_id": id },
                "query": EPISODES_QUERY
            }),
            EPISODES_FIXTURE,
        );
        Ok(parse_episodes(
            &body,
            pref(&request, "preferred_sub", "sub"),
        ))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let key = request_key(&request, "episode").unwrap_or_else(|| {
            json!({ "showId": "sample", "translationType": "sub", "episodeString": "1" })
                .to_string()
        });
        let body = get_episode_sources(&key);
        Ok(parse_hosters(&body, &request))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let mut parts = key.splitn(4, '|');
        let name = parts.next().unwrap_or("AllAnime");
        let priority = parts
            .next()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(0.0);
        let kind = parts.next().unwrap_or("external");
        let url = parts.next().unwrap_or_default();
        let mut streams = if kind == "internal" {
            resolve_internal(url, name)
        } else {
            resolve_external(url, name)
        };
        for stream in &mut streams {
            stream.preferred = priority >= 0.0;
        }
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
        Ok(
            request_key(&request, "item")
                .map(|key| format!("{SITE_URL}/bangumi/{}", show_id(&key))),
        )
    }

    fn episode_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(None)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&id, None)),
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
        .with_cookies_for(SITE_URL)
        .with_webview_challenge_fallback()
}

fn post_graphql(body: Value, fixture: &str) -> String {
    client()
        .post(format!("{API_URL}/api"))
        .header("Accept", "*/*")
        .header("Origin", GRAPHQL_ORIGIN)
        .referer(GRAPHQL_ORIGIN)
        .json(body.to_string())
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn get_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client()
        .get(target)
        .referer(referer)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular_result(body: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let entries = root
        .pointer("/data/queryPopular/recommendations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| value.get("anyCard"))
        .filter_map(card_item)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() as u64 == PAGE_SIZE,
        entries,
    }
}

fn parse_search_result(body: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let entries = root
        .pointer("/data/shows/edges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(card_item)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() as u64 == PAGE_SIZE,
        entries,
    }
}

fn card_item(card: &Value) -> Option<CatalogItem> {
    let id = card.get("_id")?.as_str()?;
    let name = card
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("AllAnime");
    let title = card
        .get("englishName")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(name);
    let key = format!(
        "{}<&sep>{}<&sep>{}",
        id,
        card.get("slugTime")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        slugify(name)
    );
    Some(CatalogItem {
        key,
        title: title.to_string(),
        cover: card
            .get("thumbnail")
            .and_then(Value::as_str)
            .map(thumbnail_url),
        url: Some(format!("{SITE_URL}/bangumi/{id}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn fetch_details(key: &str, title: Option<&str>) -> CatalogItem {
    let id = show_id(key);
    let body = post_graphql(
        json!({
            "variables": { "_id": id },
            "query": DETAILS_QUERY
        }),
        DETAILS_FIXTURE,
    );
    let root: Value = serde_json::from_str(&body).unwrap_or_default();
    let show = root.pointer("/data/show").unwrap_or(&Value::Null);
    CatalogItem {
        key: key.to_string(),
        title: title
            .map(ToString::to_string)
            .unwrap_or_else(|| title_from_key(key)),
        cover: show
            .get("thumbnail")
            .and_then(Value::as_str)
            .map(thumbnail_url),
        url: Some(format!("{SITE_URL}/bangumi/{id}")),
        description: details_description(show),
        tags: show
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect(),
        authors: show
            .get("studios")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect(),
        rating: show
            .get("score")
            .and_then(Value::as_f64)
            .map(|score| score as f32 / 2.0),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(show.get("status").and_then(Value::as_str)),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn details_description(show: &Value) -> Option<String> {
    let mut out = html::strip_tags(
        show.get("description")
            .and_then(Value::as_str)
            .unwrap_or(""),
    );
    let kind = show
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("Unknown");
    let score = show
        .get("score")
        .and_then(Value::as_f64)
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string());
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(&format!("Type: {kind}\nScore: {score}"));
    Some(out)
}

fn parse_episodes(body: &str, sub_pref: &str) -> Vec<VideoEpisode> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let show = root.pointer("/data/show").unwrap_or(&Value::Null);
    let show_id = show.get("_id").and_then(Value::as_str).unwrap_or("sample");
    let episodes = show
        .pointer(&format!("/availableEpisodesDetail/{sub_pref}"))
        .and_then(Value::as_array)
        .or_else(|| {
            show.pointer("/availableEpisodesDetail/sub")
                .and_then(Value::as_array)
        })
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|ep| {
            let key = json!({
                "showId": show_id,
                "translationType": sub_pref,
                "episodeString": ep
            })
            .to_string();
            VideoEpisode {
                key,
                title: Some(format!("Episode {ep} ({sub_pref})")),
                episode_number: ep.parse().ok(),
                language: Some("en".to_string()),
                labels: vec![sub_pref.to_string()],
                ..VideoEpisode::default()
            }
        })
        .collect::<Vec<_>>();
    episodes
}

fn get_episode_sources(key: &str) -> String {
    let variables: Value = serde_json::from_str(key)
        .ok()
        .and_then(|value: Value| value.get("variables").cloned().or(Some(value)))
        .unwrap_or_else(
            || json!({ "showId": "sample", "translationType": "sub", "episodeString": "1" }),
        );
    let extensions = json!({
        "persistedQuery": {
            "version": 1,
            "sha256Hash": STREAM_HASH
        }
    });
    let target = format!(
        "{API_URL}/api?variables={}&extensions={}",
        manatan_shared::sdk::http::url_encode(&variables.to_string()),
        manatan_shared::sdk::http::url_encode(&extensions.to_string())
    );
    get_or_fixture(&target, SOURCES_FIXTURE, BASE_URL)
}

fn parse_hosters(body: &str, request: &Value) -> Vec<VideoHoster> {
    let mut root: Value = serde_json::from_str(body).unwrap_or_default();
    if let Some(encrypted) = root
        .pointer("/data/tobeparsed")
        .and_then(Value::as_str)
        .and_then(decrypt_tobeparsed)
    {
        root = serde_json::from_str(&encrypted).unwrap_or_default();
    }
    let sources = root
        .pointer("/data/episode/sourceUrls")
        .or_else(|| root.pointer("/episode/sourceUrls"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    let mut hosters = Vec::new();
    for source in sources {
        let Some(raw_url) = source.get("sourceUrl").and_then(Value::as_str) else {
            continue;
        };
        let source_url = decrypt_source(raw_url);
        let source_name = source
            .get("sourceName")
            .and_then(Value::as_str)
            .unwrap_or("AllAnime");
        let source_type = source
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let priority = source
            .get("priority")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let kind = if source_url.starts_with("/apivtwo/") {
            "internal"
        } else {
            "external"
        };
        if !selected_hoster(request, &source_url, source_name, source_type, kind) {
            continue;
        }
        let name = if kind == "internal" {
            format!("Internal {source_name}")
        } else {
            hoster_name(&source_url, source_name)
        };
        hosters.push(VideoHoster {
            key: format!("{name}|{priority}|{kind}|{source_url}"),
            name,
            url: Some(BASE_URL.to_string()),
            lazy: true,
            video_count: Some(1),
            headers: referer_headers(BASE_URL),
            ..VideoHoster::default()
        });
    }
    hosters
}

fn selected_hoster(request: &Value, url: &str, name: &str, source_type: &str, kind: &str) -> bool {
    if kind == "internal" {
        return true;
    }
    if source_type == "player" {
        return true;
    }
    let preferred = pref(request, "preferred_server", "site_default");
    preferred == "site_default"
        || hoster_name(url, name)
            .to_ascii_lowercase()
            .contains(&preferred.to_ascii_lowercase())
}

fn resolve_internal(path: &str, name: &str) -> Vec<VideoStream> {
    let endpoint = client()
        .get(format!("{SITE_URL}/getVersion"))
        .xhr()
        .send_text()
        .ok()
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        .and_then(|value| {
            value
                .get("episodeIframeHead")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| FALLBACK_PLAYER_DOMAIN.to_string());
    let target = format!("{endpoint}{}", path.replace("/clock?", "/clock.json?"));
    let body = get_or_fixture(&target, INTERNAL_FIXTURE, &endpoint);
    let root: Value = serde_json::from_str(&body).unwrap_or_default();
    let mut streams = Vec::new();
    for link in root
        .get("links")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(url) = link.get("link").and_then(Value::as_str) else {
            continue;
        };
        let resolution = link
            .get("resolutionStr")
            .and_then(Value::as_str)
            .unwrap_or("auto");
        let subtitles = link
            .get("subtitles")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(parse_subtitle)
            .collect::<Vec<_>>();
        if link.get("hls").and_then(Value::as_bool).unwrap_or(false) || url.contains(".m3u8") {
            streams.extend(parse_hls(url, name, resolution, &endpoint, subtitles));
        } else {
            let mut stream = media_stream(url, name, resolution, &endpoint);
            stream.subtitles = subtitles;
            streams.push(stream);
        }
    }
    streams
}

fn resolve_external(url: &str, name: &str) -> Vec<VideoStream> {
    if url.contains(".m3u8") {
        return parse_hls(url, name, "auto", BASE_URL, Vec::new());
    }
    if url.contains(".mp4") || url.contains(".webm") || url.contains(".mkv") {
        return vec![media_stream(url, name, quality_from_url(url), BASE_URL)];
    }
    vec![VideoStream {
        url: normalize_url(url),
        name: Some(name.to_string()),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(BASE_URL),
        ..VideoStream::default()
    }]
}

fn parse_hls(
    target: &str,
    name: &str,
    fallback_quality: &str,
    referer: &str,
    subtitles: Vec<SubtitleTrack>,
) -> Vec<VideoStream> {
    let body = get_or_fixture(target, "", referer);
    if body.trim().is_empty() || !body.contains("#EXT-X-STREAM-INF") {
        let mut stream = media_stream(target, name, fallback_quality, referer);
        stream.subtitles = subtitles;
        return vec![stream];
    }
    let mut streams = Vec::new();
    let mut quality = fallback_quality.to_string();
    for line in body.lines() {
        if line.starts_with("#EXT-X-STREAM-INF") {
            quality = line
                .split("RESOLUTION=")
                .nth(1)
                .and_then(|part| part.split(['x', ',']).nth(1))
                .map(|height| format!("{height}p"))
                .unwrap_or_else(|| fallback_quality.to_string());
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            let stream_url = absolute_remote(line.trim(), target);
            let mut stream = media_stream(&stream_url, name, &quality, referer);
            stream.subtitles = subtitles.clone();
            streams.push(stream);
        }
    }
    streams
}

fn media_stream(url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = url.contains(".m3u8");
    VideoStream {
        url: normalize_url(url),
        name: Some(format!("{name} {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(referer),
        ..VideoStream::default()
    }
}

fn parse_subtitle(value: &Value) -> Option<SubtitleTrack> {
    Some(SubtitleTrack {
        url: value.get("src")?.as_str()?.to_string(),
        language: value
            .get("lang")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        label: value
            .get("label")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        format: Some("vtt".to_string()),
        ..SubtitleTrack::default()
    })
}

fn decrypt_source(input: &str) -> String {
    let (payload, key_type) = if let Some(rest) = input.strip_prefix("--") {
        (rest, Some(3usize))
    } else if let Some(rest) = input.strip_prefix("#-") {
        (rest, Some(2usize))
    } else if let Some(rest) = input.strip_prefix("##") {
        (rest, Some(1usize))
    } else if let Some(rest) = input.strip_prefix("-#") {
        (rest, Some(4usize))
    } else if let Some(rest) = input.strip_prefix('#') {
        (rest, Some(0usize))
    } else {
        (input, None)
    };
    let Ok(bytes) = hex_bytes(payload) else {
        return input.to_string();
    };
    if let Some(index) = key_type {
        return xor_decode(&bytes, XOR_MASKS[index]);
    }
    for mask in XOR_MASKS {
        let decoded = xor_decode(&bytes, mask);
        if decoded.contains("/clock") || decoded.contains("http") {
            return decoded;
        }
    }
    input.to_string()
}

fn decrypt_tobeparsed(input: &str) -> Option<String> {
    let blob = STANDARD.decode(input).ok()?;
    if blob.len() < 14 {
        return None;
    }
    let version = blob[0];
    let iv = &blob[1..13];
    let encrypted = &blob[13..];
    let key = Sha256::digest(format!("Xot36i3lK3:v{version}").as_bytes());
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    let plaintext = cipher.decrypt(Nonce::from_slice(iv), encrypted).ok()?;
    String::from_utf8(plaintext).ok()
}

fn hex_bytes(input: &str) -> Result<Vec<u8>, ()> {
    if input.len() % 2 != 0 {
        return Err(());
    }
    let mut bytes = Vec::new();
    for chunk in input.as_bytes().chunks(2) {
        let text = std::str::from_utf8(chunk).map_err(|_| ())?;
        bytes.push(u8::from_str_radix(text, 16).map_err(|_| ())?);
    }
    Ok(bytes)
}

fn xor_decode(bytes: &[u8], mask: u8) -> String {
    String::from_utf8_lossy(&bytes.iter().map(|byte| byte ^ mask).collect::<Vec<_>>()).into_owned()
}

const XOR_MASKS: [u8; 5] = [56, 49, 67, 55, 35];

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred_quality = pref(request, "preferred_quality", "1080");
    let preferred_server = pref(request, "preferred_server", "site_default").to_ascii_lowercase();
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or_default();
        let name = stream
            .name
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        (
            i32::from(preferred_server == "site_default" || name.contains(&preferred_server)),
            i32::from(quality.contains(preferred_quality)),
            quality_score(quality),
        )
    });
    streams.reverse();
    for stream in streams {
        stream.preferred = stream
            .quality
            .as_deref()
            .is_some_and(|quality| quality.contains(preferred_quality));
    }
}

fn quality_score(value: &str) -> i32 {
    value
        .split(|ch: char| !ch.is_ascii_digit())
        .find_map(|part| part.parse::<i32>().ok())
        .unwrap_or(0)
}

fn thumbnail_url(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("https://") {
        format!("https://wp.youtube-anime.com/{rest}?w=250")
    } else {
        format!("https://wp.youtube-anime.com/aln.youtube-anime.com/{url}?w=250")
    }
}

fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn show_id(key: &str) -> String {
    key.split("<&sep>").next().unwrap_or(key).to_string()
}

fn title_from_key(key: &str) -> String {
    key.split("<&sep>")
        .nth(2)
        .unwrap_or("AllAnime")
        .replace('-', " ")
}

fn id_from_url(input: &str) -> Option<String> {
    input.split("/bangumi/").nth(1).map(|value| {
        value
            .split(['/', '?', '#'])
            .next()
            .unwrap_or(value)
            .to_string()
    })
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value {
        Some("Releasing") | Some("Not Yet Released") => ItemStatus::Ongoing,
        Some("Finished") => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn hoster_name(url: &str, fallback: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if lower.contains("gogo") || lower.contains("vidstream") || lower.contains("playtaku") {
        "Vidstreaming".to_string()
    } else if lower.contains("dood") {
        "Doodstream".to_string()
    } else if lower.contains("ok.ru") || lower.contains("okru") {
        "Okru".to_string()
    } else if lower.contains("mp4upload") {
        "MP4Upload".to_string()
    } else if lower.contains("streamlare") {
        "Streamlare".to_string()
    } else if lower.contains("filemoon") || lower.contains("moonplayer") {
        "Filemoon".to_string()
    } else if lower.contains("wish") {
        "StreamWish".to_string()
    } else {
        fallback.to_string()
    }
}

fn normalize_url(input: &str) -> String {
    if input.starts_with("//") {
        format!("https:{input}")
    } else {
        input.to_string()
    }
}

fn absolute_remote(path: &str, base: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else if path.starts_with("//") {
        format!("https:{path}")
    } else {
        let prefix = base
            .rsplit_once('/')
            .map(|(prefix, _)| prefix)
            .unwrap_or(base);
        format!(
            "{}/{}",
            prefix.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

fn quality_from_url(url: &str) -> &str {
    for quality in ["2160p", "1440p", "1080p", "720p", "480p", "360p", "240p"] {
        if url.contains(quality) {
            return quality;
        }
    }
    "direct"
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn pref<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_str)
        .or_else(|| request.get(key).and_then(Value::as_str))
        .unwrap_or(default)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| value.get("key").or_else(|| value.get("url")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            request
                .get("key")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

const POPULAR_QUERY: &str = r#"query($type: VaildPopularTypeEnumType!, $size: Int!, $page: Int, $dateRange: Int) { queryPopular(type: $type, size: $size, dateRange: $dateRange, page: $page) { total recommendations { anyCard { _id name thumbnail englishName nativeName slugTime } } } }"#;
const SEARCH_QUERY: &str = r#"query($search: SearchInput, $limit: Int, $page: Int, $translationType: VaildTranslationTypeEnumType, $countryOrigin: VaildCountryOriginEnumType) { shows(search: $search, limit: $limit, page: $page, translationType: $translationType, countryOrigin: $countryOrigin) { pageInfo { total } edges { _id name thumbnail englishName nativeName slugTime } } }"#;
const DETAILS_QUERY: &str = r#"query($_id: String!) { show(_id: $_id) { thumbnail description type season score genres status studios } }"#;
const EPISODES_QUERY: &str =
    r#"query($_id: String!) { show(_id: $_id) { _id availableEpisodesDetail } }"#;

const POPULAR_FIXTURE: &str = r#"{"data":{"queryPopular":{"recommendations":[{"anyCard":{"_id":"sample","name":"Sample AllAnime","thumbnail":"sample.jpg","englishName":"Sample AllAnime","slugTime":"1"}}]}}}"#;
const SEARCH_FIXTURE: &str = r#"{"data":{"shows":{"edges":[{"_id":"sample","name":"Sample AllAnime","thumbnail":"sample.jpg","englishName":"Sample AllAnime","slugTime":"1"}]}}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"show":{"thumbnail":"sample.jpg","description":"Fixture details.","type":"TV","score":8.2,"genres":["Action"],"status":"Finished","studios":["Fixture Studio"]}}}"#;
const EPISODES_FIXTURE: &str =
    r#"{"data":{"show":{"_id":"sample","availableEpisodesDetail":{"sub":["1","2"],"dub":["1"]}}}}"#;
const SOURCES_FIXTURE: &str = r#"{"data":{"episode":{"sourceUrls":[{"sourceUrl":"https://fixtures.invalid/allanime/720.mp4","type":"player","sourceName":"Fixture","priority":1}]}}}"#;
const INTERNAL_FIXTURE: &str = r#"{"links":[{"link":"https://fixtures.invalid/allanime/master.m3u8","hls":true,"resolutionStr":"720p","subtitles":[{"lang":"en","src":"https://fixtures.invalid/allanime/en.vtt","label":"English"}]}]}"#;

export_video_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_plain_source() {
        assert_eq!(
            decrypt_source("https://fixtures.invalid/a.mp4"),
            "https://fixtures.invalid/a.mp4"
        );
    }

    #[test]
    fn parses_episode_fixture() {
        let episodes = parse_episodes(EPISODES_FIXTURE, "sub");
        assert_eq!(episodes.len(), 2);
        assert_eq!(episodes[0].episode_number, Some(1.0));
    }
}
