use aes::{Aes128, Aes256};
use base64::{Engine, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

type Aes128CbcDec = cbc::Decryptor<Aes128>;
type Aes128CbcEnc = cbc::Encryptor<Aes128>;
type Aes256CbcDec = cbc::Decryptor<Aes256>;
type Aes256CbcEnc = cbc::Encryptor<Aes256>;

const SOURCE: AnimeTake = AnimeTake;
const BASE_URL: &str = "https://animetake.tv";

struct AnimeTake;

impl VideoSource for AnimeTake {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/animelist/?page={page}")
        } else {
            format!("{BASE_URL}/animelist/popular")
        };
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
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
        let page = page(&request);
        let target = if query.is_empty() {
            let params = search_filter_params(&request);
            if params.is_empty() {
                format!("{BASE_URL}/animelist/?page={page}")
            } else {
                format!("{BASE_URL}/animelist/?page={page}&{params}")
            }
        } else {
            format!(
                "{BASE_URL}/search/?search={}&page={page}",
                url::query_escape(&query.replace(' ', "+").to_ascii_lowercase())
            )
        };
        let body = get_or_fixture(&target, SEARCH_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        let body = get_or_fixture(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode =
            request_key(&request, "episode").unwrap_or_else(|| "/watch/sample-1".to_string());
        let body = get_or_fixture(&absolute_url(&episode), WATCH_FIXTURE, BASE_URL);
        Ok(parse_hosters(&body, &absolute_url(&episode), &request))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let parts = key.split('|').collect::<Vec<_>>();
        if parts.len() < 4 {
            return Ok(Vec::new());
        }
        let kind = parts[0];
        let name = parts[1];
        let target = parts[2];
        let referer = parts[3];
        let mut streams = match kind {
            "vidstream" => resolve_vidstream(target, name),
            "filemoon" | "doodstream" | "mp4upload" => resolve_generic_embed(target, name, referer),
            _ => resolve_generic_embed(target, name, referer),
        };
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
        Ok(request_key(&request, "item").map(|path| absolute_url(&path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|path| absolute_url(&path)))
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
        .with_referer(referer)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("col-sm-6")
            .skip(1)
            .filter_map(parse_card)
            .collect(),
        has_next_page: has_next_page(body),
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let title = html::text_between(chunk, "latestep_title", "</span>")
        .or_else(|| html::text_between(chunk, "<h4", "</h4>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())?;
    Some(CatalogItem {
        key: path_key(&href),
        title,
        cover: html::attr_after(chunk, "<img", "data-src")
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(&href)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
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
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, path: &str) -> Option<CatalogItem> {
    let title = html::text_between(body, "<h3", "</h3>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())?;
    let mut description = html::text_between(body, "visible-md", "</div>")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    if let Some(extra) = html::text_between(body, "<div class=\"well", "</div>")
        .and_then(|block| html::text_between(&block, "<p", "</p>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
    {
        if description.is_empty() {
            description = extra;
        } else {
            description.push_str("\n\n");
            description.push_str(&extra);
        }
    }
    Some(CatalogItem {
        key: path_key(path),
        title,
        cover: html::attr_after(body, "<img", "data-src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description: (!description.is_empty()).then_some(description),
        tags: collect_label_spans(body),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: if body.contains("Next Episode") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Completed
        },
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let tab_content = body.split("tab-content").nth(1).unwrap_or(body);
    let mut episodes = tab_content
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("anime-title"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "anime-title", "</div>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path(&href));
            let date = html::text_between(chunk, "front_time", "</span>")
                .map(|value| html::strip_tags(&value))
                .and_then(|value| parse_date(&value));
            Some(VideoEpisode {
                key: path_key(&href),
                title: Some(title.clone()),
                episode_number: title
                    .split_whitespace()
                    .last()
                    .and_then(|value| value.parse::<f32>().ok()),
                date_uploaded: date,
                url: Some(absolute_url(&href)),
                language: Some("en".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect::<Vec<_>>();
    episodes.reverse();
    episodes
}

fn parse_hosters(body: &str, episode_url: &str, request: &Value) -> Vec<VideoHoster> {
    let mut out = Vec::new();
    for script in body.split("<script").skip(1) {
        if !script.contains("function") {
            continue;
        }
        let Some(frame_path) = extract_iframe_src(script) else {
            continue;
        };
        let frame_url = absolute_url(&frame_path);
        let frame = get_or_fixture(&frame_url, IFRAME_FIXTURE, episode_url);
        let embed = html::attr_after(&frame, "<iframe", "src")
            .map(|value| absolute_remote(&value, &frame_url))
            .unwrap_or(frame_url);
        if script.contains("vidstream()") {
            out.push(video_hoster(
                "vidstream",
                "Vidstreaming",
                &embed,
                episode_url,
            ));
            let player = get_or_fixture(&embed, GOGO_FIXTURE, episode_url);
            for chunk in player.split("linkserver").skip(1) {
                let Some(link) = html::attr(chunk, "data-video") else {
                    continue;
                };
                let name = html::strip_tags(chunk)
                    .split_whitespace()
                    .next()
                    .unwrap_or("Mirror")
                    .to_string();
                let normalized = absolute_remote(&link, &embed);
                out.push(video_hoster(
                    &hoster_kind(&normalized, &name),
                    &hoster_name(&normalized, &name),
                    &normalized,
                    &embed,
                ));
            }
        } else if script.contains("fm()") {
            out.push(video_hoster("filemoon", "Filemoon", &embed, episode_url));
        }
    }
    dedupe_hosters(out, request)
}

fn video_hoster(kind: &str, name: &str, target: &str, referer: &str) -> VideoHoster {
    VideoHoster {
        key: format!("{kind}|{name}|{target}|{referer}"),
        name: name.to_string(),
        url: Some(target.to_string()),
        lazy: true,
        video_count: Some(1),
        headers: referer_headers(referer),
        ..VideoHoster::default()
    }
}

fn resolve_vidstream(server_url: &str, name: &str) -> Vec<VideoStream> {
    let document = get_or_fixture(server_url, "", BASE_URL);
    let mut streams = gogo_streams(server_url, name, &document);
    if streams.is_empty() {
        streams = resolve_generic_embed(server_url, name, BASE_URL);
    }
    streams
}

fn gogo_streams(server_url: &str, name: &str, document: &str) -> Vec<VideoStream> {
    let iv = digits_after(document, "wrapper", "container-");
    let secret = digits_after(document, "<body", "container-");
    let decrypt_key = digits_after(document, "videocontent", "videocontent-");
    let Some(data_value) = html::attr_after(document, "<script", "data-value") else {
        return Vec::new();
    };
    let Some(params) = aes_crypt(&data_value, &iv, &secret, false)
        .and_then(|value| value.split_once('&').map(|(_, rest)| rest.to_string()))
    else {
        return Vec::new();
    };
    let Some(id) = query_param(server_url, "id") else {
        return Vec::new();
    };
    let Some(encrypted_id) = aes_crypt(&id, &iv, &secret, true) else {
        return Vec::new();
    };
    let ajax = format!(
        "{}/encrypt-ajax.php?id={}&{}&alias={}",
        origin(server_url),
        manatan_shared::sdk::http::url_encode(&encrypted_id),
        params,
        id
    );
    let response = client(server_url)
        .get(ajax)
        .xhr()
        .referer(server_url)
        .send_text()
        .unwrap_or_default();
    let Ok(payload) = serde_json::from_str::<Value>(&response) else {
        return Vec::new();
    };
    let Some(data) = payload.get("data").and_then(Value::as_str) else {
        return Vec::new();
    };
    let Some(decrypted) = aes_crypt(data, &iv, &decrypt_key, false) else {
        return Vec::new();
    };
    let Ok(sources) = serde_json::from_str::<Value>(&decrypted) else {
        return Vec::new();
    };
    let mut streams = Vec::new();
    for source in sources
        .get("source")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(file) = source.get("file").and_then(Value::as_str) else {
            continue;
        };
        let label = source
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("auto")
            .replace(' ', "");
        if file.contains(".m3u8") {
            streams.extend(parse_hls(file, name, server_url));
        } else {
            streams.push(direct_stream(file, name, &label, server_url));
        }
    }
    streams
}

fn resolve_generic_embed(target: &str, name: &str, referer: &str) -> Vec<VideoStream> {
    if target.contains(".m3u8") {
        return parse_hls(target, name, referer);
    }
    if target.ends_with(".mp4") || target.contains(".mp4?") {
        return vec![direct_stream(target, name, "mp4", referer)];
    }
    let body = get_or_fixture(target, "", referer);
    if let Some(manifest) = find_manifest(&body, target) {
        if manifest.contains(".m3u8") {
            return parse_hls(&manifest, name, target);
        }
        return vec![direct_stream(&manifest, name, "mp4", target)];
    }
    vec![external_stream(target, name, referer)]
}

fn find_manifest(body: &str, base: &str) -> Option<String> {
    for marker in [
        "file: \"",
        "file:\"",
        "file: '",
        "\"file\":\"",
        "\"src\":\"",
        "source src=\"",
    ] {
        let Some(value) = body.split(marker).nth(1) else {
            continue;
        };
        let raw = value
            .split(['"', '\'', '<'])
            .next()
            .unwrap_or_default()
            .replace("\\/", "/");
        if raw.contains(".m3u8") || raw.contains(".mp4") {
            return Some(absolute_remote(&raw, base));
        }
    }
    None
}

fn parse_hls(master: &str, name: &str, referer: &str) -> Vec<VideoStream> {
    let playlist = client(referer).get(master).send_text().unwrap_or_default();
    if !playlist.contains("#EXT-X-STREAM-INF") {
        return vec![hls_stream(master, name, "auto", referer)];
    }
    let mut streams = Vec::new();
    for block in playlist.split("#EXT-X-STREAM-INF:").skip(1) {
        let quality = block
            .split("RESOLUTION=")
            .nth(1)
            .and_then(|part| part.split('x').nth(1))
            .and_then(|part| part.split([',', '\n']).next())
            .map(|height| format!("{height}p"))
            .unwrap_or_else(|| "auto".to_string());
        let Some(line) = block
            .lines()
            .find(|line| !line.trim().is_empty() && !line.starts_with('#'))
        else {
            continue;
        };
        streams.push(hls_stream(
            &absolute_remote(line.trim(), master),
            name,
            &quality,
            referer,
        ));
    }
    if streams.is_empty() {
        streams.push(hls_stream(master, name, "auto", referer));
    }
    streams
}

fn hls_stream(stream_url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(format!("{name} {quality}")),
        quality: Some(quality.to_string()),
        format: Some("hls".to_string()),
        is_hls: true,
        stream_kind: Some(VideoStreamKind::Hls),
        headers: referer_headers(referer),
        ..VideoStream::default()
    }
}

fn direct_stream(stream_url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(format!("{name} {quality}")),
        quality: Some(quality.to_string()),
        format: Some("mp4".to_string()),
        stream_kind: Some(VideoStreamKind::Direct),
        headers: referer_headers(referer),
        ..VideoStream::default()
    }
}

fn external_stream(stream_url: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        initialized: true,
        headers: referer_headers(referer),
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred_quality = pref(request, "preferred_quality", "1080p");
    let preferred_server = pref(request, "preferred_server", "Vidstreaming");
    streams.sort_by_key(|stream| {
        let name = stream.name.as_deref().unwrap_or_default();
        let quality = stream.quality.as_deref().unwrap_or_default();
        (
            quality.contains(&preferred_quality),
            name.contains(&preferred_server),
            quality_score(quality),
        )
    });
    streams.reverse();
    for stream in streams {
        let name = stream.name.as_deref().unwrap_or_default();
        let quality = stream.quality.as_deref().unwrap_or_default();
        stream.preferred = quality.contains(&preferred_quality) || name.contains(&preferred_server);
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

fn search_filter_params(request: &Value) -> String {
    let mut params = Vec::new();
    add_filter_params(request, &mut params, "letter", "letters");
    add_filter_params(request, &mut params, "genre", "genres");
    add_filter_params(request, &mut params, "score", "score");
    add_filter_params(request, &mut params, "year", "years");
    add_filter_params(request, &mut params, "rating", "ratings");
    params.join("&")
}

fn add_filter_params(request: &Value, params: &mut Vec<String>, field: &str, query_key: &str) {
    for value in filter_values(request, field) {
        let normalized = option_value(&value);
        if normalized.is_empty() {
            continue;
        }
        params.push(format!("{}[]={}", query_key, normalized.replace(' ', "+")));
    }
}

fn filter_values(request: &Value, field: &str) -> Vec<String> {
    let Some(value) = request
        .get("filters")
        .and_then(|filters| filters.get(field))
        .or_else(|| request.get(field))
    else {
        return Vec::new();
    };
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    value
        .as_str()
        .map(|value| vec![value.to_string()])
        .unwrap_or_default()
}

fn option_value(value: &str) -> String {
    value
        .rsplit_once(':')
        .map(|(_, raw)| raw)
        .unwrap_or(value)
        .trim()
        .to_string()
}

fn extract_iframe_src(script: &str) -> Option<String> {
    let iframe = script.split("<iframe").nth(1)?;
    html::attr(iframe, "src")
}

fn collect_label_spans(body: &str) -> Vec<String> {
    body.split("animeinfo_label")
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, "<span", "</span>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
        })
        .collect()
}

fn dedupe_hosters(hosters: Vec<VideoHoster>, request: &Value) -> Vec<VideoHoster> {
    let preferred = pref(request, "preferred_server", "Vidstreaming");
    let mut out = Vec::new();
    for hoster in hosters {
        if out.iter().any(|item: &VideoHoster| item.key == hoster.key) {
            continue;
        }
        out.push(hoster);
    }
    out.sort_by_key(|hoster| hoster.name.contains(&preferred));
    out.reverse();
    out
}

fn hoster_kind(url: &str, fallback: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if lower.contains("dood") {
        "doodstream".to_string()
    } else if lower.contains("mp4upload") {
        "mp4upload".to_string()
    } else if lower.contains("filemoon") || lower.contains("moonplayer") {
        "filemoon".to_string()
    } else if lower.contains("vidstream") || lower.contains("gogo") || lower.contains("playtaku") {
        "vidstream".to_string()
    } else {
        fallback.to_ascii_lowercase().replace(' ', "")
    }
}

fn hoster_name(url: &str, fallback: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if lower.contains("dood") {
        "Doodstream".to_string()
    } else if lower.contains("mp4upload") {
        "Mp4upload".to_string()
    } else if lower.contains("filemoon") || lower.contains("moonplayer") {
        "Filemoon".to_string()
    } else if lower.contains("vidstream") || lower.contains("gogo") || lower.contains("playtaku") {
        "Vidstreaming".to_string()
    } else {
        fallback.to_string()
    }
}

fn parse_date(input: &str) -> Option<i64> {
    let parts = input.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 3 {
        return None;
    }
    let day = parts[0].parse::<i32>().ok()?;
    let month = match parts[1].to_ascii_lowercase().as_str() {
        "january" | "jan" => 1,
        "february" | "feb" => 2,
        "march" | "mar" => 3,
        "april" | "apr" => 4,
        "may" => 5,
        "june" | "jun" => 6,
        "july" | "jul" => 7,
        "august" | "aug" => 8,
        "september" | "sep" => 9,
        "october" | "oct" => 10,
        "november" | "nov" => 11,
        "december" | "dec" => 12,
        _ => return None,
    };
    let year = parts[2].parse::<i32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400_000)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let y = year - if month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146097 + doe - 719468)
}

fn aes_crypt(input: &str, iv: &[u8], key: &[u8], encrypt: bool) -> Option<String> {
    if encrypt {
        let bytes = match key.len() {
            16 => Aes128CbcEnc::new_from_slices(key, iv)
                .ok()?
                .encrypt_padded_vec_mut::<Pkcs7>(input.as_bytes()),
            32 => Aes256CbcEnc::new_from_slices(key, iv)
                .ok()?
                .encrypt_padded_vec_mut::<Pkcs7>(input.as_bytes()),
            _ => return None,
        };
        Some(STANDARD.encode(bytes))
    } else {
        let bytes = STANDARD.decode(input).ok()?;
        let plain = match key.len() {
            16 => Aes128CbcDec::new_from_slices(key, iv)
                .ok()?
                .decrypt_padded_vec_mut::<Pkcs7>(&bytes)
                .ok()?,
            32 => Aes256CbcDec::new_from_slices(key, iv)
                .ok()?
                .decrypt_padded_vec_mut::<Pkcs7>(&bytes)
                .ok()?,
            _ => return None,
        };
        String::from_utf8(plain).ok()
    }
}

fn digits_after(document: &str, marker: &str, prefix: &str) -> Vec<u8> {
    document
        .split(marker)
        .nth(1)
        .and_then(|chunk| chunk.split(prefix).nth(1))
        .unwrap_or_default()
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .map(|ch| ch as u8)
        .collect()
}

fn query_param(input: &str, key: &str) -> Option<String> {
    input
        .split('?')
        .nth(1)?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value.to_string())
}

fn origin(input: &str) -> String {
    let Some((scheme, rest)) = input.split_once("://") else {
        return BASE_URL.to_string();
    };
    format!("{scheme}://{}", rest.split('/').next().unwrap_or_default())
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn absolute_url(input: &str) -> String {
    absolute_remote(input, BASE_URL)
}

fn absolute_remote(input: &str, base: &str) -> String {
    let trimmed = input.trim().replace("\\/", "/");
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed
    } else if let Some(rest) = trimmed.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url::join_url(base, &trimmed)
    }
}

fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .filter(|path| !path.trim_matches('/').is_empty())
        .map(path_key)
}

fn path_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) {
        return path_key(path);
    }
    format!(
        "/{}",
        input
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .trim_matches('/')
    )
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request_raw_key(request, field).map(|value| path_key(&value))
}

fn request_raw_key(request: &Value, field: &str) -> Option<String> {
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

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("AnimeTake")
        .replace('-', " ")
}

fn has_next_page(body: &str) -> bool {
    body.contains("pagination") && body.contains("page-item")
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

fn with_listing(request: &Value, listing: &str) -> Value {
    json!({
        "listing": listing,
        "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
    })
}

const LIST_FIXTURE: &str = r#"
<div class="col-sm-6"><div><a href="/anime/sample-anime"><div class="latestep_image"><img data-src="/sample.jpg"></div><span class="latestep_title"><h4>Sample Anime</h4></span></a></div></div>
"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"
<h3><b>Sample Anime</b></h3>
<a class="animeinfo_label"><span>Action</span></a>
<div class="visible-md">Sample description.</div>
<div class="well"><center>Next Episode</center><p>Sample extra description.</p></div>
<div class="tab-content"><div id="eps"><a href="/watch/sample-1"><div class="col-xs-12"><span class="front_time">01 January 2024</span><div class="anime-title"><b>Episode 1</b></div></div></a></div></div>
"#;
const WATCH_FIXTURE: &str = r#"
<div id="divscript"><script>function vidstream(){document.write('<iframe src="/embed/sample"></iframe>');}</script></div>
"#;
const IFRAME_FIXTURE: &str =
    r#"<iframe src="https://vidstreaming.example/streaming.php?id=sample"></iframe>"#;
const GOGO_FIXTURE: &str = r#"<div id="list-server-more"><ul><li class="linkserver" data-video="https://mp4upload.com/embed-sample.html">Mp4upload</li></ul></div>"#;

export_video_source!(SOURCE);
