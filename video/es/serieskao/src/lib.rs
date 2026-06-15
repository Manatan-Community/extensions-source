use aes::Aes256;
use base64::{Engine as _, engine::general_purpose};
use cbc::{
    Decryptor,
    cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7},
};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::{Value, json};

type Aes256CbcDec = Decryptor<Aes256>;

const SOURCE: SeriesKao = SeriesKao;
const BASE_URL: &str = "https://serieskao.top";
const AES_KEY: &str = "Ak7qrvvH4WKYxV2OgaeHAEg2a5eh16vE";

struct SeriesKao;

impl VideoSource for SeriesKao {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = fetch(
            &format!("{BASE_URL}/series?page={}", page(&request)),
            LIST_FIXTURE,
            BASE_URL,
        );
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
        if query.is_empty() {
            return self.list(request);
        }
        let body = fetch(
            &format!(
                "{BASE_URL}/search?s={}&page={}",
                url::query_escape(query),
                page(&request)
            ),
            LIST_FIXTURE,
            BASE_URL,
        );
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/serie/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/serie/sample".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body, &path))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode =
            request_key(&request, "episode").unwrap_or_else(|| "/ver/sample-1".to_string());
        let referer = absolute_url(&episode);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        Ok(parse_episode_hosters(&body, &referer))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = key.splitn(4, '|');
        let name = parts.next().unwrap_or("External");
        let embed = parts.next().unwrap_or_default();
        let lang = parts.next().unwrap_or("unknown");
        let referer = parts.next().unwrap_or(BASE_URL);
        let mut streams = resolve_embed(embed, name, lang, referer, &request);
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
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Series".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        }])
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
            if path.starts_with("/ver/") {
                return Ok(Some(UrlResolveResult {
                    episode: Some(
                        json!({"key": path, "url": absolute_url(&path), "language": "es"}),
                    ),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
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

fn fetch(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc
            .select(&selector("a.poster-card"))
            .filter_map(card)
            .collect(),
        has_next_page: doc.select(&selector("a.page-link")).next().is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href")?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_text(el, ".poster-card__title")
            .or_else(|| {
                attr(&el, "title").map(|value| value.trim_start_matches("VER").trim().to_string())
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_path(&key)),
        cover: select_attr(el, "img", "src")
            .or_else(|| select_attr(el, "img", "data-src"))
            .map(|src| absolute_url(&src.replace("/w154/", "/w500/"))),
        url: Some(absolute_url(&key)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, "h1.m-b-5, h1, .poster-card__title").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, "div.card-body div.row div.col-sm-3 img.img-fluid, .poster img, meta[property='og:image']", "src")
            .or_else(|| select_attr_doc(&doc, "meta[property='og:image']", "content"))
            .map(|src| absolute_url(&src.replace("/w154/", "/w500/"))),
        description: select_text_doc(&doc, "div.col-sm-4 div.text-large, .text-large, .sinopsis, .overview"),
        tags: select_texts_doc(&doc, "div.p-v-20.p-h-15.text-center a span, .genres a, a[href*='/generos/']"),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, path: &str) -> Vec<VideoEpisode> {
    if path.contains("/pelicula/") {
        return vec![VideoEpisode {
            key: path.to_string(),
            title: Some("PELICULA".to_string()),
            episode_number: Some(1.0),
            url: Some(absolute_url(path)),
            language: Some("es".to_string()),
            ..VideoEpisode::default()
        }];
    }
    let doc = Html::parse_document(body);
    let tabs = doc
        .select(&selector("#season-tabs li a[data-tab]"))
        .collect::<Vec<_>>();
    let number_re = Regex::new(r#"\d+"#).unwrap();
    let mut episodes = Vec::new();
    if tabs.is_empty() {
        for (index, el) in doc
            .select(&selector(".episodes-list a.episode-item"))
            .enumerate()
        {
            let episode_number = select_text(el, ".episode-number")
                .and_then(|text| number_re.find(&text).map(|m| m.as_str().to_string()))
                .unwrap_or_else(|| (index + 1).to_string());
            let title = select_text(el, ".episode-title")
                .unwrap_or_else(|| format!("Episodio {episode_number}"));
            let href = attr(&el, "href").unwrap_or_default();
            episodes.push(episode_item(
                &href,
                &format!("T1 - Episodio {episode_number}: {title}"),
                &episode_number,
                Some(1.0),
            ));
        }
    } else {
        for (index, tab) in tabs.into_iter().enumerate() {
            let season_id = attr(&tab, "data-tab").unwrap_or_default();
            let season_number = number_re
                .find(&text(tab))
                .map(|m| m.as_str().to_string())
                .or_else(|| number_re.find(&season_id).map(|m| m.as_str().to_string()))
                .unwrap_or_else(|| (index + 1).to_string());
            let pane_selector = format!("#{season_id}");
            let episode_selector = selector(".episodes-list a.episode-item");
            for pane in doc.select(&selector(&pane_selector)) {
                for el in pane.select(&episode_selector) {
                    let episode_number = select_text(el, ".episode-number")
                        .and_then(|text| number_re.find(&text).map(|m| m.as_str().to_string()))
                        .unwrap_or_else(|| "0".to_string());
                    let title = select_text(el, ".episode-title")
                        .unwrap_or_else(|| format!("Episodio {episode_number}"));
                    let href = attr(&el, "href").unwrap_or_default();
                    episodes.push(episode_item(
                        &href,
                        &format!("T{season_number} - Episodio {episode_number}: {title}"),
                        &episode_number,
                        season_number.parse::<f32>().ok(),
                    ));
                }
            }
        }
    }
    episodes.reverse();
    episodes
}

fn episode_item(href: &str, title: &str, number: &str, season: Option<f32>) -> VideoEpisode {
    let key = path_key(href);
    VideoEpisode {
        key: key.clone(),
        title: Some(title.to_string()),
        episode_number: number.parse::<f32>().ok(),
        season_number: season,
        url: Some(absolute_url(&key)),
        language: Some("es".to_string()),
        ..VideoEpisode::default()
    }
}

fn parse_episode_hosters(body: &str, referer: &str) -> Vec<VideoHoster> {
    let sources_block = Regex::new(r#"var\s+videoSources\s*=\s*\[(?s)(.+?)]\s*;"#)
        .unwrap()
        .captures(body)
        .and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
        .unwrap_or_default();
    let source_re = Regex::new(r#"['"]([^'"]+)['"]"#).unwrap();
    let mut hosters = Vec::new();
    for source_url in source_re
        .captures_iter(&sources_block)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
        .filter(|url| url.starts_with("http"))
    {
        let source_body = fetch(&source_url, "", referer);
        for (embed, lang) in parse_data_links(&source_body) {
            let name = host_name(&embed);
            hosters.push(VideoHoster {
                key: format!("{name}|{}|{lang}|{source_url}", absolute_url(&embed)),
                name: format!("{lang} {name}"),
                url: Some(absolute_url(&embed)),
                lazy: true,
                video_count: Some(1),
                ..VideoHoster::default()
            });
        }
    }
    hosters
}

fn parse_data_links(body: &str) -> Vec<(String, String)> {
    let raw = Regex::new(r#"dataLink\s*=\s*(?s)([^;]+);"#)
        .unwrap()
        .captures(body)
        .and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()));
    let Some(payload) = raw.as_deref().and_then(resolve_data_link) else {
        return Vec::new();
    };
    let Ok(items) = serde_json::from_str::<Vec<Item>>(&payload) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let lang = match item
            .video_language
            .unwrap_or_default()
            .to_ascii_uppercase()
            .as_str()
        {
            "LAT" => "[LAT]",
            "ESP" => "[CAST]",
            "SUB" => "[SUB]",
            _ => "unknown",
        };
        for embed in item.sorted_embeds {
            if embed
                .embed_type
                .as_deref()
                .unwrap_or_default()
                .eq_ignore_ascii_case("video")
            {
                if let Some(link) = decrypt_embed_link(embed.link.as_deref()) {
                    out.push((link, lang.to_string()));
                }
            }
        }
    }
    out
}

fn resolve_data_link(raw: &str) -> Option<String> {
    let mut expr = raw.trim().trim_end_matches(';').to_string();
    for _ in 0..8 {
        if let Some(inner) =
            outer_call(&expr, "JSON.parse").or_else(|| outer_call(&expr, "window.JSON.parse"))
        {
            expr = trim_quotes(inner).to_string();
        } else if let Some(inner) = outer_call(&expr, "decodeURIComponent")
            .or_else(|| outer_call(&expr, "window.decodeURIComponent"))
        {
            expr = percent_decode(trim_quotes(inner))?;
        } else if let Some(inner) =
            outer_call(&expr, "atob").or_else(|| outer_call(&expr, "window.atob"))
        {
            expr = String::from_utf8(decode_base64_any(trim_quotes(inner))?).ok()?;
        } else {
            break;
        }
    }
    let expr = trim_quotes(expr.trim()).to_string();
    (!expr.is_empty()).then_some(expr)
}

fn decrypt_embed_link(raw: Option<&str>) -> Option<String> {
    let link = raw?.trim();
    if link.starts_with("http://") || link.starts_with("https://") {
        return Some(link.to_string());
    }
    aes_decrypt(link).or_else(|| decode_jwt_link(link))
}

fn aes_decrypt(input: &str) -> Option<String> {
    let data = decode_base64_any(input)?;
    let key = AES_KEY.as_bytes();
    let attempts = [
        (data.get(16..)?.to_vec(), data.get(..16)?.to_vec()),
        (data.clone(), vec![0; 16]),
    ];
    for (cipher_text, iv) in attempts {
        if let Ok(plain) = Aes256CbcDec::new_from_slices(key, &iv)
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(&cipher_text)
        {
            if let Ok(text) = String::from_utf8(plain) {
                if text.starts_with("http") {
                    return Some(text);
                }
            }
        }
    }
    None
}

fn decode_jwt_link(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let value: Value = serde_json::from_slice(&decode_base64_any(payload)?).ok()?;
    value
        .get("link")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("data")
                .and_then(|data| data.get("link"))
                .and_then(Value::as_str)
        })
        .map(ToString::to_string)
}

fn resolve_embed(
    embed: &str,
    name: &str,
    lang: &str,
    referer: &str,
    request: &Value,
) -> Vec<VideoStream> {
    let embed = absolute_url(embed);
    if embed.contains(".m3u8") {
        return parse_hls(&embed, name, lang, referer, request);
    }
    let body = fetch(&embed, "", referer);
    if let Some(src) = first_media_url(&body).map(|src| absolute_remote(&src, &embed)) {
        if src.contains(".m3u8") {
            parse_hls(&src, name, lang, &embed, request)
        } else {
            vec![stream(&src, name, lang, "direct", &embed, false)]
        }
    } else {
        vec![external_stream(&embed, name, lang, referer)]
    }
}

fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)"#,
        r#"src\s*:\s*["']([^"']+)"#,
        r#"<source[^>]+src=["']([^"']+)"#,
    ]
    .into_iter()
    .find_map(|pattern| {
        Regex::new(pattern)
            .ok()?
            .captures(body)?
            .get(1)
            .map(|m| m.as_str().replace("\\/", "/"))
    })
}

fn parse_hls(
    master: &str,
    name: &str,
    lang: &str,
    referer: &str,
    request: &Value,
) -> Vec<VideoStream> {
    let body = client(referer)
        .get(master)
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    let mut streams = Vec::new();
    let mut quality = "auto".to_string();
    for line in body.lines() {
        if line.starts_with("#EXT-X-STREAM-INF") {
            quality = line
                .split("RESOLUTION=")
                .nth(1)
                .and_then(|v| v.split('x').nth(1))
                .and_then(|v| v.split(',').next())
                .map(|v| format!("{v}p"))
                .unwrap_or_else(|| "auto".to_string());
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            streams.push(stream(
                &absolute_remote(line.trim(), master),
                name,
                lang,
                &quality,
                referer,
                true,
            ));
        }
    }
    if streams.is_empty() {
        streams.push(stream(master, name, lang, "auto", referer, true));
    }
    sort_streams(&mut streams, request);
    streams
}

fn stream(
    target: &str,
    name: &str,
    lang: &str,
    quality: &str,
    referer: &str,
    hls: bool,
) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{lang} {name} {quality}")),
        quality: Some(format!("{lang} {name} {quality}")),
        format: Some(if hls { "hls" } else { "mp4" }.to_string()),
        is_hls: hls,
        stream_kind: Some(if hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn external_stream(target: &str, name: &str, lang: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{lang} {name} External")),
        quality: Some(format!("{lang} {name}")),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let server = pref(request, "preferred_server", "Voe").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    let lang = pref(request, "preferred_language", "[LAT]");
    streams.sort_by_key(|stream| {
        let value = stream
            .quality
            .clone()
            .or_else(|| stream.name.clone())
            .unwrap_or_default();
        let lower = value.to_ascii_lowercase();
        (
            value.contains(&lang),
            lower.contains(&server),
            value.contains(&quality),
            quality_rank(&value),
        )
    });
    streams.reverse();
}

fn selector(input: &str) -> Selector {
    Selector::parse(input).unwrap()
}

fn select_text_doc(doc: &Html, sel: &str) -> Option<String> {
    doc.select(&selector(sel))
        .next()
        .map(text)
        .filter(|value| !value.is_empty())
}

fn select_texts_doc(doc: &Html, sel: &str) -> Vec<String> {
    doc.select(&selector(sel))
        .map(text)
        .filter(|value| !value.is_empty())
        .collect()
}

fn select_attr_doc(doc: &Html, sel: &str, name: &str) -> Option<String> {
    doc.select(&selector(sel))
        .next()
        .and_then(|el| el.value().attr(name))
        .map(ToString::to_string)
}

fn select_text(el: ElementRef<'_>, sel: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .map(text)
        .filter(|value| !value.is_empty())
}

fn select_attr(el: ElementRef<'_>, sel: &str, name: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .and_then(|el| el.value().attr(name))
        .map(ToString::to_string)
}

fn attr(el: &ElementRef<'_>, name: &str) -> Option<String> {
    el.value().attr(name).map(ToString::to_string)
}

fn text(el: ElementRef<'_>) -> String {
    el.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
        .filter(|path| path.starts_with('/'))
        .map(path_key)
}

fn path_key(input: &str) -> String {
    format!(
        "/{}",
        input
            .strip_prefix(BASE_URL)
            .unwrap_or(input)
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

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
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

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn quality_rank(input: &str) -> i32 {
    Regex::new(r#"(\d+)"#)
        .unwrap()
        .captures(input)
        .and_then(|cap| cap.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0)
}

fn host_name(input: &str) -> String {
    input
        .split("://")
        .nth(1)
        .unwrap_or(input)
        .split('/')
        .next()
        .unwrap_or("External")
        .replace("www.", "")
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("SeriesKao")
        .replace('-', " ")
}

fn outer_call<'a>(input: &'a str, prefix: &str) -> Option<&'a str> {
    let trimmed = input.trim();
    if !trimmed.starts_with(prefix) || !trimmed.ends_with(')') {
        return None;
    }
    let start = trimmed.find('(')?;
    let end = trimmed.rfind(')')?;
    (end > start).then(|| trimmed[start + 1..end].trim())
}

fn trim_quotes(input: &str) -> &str {
    let trimmed = input.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        &trimmed[1..trimmed.len().saturating_sub(1)]
    } else {
        trimmed
    }
}

fn percent_decode(input: &str) -> Option<String> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn decode_base64_any(input: &str) -> Option<Vec<u8>> {
    let padded = pad_base64(input.trim());
    general_purpose::STANDARD
        .decode(&padded)
        .or_else(|_| general_purpose::URL_SAFE.decode(&padded))
        .or_else(|_| general_purpose::URL_SAFE_NO_PAD.decode(input.trim()))
        .ok()
}

fn pad_base64(input: &str) -> String {
    let padding = (4 - input.len() % 4) % 4;
    format!("{input}{}", "=".repeat(padding))
}

#[derive(Deserialize)]
struct Item {
    video_language: Option<String>,
    #[serde(default, rename = "sortedEmbeds")]
    sorted_embeds: Vec<Embed>,
}

#[derive(Deserialize)]
struct Embed {
    link: Option<String>,
    #[serde(rename = "type")]
    embed_type: Option<String>,
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<a class="poster-card" href="/serie/sample" title="VER Sample"><div class="poster-card__title">Sample</div><img src="/w154/sample.jpg"></a>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="m-b-5">Sample</h1><div class="col-sm-4"><div class="text-large">Sample synopsis.</div></div><div class="episodes-list"><a class="episode-item" href="/ver/sample-1"><span class="episode-number">1</span><span class="episode-title">Piloto</span></a></div>"#;
const WATCH_FIXTURE: &str =
    r#"<script>var videoSources = ["https://example.invalid/source"];</script>"#;
