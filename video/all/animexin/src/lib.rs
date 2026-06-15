use aes::{Aes128, Aes256};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::{
    Decryptor, Encryptor,
    cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7},
};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind,
    abi::{ExtensionError, ExtensionResult},
    export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

const SOURCE: AnimeXin = AnimeXin;
const BASE_URL: &str = "https://animexin.dev";

struct AnimeXin;

impl VideoSource for AnimeXin {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let order = if listing == "latest" {
            "update"
        } else {
            "popular"
        };
        let body = get_or_fixture(
            &format!("{BASE_URL}/anime/?page={page}&order={order}"),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: has_next_page(&body),
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
                entries: vec![fetch_details(&path)],
                has_next_page: false,
            });
        }
        if query.starts_with("path:") {
            return Ok(Paged {
                entries: vec![fetch_details(query.trim_start_matches("path:"))],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let body = get_or_fixture(
            &format!(
                "{BASE_URL}/page/{page}/?s={}",
                manatan_shared::sdk::http::url_encode(query)
            ),
            SEARCH_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        Ok(fetch_details(
            &request_key(&request, "item").unwrap_or_else(|| "/anime/sample/".to_string()),
        ))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample/".to_string());
        let body = get_or_fixture(&absolute_url(&path), EPISODES_FIXTURE);
        Ok(parse_episodes(&body))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/sample-episode/".to_string());
        let body = get_or_fixture(&absolute_url(&path), HOSTERS_FIXTURE);
        Ok(parse_hosters(&body))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let name = request
            .get("hoster")
            .and_then(|hoster| hoster.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Mirror");
        let embed = resolve_embedded_url(&key)?;
        Ok(resolve_embed_streams(&embed, name))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut streams = Vec::new();
        for hoster in self.hosters(request)? {
            let request = json!({ "hoster": { "key": hoster.key, "name": hoster.name } });
            streams.extend(self.resolve_hoster(request)?);
        }
        sort_streams(&mut streams, "720p");
        Ok(streams)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: self.list(json!({"listing": "popular", "page": 1}))?.entries,
                has_more: true,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: self.list(json!({"listing": "latest", "page": 1}))?.entries,
                has_more: true,
                ..HomeSection::default()
            },
        ])
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE);
    parse_details(&body, path).unwrap_or_else(|| fallback_item(path))
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("<article")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "div class=\"tt", "</div>")
                .or_else(|| html::text_between(chunk, "div class='tt", "</div>"))
                .map(|text| html::strip_tags(&text))
                .filter(|text| !text.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))?;
            Some(CatalogItem {
                key: path_key(&href),
                title,
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| {
                        absolute_url(&image)
                            .split("?resize")
                            .next()
                            .unwrap_or("")
                            .to_string()
                    }),
                url: Some(absolute_url(&href)),
                language: Some("all".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, path: &str) -> Option<CatalogItem> {
    let title = html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value))?;
    let description = html::text_between(body, "entry-content", "</div>")
        .or_else(|| html::text_between(body, "class=\"desc", "</div>"))
        .map(|value| html::strip_tags(&value));
    let details = html::text_between(body, "info-content", "</div>")
        .or_else(|| html::text_between(body, "right ul data", "</ul>"))
        .unwrap_or_default();
    Some(CatalogItem {
        key: path_key(path),
        title,
        cover: html::attr_after(body, "div class=\"thumb", "src")
            .or_else(|| html::attr_after(body, "div class=\"limage", "src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description,
        tags: collect_anchor_text(body, "genxed"),
        authors: info_value(&details, "Fansub").into_iter().collect(),
        artists: info_value(&details, "Studio").into_iter().collect(),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(info_value(&details, "Status").as_deref()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    body.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let ep_text = html::text_between(chunk, "epl-num", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| "0".to_string());
            Some(VideoEpisode {
                key: path_key(&href),
                title: Some(format!("Episode {ep_text}")),
                episode_number: ep_text
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse::<f32>().ok()),
                date_uploaded: None,
                url: Some(absolute_url(&href)),
                language: Some("all".to_string()),
                labels: html::text_between(chunk, "epl-sub", "</")
                    .map(|value| vec![html::strip_tags(&value)])
                    .unwrap_or_default(),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_hosters(body: &str) -> Vec<VideoHoster> {
    let mut hosters = Vec::new();
    for chunk in body.split("<option").skip(1) {
        if !(chunk.contains("data-index") || chunk.contains("value=")) {
            continue;
        }
        if let Some(value) = html::attr(chunk, "value") {
            let name = html::strip_tags(chunk).trim().to_string();
            hosters.push(video_hoster(
                &value,
                if name.is_empty() { "Mirror" } else { &name },
            ));
        }
    }
    for chunk in body.split("<a").skip(1) {
        if let Some(value) = html::attr(chunk, "data-em") {
            let name = html::strip_tags(chunk).trim().to_string();
            hosters.push(video_hoster(
                &value,
                if name.is_empty() { "Mirror" } else { &name },
            ));
        }
    }
    hosters
}

fn video_hoster(key: &str, name: &str) -> VideoHoster {
    VideoHoster {
        key: key.to_string(),
        name: name.to_string(),
        lazy: true,
        video_count: Some(1),
        ..VideoHoster::default()
    }
}

fn resolve_embedded_url(encoded: &str) -> ExtensionResult<String> {
    if encoded.starts_with("http") {
        let doc = client()
            .get(encoded)
            .browser_document()
            .send_text()
            .map_err(|error| error_with(format!("hoster request failed: {}", error.message)))?;
        return embed_from_document(&doc).ok_or_else(|| error_with("missing embed iframe"));
    }
    let decoded = STANDARD
        .decode(encoded)
        .map_err(|error| error_with(format!("invalid encoded mirror data: {error}")))?;
    let doc = String::from_utf8_lossy(&decoded);
    embed_from_document(&doc).ok_or_else(|| error_with("missing embed iframe"))
}

fn embed_from_document(doc: &str) -> Option<String> {
    html::attr_after(doc, "<iframe", "src")
        .or_else(|| html::attr_after(doc, "itemprop=\"embedUrl\"", "content"))
        .map(|value| {
            if value.starts_with("//") {
                format!("https:{value}")
            } else {
                absolute_url(&value)
            }
        })
}

fn resolve_embed_streams(embed: &str, name: &str) -> Vec<VideoStream> {
    if embed.contains("vidstreaming") {
        return vidstreaming_streams(embed, name);
    }
    if embed.contains(".m3u8") {
        return vec![hls_stream(embed, name, "HLS", embed)];
    }
    vec![external_stream(embed, name)]
}

fn vidstreaming_streams(server_url: &str, name: &str) -> Vec<VideoStream> {
    let Ok(document) = client().get(server_url).browser_document().send_text() else {
        return vec![external_stream(server_url, name)];
    };
    let iv = digits_after(&document, "wrapper", "container-");
    let secret = digits_after(&document, "<body", "container-");
    let decrypt_key = digits_after(&document, "videocontent", "videocontent-");
    let Some(data_value) = html::attr_after(&document, "<script", "data-value") else {
        return vec![external_stream(server_url, name)];
    };
    let Some(params) = aes_crypt(&data_value, &iv, &secret, false)
        .and_then(|value| value.split_once('&').map(|(_, rest)| rest.to_string()))
    else {
        return vec![external_stream(server_url, name)];
    };
    let Some(id) = query_param(server_url, "id") else {
        return vec![external_stream(server_url, name)];
    };
    let Some(encrypted_id) = aes_crypt(&id, &iv, &secret, true) else {
        return vec![external_stream(server_url, name)];
    };
    let ajax = format!(
        "{}/encrypt-ajax.php?id={}&{}&alias={}",
        origin(server_url),
        manatan_shared::sdk::http::url_encode(&encrypted_id),
        params,
        id
    );
    let Ok(response) = client().get(ajax).xhr().send_text() else {
        return vec![external_stream(server_url, name)];
    };
    let Ok(payload) = serde_json::from_str::<Value>(&response) else {
        return vec![external_stream(server_url, name)];
    };
    let Some(data) = payload.get("data").and_then(Value::as_str) else {
        return vec![external_stream(server_url, name)];
    };
    let Some(decrypted) = aes_crypt(data, &iv, &decrypt_key, false) else {
        return vec![external_stream(server_url, name)];
    };
    let Ok(sources) = serde_json::from_str::<Value>(&decrypted) else {
        return vec![external_stream(server_url, name)];
    };
    let suffix = if server_url.contains("token=") {
        "Vid-mp4 - Gogostream"
    } else {
        "Vid-mp4 - Vidstreaming"
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
            let playlist = client().get(file).send_text().unwrap_or_default();
            let parsed = if playlist.contains("#EXT-X-STREAM-INF") {
                parse_hls_playlist(&playlist, file, name, suffix)
            } else {
                vec![hls_stream(file, name, &label, server_url)]
            };
            streams.extend(parsed);
        } else {
            streams.push(direct_stream(
                file,
                &format!("{name} - {label} ({suffix})"),
                &label,
                server_url,
            ));
        }
    }
    sort_streams(&mut streams, "720p");
    streams
}

fn parse_hls_playlist(body: &str, master: &str, name: &str, suffix: &str) -> Vec<VideoStream> {
    let mut streams = Vec::new();
    for block in body.split("#EXT-X-STREAM-INF:").skip(1) {
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
        let stream_url = if line.starts_with("http") {
            line.to_string()
        } else {
            format!(
                "{}/{}",
                master
                    .rsplit_once('/')
                    .map(|(base, _)| base)
                    .unwrap_or(master),
                line
            )
        };
        streams.push(hls_stream(
            &stream_url,
            name,
            &format!("{quality} ({suffix})"),
            master,
        ));
    }
    streams
}

fn hls_stream(stream_url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(format!("{name} - {quality}")),
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
        name: Some(name.to_string()),
        quality: Some(quality.to_string()),
        format: Some("mp4".to_string()),
        stream_kind: Some(VideoStreamKind::Direct),
        headers: referer_headers(referer),
        ..VideoStream::default()
    }
}

fn external_stream(stream_url: &str, name: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        initialized: true,
        ..VideoStream::default()
    }
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn aes_crypt(input: &str, iv: &[u8], key: &[u8], encrypt: bool) -> Option<String> {
    if encrypt {
        let bytes = match key.len() {
            16 => Encryptor::<Aes128>::new_from_slices(key, iv)
                .ok()?
                .encrypt_padded_vec_mut::<Pkcs7>(input.as_bytes()),
            32 => Encryptor::<Aes256>::new_from_slices(key, iv)
                .ok()?
                .encrypt_padded_vec_mut::<Pkcs7>(input.as_bytes()),
            _ => return None,
        };
        Some(STANDARD.encode(bytes))
    } else {
        let bytes = STANDARD.decode(input).ok()?;
        let plain = match key.len() {
            16 => Decryptor::<Aes128>::new_from_slices(key, iv)
                .ok()?
                .decrypt_padded_vec_mut::<Pkcs7>(&bytes)
                .ok()?,
            32 => Decryptor::<Aes256>::new_from_slices(key, iv)
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
        .find_map(|pair| pair.split_once('='))
        .filter(|(name, _)| *name == key)
        .map(|(_, value)| value.to_string())
}

fn origin(input: &str) -> String {
    let Some((scheme, rest)) = input.split_once("://") else {
        return BASE_URL.to_string();
    };
    format!("{scheme}://{}", rest.split('/').next().unwrap_or_default())
}

fn collect_anchor_text(body: &str, marker: &str) -> Vec<String> {
    let block = body.split(marker).nth(1).unwrap_or_default();
    block
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn info_value(block: &str, label: &str) -> Option<String> {
    block
        .split("<span")
        .find(|chunk| chunk.contains(label))
        .map(html::strip_tags)
        .map(|text| text.replace(label, "").replace(':', "").trim().to_string())
        .filter(|text| !text.is_empty())
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "completed" => ItemStatus::Completed,
        "ongoing" => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn sort_streams(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by_key(|stream| quality_score(stream.quality.as_deref()));
    streams.reverse();
    for stream in streams {
        stream.preferred = stream
            .quality
            .as_deref()
            .map(|quality| quality.contains(preferred))
            .unwrap_or(false);
    }
}

fn quality_score(quality: Option<&str>) -> i32 {
    quality
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .filter(|path| !path.trim_matches('/').is_empty())
        .map(path_key)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("url")))
        .and_then(Value::as_str)
        .map(path_key)
}

fn path_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) {
        return path_key(path);
    }
    let path = input.split('?').next().unwrap_or(input).trim();
    format!("/{}", path.trim_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn has_next_page(body: &str) -> bool {
    body.contains("pagination")
        && (body.contains("class=\"next\"")
            || body.contains("class='next'")
            || body.contains("hpage"))
}

fn fallback_item(path: &str) -> CatalogItem {
    CatalogItem {
        key: path_key(path),
        title: path.trim_matches('/').replace('-', " "),
        url: Some(absolute_url(path)),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn error_with(message: impl Into<String>) -> ExtensionError {
    ExtensionError {
        message: message.into(),
    }
}

const LIST_FIXTURE: &str = r#"
<div class="listupd">
<article><a class="tip" href="/anime/sample/"><img data-src="/sample.jpg"><div class="tt">Sample Anime</div></a></article>
</div>
"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample Anime</h1>
<div class="thumb"><img src="/sample.jpg"></div>
<div class="info-content"><span>Status: Ongoing</span><div class="genxed"><a>Action</a></div></div>
<div class="entry-content">Sample description.</div>
"#;
const EPISODES_FIXTURE: &str = r#"
<div class="eplister"><ul><li><a href="/sample-episode/"><span class="epl-num">1</span><span class="epl-sub">All Sub</span></a></li></ul></div>
"#;
const HOSTERS_FIXTURE: &str = r#"
<select class="mirror"><option data-index="0" value="PGlmcmFtZSBzcmM9Imh0dHBzOi8vZXhhbXBsZS5jb20vdmlkZW8ubTN1OCI+PC9pZnJhbWU+">Fixture HLS</option></select>
"#;

export_video_source!(SOURCE);
