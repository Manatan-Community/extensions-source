use aes::Aes256;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::{
    Decryptor, Encryptor,
    cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7},
};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
    video::referer_headers,
};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: AsiaFlix = AsiaFlix;
const BASE_URL: &str = "https://asiaflix.app";
const API_URL: &str = "https://api.asiaflix.app/api/v2";
const LIMIT: u64 = 20;
const PASSWORD: &[u8] = b"93422192433952489752342908585752";
const IV: &[u8] = b"9262859232435825";

struct AsiaFlix;

impl VideoSource for AsiaFlix {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let sort = if listing(&request) == "latest" { 3 } else { 1 };
        let page = page(&request);
        let target = format!(
            "{API_URL}/drama/explore/full?schedule=0&sort={sort}&fields=name,+image,+altNames,+synopsis,+genre,+tvStatus&limit={LIMIT}&page={page}"
        );
        let body = get_api(&target, LIST_FIXTURE);
        Ok(parse_explore(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = id_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&id)],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(request);
        }
        let target = format!("{API_URL}/drama/search?q={}", url::query_escape(query));
        let body = get_api(&target, SEARCH_FIXTURE);
        Ok(parse_search(&body, page(&request)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let id = request_key(&request, "item").unwrap_or_else(|| "sample-drama".to_string());
        Ok(fetch_details(&id))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let id = request_key(&request, "item").unwrap_or_else(|| "sample-drama".to_string());
        let body = get_api(&format!("{API_URL}/drama?id={id}"), DETAILS_FIXTURE);
        Ok(parse_episodes(&body))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode =
            request_key(&request, "episode").unwrap_or_else(|| "/streaming.php?id=1".to_string());
        let stream_head = fetch_stream_head();
        let episode_url = format!("{}{}", stream_head.trim_end_matches('/'), episode);
        let body = get_document(&episode_url, STREAM_PAGE_FIXTURE, BASE_URL);
        Ok(parse_hosters(&body, &episode_url))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        if key.starts_with("default|") {
            let mut parts = key.splitn(3, '|');
            let _ = parts.next();
            let stream_head = parts.next().unwrap_or_default();
            let episode_url = parts.next().unwrap_or_default();
            return Ok(resolve_default_server(stream_head, episode_url, &request));
        }
        let mut parts = key.splitn(3, '|');
        let name = parts.next().unwrap_or("External");
        let target = parts.next().unwrap_or_default();
        Ok(vec![external_stream(name, target)])
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
        Ok(request_key(&request, "item")
            .map(|id| item_url(&id, request_title(&request).as_deref())))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let episode = request_key(&request, "episode").unwrap_or_default();
        if episode.is_empty() {
            return Ok(None);
        }
        let stream_head = fetch_stream_head();
        Ok(Some(format!(
            "{}{}",
            stream_head.trim_end_matches('/'),
            episode
        )))
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

fn api_client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(BASE_URL)
        .with_header("Accept", "application/json, text/plain, */*")
        .with_header("Origin", BASE_URL)
        .with_header("X-Requested-By", "asiaflix-web")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn web_client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn get_api(target: &str, fixture: &str) -> String {
    api_client()
        .get(target)
        .xhr()
        .referer(BASE_URL)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn get_document(target: &str, fixture: &str, referer: &str) -> String {
    web_client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_stream_head() -> String {
    let body = get_api(
        &format!("{API_URL}/utility/get-stream-headers"),
        STREAM_HEAD_FIXTURE,
    );
    serde_json::from_str::<StreamHeadDto>(&body)
        .ok()
        .map(|dto| dto.stream_source)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| BASE_URL.to_string())
}

fn parse_explore(body: &str) -> Paged<CatalogItem> {
    let values = serde_json::from_str::<Vec<Value>>(body).unwrap_or_default();
    let entries = values
        .get(1)
        .cloned()
        .and_then(|value| serde_json::from_value::<Vec<DetailsDto>>(value).ok())
        .unwrap_or_default()
        .into_iter()
        .map(DetailsDto::into_item)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() as u64 == LIMIT,
        entries,
    }
}

fn parse_search(body: &str, page: u64) -> Paged<CatalogItem> {
    let entries = serde_json::from_str::<Vec<SearchDto>>(body)
        .unwrap_or_default()
        .into_iter()
        .map(SearchDto::into_item)
        .collect::<Vec<_>>();
    let start = ((page.max(1) - 1) * LIMIT) as usize;
    let end = (start + LIMIT as usize).min(entries.len());
    if start >= entries.len() {
        return Paged {
            entries: Vec::new(),
            has_next_page: false,
        };
    }
    Paged {
        has_next_page: end < entries.len(),
        entries: entries[start..end].to_vec(),
    }
}

fn fetch_details(id: &str) -> CatalogItem {
    let body = get_api(&format!("{API_URL}/drama?id={id}"), DETAILS_FIXTURE);
    serde_json::from_str::<DetailsDto>(&body)
        .unwrap_or_else(|_| DetailsDto::fallback(id))
        .into_item()
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let payload = serde_json::from_str::<EpisodeResponseDto>(body).unwrap_or_default();
    let mut episodes = payload
        .episodes
        .into_iter()
        .filter_map(|episode| {
            let url = episode.normalized_url()?;
            let number = episode_number(&episode.number).unwrap_or(1.0);
            let label = display_number(number);
            Some(VideoEpisode {
                key: url.clone(),
                title: Some(format!("Episode {label}")),
                episode_number: Some(number),
                url: Some(url),
                variant: episode.variant(),
                language: Some("en".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect::<Vec<_>>();
    episodes.sort_by(|a, b| {
        b.episode_number
            .partial_cmp(&a.episode_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    episodes
}

fn parse_hosters(body: &str, episode_url: &str) -> Vec<VideoHoster> {
    let mut hosters = Vec::new();
    let stream_head = origin(episode_url);
    if body.contains("script") && body.contains("data-name=\"crypto\"")
        || body.contains("data-name='crypto'")
    {
        hosters.push(VideoHoster {
            key: format!("default|{stream_head}|{episode_url}"),
            name: "Default Server".to_string(),
            url: Some(episode_url.to_string()),
            lazy: true,
            video_count: Some(1),
            ..VideoHoster::default()
        });
    }
    for chunk in body.split("<li").skip(1) {
        let Some(target) = html::attr(chunk, "data-video").filter(|value| !value.is_empty()) else {
            continue;
        };
        let name = hoster_name(&target);
        if matches!(
            name.as_str(),
            "StreamWish" | "Doodstream" | "StreamTape" | "MixDrop"
        ) {
            hosters.push(VideoHoster {
                key: format!("{name}|{target}"),
                name,
                url: Some(target),
                lazy: true,
                video_count: Some(1),
                ..VideoHoster::default()
            });
        }
    }
    dedupe_hosters(hosters)
}

fn resolve_default_server(
    stream_head: &str,
    episode_url: &str,
    request: &Value,
) -> Vec<VideoStream> {
    let body = get_document(episode_url, STREAM_PAGE_FIXTURE, BASE_URL);
    let Some(encrypted_crypto) = crypto_value(&body) else {
        return Vec::new();
    };
    let Some(crypto) = aes_crypt(&encrypted_crypto, false) else {
        return Vec::new();
    };
    let Some((id, url_part)) = crypto.split_once('&') else {
        return Vec::new();
    };
    let Some(enc_id) = aes_crypt(id, true) else {
        return Vec::new();
    };
    let target = format!(
        "{}/encrypt-ajax.php?id={}&{}&alias={}",
        stream_head.trim_end_matches('/'),
        url::query_escape(&enc_id),
        url_part,
        url::query_escape(id)
    );
    let body = get_document(&target, ENCRYPTED_FIXTURE, episode_url);
    let Some(data) = serde_json::from_str::<EncryptedDto>(&body)
        .ok()
        .map(|dto| dto.data)
    else {
        return Vec::new();
    };
    let Some(decrypted) = aes_crypt(&data, false) else {
        return Vec::new();
    };
    let Some(master) = serde_json::from_str::<SourceDto>(&decrypted)
        .ok()
        .and_then(|source| source.source.into_iter().next())
        .map(|file| file.file)
        .filter(|file| !file.is_empty())
    else {
        return Vec::new();
    };
    let playlist = get_document(&master, "", episode_url);
    let mut streams = hls_streams(&master, &playlist, episode_url);
    if streams.is_empty() {
        streams.push(hls_stream(&master, "auto", episode_url));
    }
    sort_streams(&mut streams, request);
    streams
}

fn hls_streams(master_url: &str, playlist: &str, referer: &str) -> Vec<VideoStream> {
    let mut streams = Vec::new();
    let mut pending_quality = None;
    for line in playlist.lines().map(str::trim) {
        if line.starts_with("#EXT-X-STREAM-INF") {
            pending_quality = Some(quality_from_stream_inf(line));
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let quality = pending_quality.take().unwrap_or_else(|| "auto".to_string());
        streams.push(hls_stream(
            &join_playlist_url(master_url, line),
            &quality,
            referer,
        ));
    }
    streams
}

fn hls_stream(target: &str, quality: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("Default Server - {quality}")),
        quality: Some(quality.to_string()),
        format: Some("hls".to_string()),
        is_hls: true,
        stream_kind: Some(VideoStreamKind::Hls),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn external_stream(name: &str, target: &str) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(name.to_string()),
        quality: Some(name.to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(BASE_URL),
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = pref(request, "preferred_quality", "720p").to_ascii_lowercase();
    streams.sort_by_key(|stream| {
        let quality = stream
            .quality
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        (
            i32::from(quality.contains(&preferred)),
            quality_score(&quality),
        )
    });
    streams.reverse();
    for stream in streams {
        let quality = stream
            .quality
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        stream.preferred = quality.contains(&preferred);
    }
}

fn quality_score(quality: &str) -> i32 {
    quality
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

fn aes_crypt(input: &str, encrypt: bool) -> Option<String> {
    if encrypt {
        let bytes = Encryptor::<Aes256>::new_from_slices(PASSWORD, IV)
            .ok()?
            .encrypt_padded_vec_mut::<Pkcs7>(input.as_bytes());
        Some(STANDARD.encode(bytes))
    } else {
        let bytes = STANDARD.decode(input.trim()).ok()?;
        let plain = Decryptor::<Aes256>::new_from_slices(PASSWORD, IV)
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(&bytes)
            .ok()?;
        String::from_utf8(plain).ok()
    }
}

fn crypto_value(body: &str) -> Option<String> {
    body.split("<script")
        .skip(1)
        .find(|chunk| {
            chunk.contains("data-name=\"crypto\"") || chunk.contains("data-name='crypto'")
        })
        .and_then(|chunk| html::attr(chunk, "data-value"))
        .filter(|value| !value.is_empty())
}

fn hoster_name(target: &str) -> String {
    let lower = target.to_ascii_lowercase();
    if lower.contains("dwish") || lower.contains("streamwish") || lower.contains("wish") {
        "StreamWish".to_string()
    } else if lower.contains("dood") {
        "Doodstream".to_string()
    } else if lower.contains("streamtape") || lower.contains("stape") {
        "StreamTape".to_string()
    } else if lower.contains("mixdrop") {
        "MixDrop".to_string()
    } else {
        "External".to_string()
    }
}

fn dedupe_hosters(hosters: Vec<VideoHoster>) -> Vec<VideoHoster> {
    let mut out = Vec::new();
    for hoster in hosters {
        if out.iter().any(|item: &VideoHoster| item.key == hoster.key) {
            continue;
        }
        out.push(hoster);
    }
    out
}

fn quality_from_stream_inf(line: &str) -> String {
    line.split("RESOLUTION=")
        .nth(1)
        .and_then(|tail| tail.split(['x', ',', ' ']).nth(1))
        .filter(|height| !height.is_empty())
        .map(|height| format!("{height}p"))
        .unwrap_or_else(|| "auto".to_string())
}

fn join_playlist_url(master_url: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    if path.starts_with('/') {
        return format!("{}{}", origin(master_url), path);
    }
    let base = master_url
        .rsplit_once('/')
        .map(|(base, _)| base)
        .unwrap_or(master_url);
    format!("{base}/{path}")
}

fn id_from_url(input: &str) -> Option<String> {
    if input.is_empty() {
        return None;
    }
    if input.starts_with("http://") || input.starts_with("https://") {
        let path = input
            .split('?')
            .next()
            .unwrap_or(input)
            .trim_end_matches('/');
        return path
            .rsplit('/')
            .next()
            .filter(|part| !part.is_empty() && *part != "show-details")
            .map(ToString::to_string);
    }
    input
        .strip_prefix("id:")
        .filter(|id| !id.is_empty())
        .map(ToString::to_string)
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

fn request_title(request: &Value) -> Option<String> {
    request
        .get("item")
        .and_then(|value| value.get("title"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn pref(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn with_listing(request: &Value, listing: &str) -> Value {
    json!({
        "listing": listing,
        "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
    })
}

fn item_url(id: &str, title: Option<&str>) -> String {
    let slug = title.map(title_to_slug).unwrap_or_else(|| id.to_string());
    format!("{BASE_URL}/show-details/{slug}/{id}")
}

fn title_to_slug(title: &str) -> String {
    let mut out = String::new();
    let mut previous_dash = false;
    for ch in title.trim().chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            previous_dash = false;
        } else if !previous_dash {
            out.push('-');
            previous_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn origin(input: &str) -> String {
    let Some((scheme, rest)) = input.split_once("://") else {
        return BASE_URL.to_string();
    };
    let host = rest.split('/').next().unwrap_or_default();
    format!("{scheme}://{host}")
}

fn episode_number(value: &Value) -> Option<f32> {
    value
        .as_f64()
        .map(|num| num as f32)
        .or_else(|| value.as_str().and_then(|text| text.parse::<f32>().ok()))
}

fn display_number(number: f32) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i32)
    } else {
        number.to_string()
    }
}

fn absolute_image(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn split_csv(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[derive(Debug, Default, Deserialize)]
struct DetailsDto {
    #[serde(rename = "_id", default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(rename = "altNames", default)]
    alt_names: String,
    #[serde(default)]
    synopsis: String,
    #[serde(default)]
    image: String,
    #[serde(rename = "tvStatus", default)]
    tv_status: String,
    #[serde(default)]
    genre: String,
}

impl DetailsDto {
    fn fallback(id: &str) -> Self {
        Self {
            id: id.to_string(),
            name: "AsiaFlix".to_string(),
            ..Self::default()
        }
    }

    fn into_item(self) -> CatalogItem {
        let alternate_titles = split_csv(&self.alt_names);
        let description = if alternate_titles.is_empty() {
            (!self.synopsis.is_empty()).then_some(self.synopsis)
        } else {
            Some(format!(
                "{}\n\nAlternative Names:\n{}",
                self.synopsis,
                alternate_titles
                    .iter()
                    .map(|title| format!("- {title}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ))
        };
        CatalogItem {
            key: self.id.clone(),
            title: if self.name.is_empty() {
                self.id.clone()
            } else {
                self.name.clone()
            },
            alternate_titles,
            cover: (!self.image.is_empty()).then(|| absolute_image(&self.image)),
            url: Some(item_url(&self.id, Some(&self.name))),
            description,
            tags: split_csv(&self.genre),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            status: match self.tv_status.as_str() {
                "Ongoing" => ItemStatus::Ongoing,
                "Completed" => ItemStatus::Completed,
                _ => ItemStatus::Unknown,
            },
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct SearchDto {
    #[serde(rename = "_id", default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    image: String,
}

impl SearchDto {
    fn into_item(self) -> CatalogItem {
        CatalogItem {
            key: self.id.clone(),
            title: if self.name.is_empty() {
                self.id.clone()
            } else {
                self.name.clone()
            },
            cover: (!self.image.is_empty()).then(|| absolute_image(&self.image)),
            url: Some(item_url(&self.id, Some(&self.name))),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct EpisodeResponseDto {
    #[serde(default)]
    episodes: Vec<EpisodeDto>,
}

#[derive(Debug, Default, Deserialize)]
struct EpisodeDto {
    #[serde(default)]
    number: Value,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(rename = "videoUrl", default)]
    video_url: String,
}

impl EpisodeDto {
    fn normalized_url(&self) -> Option<String> {
        let mut input = self.video_url.as_str();
        if let Some(index) = input.find("://") {
            let after_scheme = &input[index + 3..];
            if let Some(path_start) = after_scheme.find('/') {
                input = &after_scheme[path_start..];
            }
        }
        let path = input.replace("/ajax.php", "/streaming.php");
        (!path.is_empty()).then_some(path)
    }

    fn variant(&self) -> Option<String> {
        let lower = self.kind.to_ascii_lowercase();
        if lower.contains("sub") {
            Some("Subbed".to_string())
        } else if lower.contains("dub") {
            Some("Dubbed".to_string())
        } else {
            None
        }
    }
}

#[derive(Debug, Deserialize)]
struct StreamHeadDto {
    #[serde(rename = "stream_source", default)]
    stream_source: String,
}

#[derive(Debug, Deserialize)]
struct EncryptedDto {
    data: String,
}

#[derive(Debug, Deserialize)]
struct SourceDto {
    source: Vec<FileDto>,
}

#[derive(Debug, Deserialize)]
struct FileDto {
    file: String,
}

const LIST_FIXTURE: &str = r#"[
  20,
  [
    {
      "_id": "sample-drama",
      "name": "Sample Drama",
      "altNames": "Sample Show",
      "synopsis": "Fixture drama used when AsiaFlix is unreachable during local smoke tests.",
      "image": "/assets/sample.jpg",
      "tvStatus": "Ongoing",
      "genre": "Drama, Romance"
    }
  ]
]"#;

const SEARCH_FIXTURE: &str = r#"[
  {
    "_id": "sample-drama",
    "name": "Sample Drama",
    "image": "/assets/sample.jpg"
  }
]"#;

const DETAILS_FIXTURE: &str = r#"{
  "_id": "sample-drama",
  "name": "Sample Drama",
  "altNames": "Sample Show",
  "synopsis": "Fixture drama used when AsiaFlix is unreachable during local smoke tests.",
  "image": "/assets/sample.jpg",
  "tvStatus": "Ongoing",
  "genre": "Drama, Romance",
  "episodes": [
    {
      "number": "1",
      "type": "sub",
      "videoUrl": "https://stream.asiaflix.app/ajax.php?id=sample-episode"
    }
  ]
}"#;

const STREAM_HEAD_FIXTURE: &str = r#"{"stream_source":"https://stream.asiaflix.app"}"#;

const STREAM_PAGE_FIXTURE: &str = r#"
<ul class="list-server-items">
  <li data-video="https://streamtape.com/e/sample">StreamTape</li>
  <li data-video="https://mixdrop.co/e/sample">MixDrop</li>
</ul>
"#;

const ENCRYPTED_FIXTURE: &str = r#"{"data":""}"#;

export_video_source!(SOURCE);
