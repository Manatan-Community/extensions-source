use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{SearchRequest, http::HttpClient},
    url,
    video::referer_headers,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;

const SOURCE: AnimeSama = AnimeSama;
const BASE_URL: &str = "https://anime-sama.to";
const VOICES: [&str; 9] = ["vostfr", "vf", "vf1", "vf2", "va", "vcn", "vj", "vkr", "vqc"];

struct AnimeSama;

impl VideoSource for AnimeSama {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = fetch(BASE_URL, LIST_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let selector_value = if listing(&request) == "latest" {
            "#containerAjoutsAnimes > div a, #containerAjoutsAnimes a"
        } else {
            "#containerPepites > div a, #containerPepites a"
        };
        let entries = doc
            .select(&selector(selector_value))
            .filter_map(card)
            .collect::<Vec<_>>();
        Ok(Paged {
            entries,
            has_next_page: false,
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
        let page = page(&request);
        let mut target = format!(
            "{BASE_URL}/catalogue/?search={}&page={page}",
            url::query_escape(query)
        );
        for (filter_key, query_key) in [("type", "type[]"), ("langue", "langue[]"), ("genre", "genre[]")] {
            if let Some(value) = filter(&request, filter_key).filter(|v| !v.is_empty()) {
                target.push('&');
                target.push_str(query_key);
                target.push('=');
                target.push_str(&url::query_escape(&value));
            }
        }
        let body = fetch(&target, SEARCH_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        Ok(Paged {
            entries: doc.select(&selector("#list_catalog > div a, #list_catalog a")).filter_map(card).collect(),
            has_next_page: doc.select(&selector("#list_pagination a:last-child")).next().is_some(),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/catalogue/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/catalogue/sample".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        let seasons = parse_seasons(&body);
        let mut episodes = Vec::new();
        for (season_title, stem) in seasons {
            let mut voice_lists = Vec::new();
            let voice_stems = if stem.contains('/') {
                vec![stem.clone()]
            } else {
                VOICES.iter().map(|voice| format!("{stem}/{voice}")).collect()
            };
            for voice_stem in voice_stems {
                let voice = VOICES
                    .iter()
                    .find(|voice| voice_stem.contains(**voice))
                    .copied()
                    .unwrap_or("vostfr");
                let js = fetch(
                    &format!("{}/{}/episodes.js", absolute_url(&path).trim_end_matches('/'), voice_stem.trim_matches('/')),
                    "",
                    &absolute_url(&path),
                );
                let players = parse_episode_arrays(&js);
                if !players.is_empty() {
                    voice_lists.push((voice.to_string(), players));
                }
            }
            let max_len = voice_lists.iter().map(|(_, list)| list.len()).max().unwrap_or(0);
            for index in 0..max_len {
                let grouped = voice_lists
                    .iter()
                    .map(|(voice, list)| {
                        let players = list.get(index).cloned().unwrap_or_default();
                        serde_json::json!({ "voice": voice, "players": players })
                    })
                    .collect::<Vec<_>>();
                episodes.push(VideoEpisode {
                    key: serde_json::to_string(&grouped).unwrap_or_else(|_| "[]".to_string()),
                    title: Some(format!("{season_title} Episode {}", index + 1)),
                    episode_number: Some((index + 1) as f32),
                    language: Some("fr".to_string()),
                    ..VideoEpisode::default()
                });
            }
        }
        if episodes.is_empty() {
            episodes.push(VideoEpisode {
                key: "[]".to_string(),
                title: Some("Episode 1".to_string()),
                episode_number: Some(1.0),
                language: Some("fr".to_string()),
                ..VideoEpisode::default()
            });
        }
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let raw = request_key(&request, "episode").unwrap_or_else(|| "[]".to_string());
        let groups = serde_json::from_str::<Vec<Value>>(&raw).unwrap_or_default();
        let mut streams = Vec::new();
        for group in groups {
            let voice = group.get("voice").and_then(Value::as_str).unwrap_or("vostfr");
            for player in group.get("players").and_then(Value::as_array).into_iter().flatten() {
                let Some(link) = player.as_str() else {
                    continue;
                };
                streams.extend(resolve_link(link, &format!("{} {}", server_label(link), voice.to_uppercase()), BASE_URL, &request));
            }
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
                title: "Pepites".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Ajouts animes".to_string(),
                entries: latest.entries,
                has_more: false,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|path| absolute_url(&path)))
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
        .with_header("Origin", BASE_URL)
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

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href")?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_text(el, "h1, h2, h3, p, span")
            .or_else(|| select_attr(el, "img", "alt"))
            .unwrap_or_else(|| title_from_path(&key)),
        cover: select_attr(el, "img", "src").map(|src| absolute_url(&src)),
        url: Some(absolute_url(&key)),
        language: Some("fr".to_string()),
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
        title: select_text_doc(&doc, "#titreOeuvre, h1").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, "#coverOeuvre, img", "src").map(|src| absolute_url(&src)),
        description: select_text_doc(&doc, "h2:contains(synopsis) + p, .synopsis, p"),
        tags: doc
            .select(&selector("h2:contains(genres) + a, .genres a"))
            .map(text)
            .filter(|v| !v.is_empty())
            .collect(),
        url: Some(absolute_url(path)),
        language: Some("fr".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_seasons(body: &str) -> Vec<(String, String)> {
    let script = Regex::new(r#"panneauAnime\("([^"]+)",\s*"([^"]+)""#).unwrap();
    let seasons = script
        .captures_iter(body)
        .map(|cap| (cap[1].to_string(), cap[2].to_string()))
        .collect::<Vec<_>>();
    if seasons.is_empty() {
        vec![("Saison 1".to_string(), "vostfr".to_string())]
    } else {
        seasons
    }
}

fn parse_episode_arrays(js: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let re = Regex::new(r#"(?s)eps\d+\s*=\s*(\[[^;]+])"#).unwrap();
    for cap in re.captures_iter(js) {
        let json = cap[1].replace('\'', "\"");
        if let Ok(mut arrays) = serde_json::from_str::<Vec<Vec<String>>>(&json) {
            out.append(&mut arrays);
        }
    }
    if out.is_empty() {
        let urls = Regex::new(r#"https?://[^"'\\\]]+"#)
            .unwrap()
            .find_iter(js)
            .map(|m| m.as_str().to_string())
            .collect::<Vec<_>>();
        if !urls.is_empty() {
            out.push(urls);
        }
    }
    out
}

fn resolve_link(link: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    if link.contains(".m3u8") {
        return parse_hls(link, name, referer, request);
    }
    if link.contains(".mp4") || link.contains(".webm") {
        return vec![stream(link, name, &preference(request, "preferred_quality", "auto"), referer)];
    }
    let body = fetch(link, "", referer);
    if let Some(media) = extract_media_url(&body) {
        return if media.contains(".m3u8") {
            parse_hls(&media, name, link, request)
        } else {
            vec![stream(&media, name, "auto", link)]
        };
    }
    vec![external(link, name, referer)]
}

fn parse_hls(url: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = fetch(url, "", referer);
    if !body.contains("#EXT-X-STREAM-INF") {
        return vec![stream(url, name, &preference(request, "preferred_quality", "auto"), referer)];
    }
    body.split("#EXT-X-STREAM-INF:")
        .skip(1)
        .filter_map(|block| {
            let quality = block
                .split("RESOLUTION=")
                .nth(1)
                .and_then(|v| v.split('x').nth(1))
                .and_then(|v| v.split([',', '\n']).next())
                .map(|v| format!("{v}p"))
                .unwrap_or_else(|| "auto".to_string());
            let line = block.lines().find(|line| !line.trim().is_empty() && !line.starts_with('#'))?;
            Some(stream(&absolute_or(line.trim(), url), name, &quality, referer))
        })
        .collect()
}

fn stream(url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = url.contains(".m3u8");
    VideoStream {
        url: url.to_string(),
        name: Some(format!("{name} - {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls { VideoStreamKind::Hls } else { VideoStreamKind::Direct }),
        headers: referer_headers(referer),
        preferred: quality.contains("1080"),
        initialized: true,
        ..VideoStream::default()
    }
}

fn external(url: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: url.to_string(),
        name: Some(name.to_string()),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        preferred: true,
        initialized: true,
        ..VideoStream::default()
    }
}

fn extract_media_url(body: &str) -> Option<String> {
    for marker in ["file:\"", "file: \"", "source:\"", "src: \""] {
        if let Some(value) = body.split(marker).nth(1) {
            let url = value.split(['"', '\'']).next()?.replace("\\/", "/");
            if url.contains(".m3u8") || url.contains(".mp4") {
                return Some(url);
            }
        }
    }
    None
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let server = preference(request, "preferred_server", "");
    let voice = preference(request, "preferred_voice", "");
    let quality = preference(request, "preferred_quality", "1080");
    streams.sort_by_key(|stream| {
        let name = stream.name.as_deref().unwrap_or_default().to_ascii_lowercase();
        let q = stream.quality.as_deref().unwrap_or_default();
        (name.contains(&server), name.contains(&voice), q.contains(&quality))
    });
    streams.reverse();
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).unwrap()
}

fn select_text(el: ElementRef<'_>, selector_value: &str) -> Option<String> {
    el.select(&selector(selector_value)).next().map(text).filter(|value| !value.is_empty())
}

fn select_text_doc(doc: &Html, selector_value: &str) -> Option<String> {
    doc.select(&selector(selector_value)).next().map(text).filter(|value| !value.is_empty())
}

fn select_attr(el: ElementRef<'_>, selector_value: &str, name: &str) -> Option<String> {
    el.select(&selector(selector_value)).next().and_then(|e| attr(&e, name))
}

fn select_attr_doc(doc: &Html, selector_value: &str, name: &str) -> Option<String> {
    doc.select(&selector(selector_value)).next().and_then(|e| attr(&e, name))
}

fn attr(el: &ElementRef<'_>, name: &str) -> Option<String> {
    el.value().attr(name).map(|v| v.to_string()).filter(|v| !v.is_empty())
}

fn text(el: ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn listing(request: &Value) -> &str {
    request.get("listing").or_else(|| request.get("listingId")).and_then(Value::as_str).unwrap_or("popular")
}

fn page(request: &Value) -> u32 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1) as u32
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request.get("filters").and_then(|f| f.get(key)).and_then(Value::as_str).map(ToString::to_string)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| value.as_str().or_else(|| value.get("key").and_then(Value::as_str)))
        .map(ToString::to_string)
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(map) = next.as_object_mut() {
        map.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

fn preference(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn path_from_url(input: &str) -> Option<String> {
    input.strip_prefix(BASE_URL).map(path_key).filter(|p| p.starts_with("/catalogue/"))
}

fn path_key(input: &str) -> String {
    let value = input.split('#').next().unwrap_or(input).split('?').next().unwrap_or(input);
    if value.starts_with("http") {
        format!("/{}", value.split('/').skip(3).collect::<Vec<_>>().join("/")).trim_end_matches('/').to_string()
    } else {
        format!("/{}", value.trim_start_matches('/')).trim_end_matches('/').to_string()
    }
}

fn absolute_url(path: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else {
        format!("{BASE_URL}/{}", path.trim_start_matches('/'))
    }
}

fn absolute_or(path: &str, base: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else {
        let prefix = base.rsplit_once('/').map(|(p, _)| p).unwrap_or(BASE_URL);
        format!("{}/{}", prefix, path.trim_start_matches('/'))
    }
}

fn title_from_path(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).replace(['-', '_'], " ")
}

fn server_label(link: &str) -> &'static str {
    let lower = link.to_ascii_lowercase();
    if lower.contains("sibnet") {
        "Sibnet"
    } else if lower.contains("sendvid") {
        "Sendvid"
    } else if lower.contains("vidmoly") {
        "VidMoly"
    } else if lower.contains("vk.") {
        "VK"
    } else {
        "External"
    }
}

const LIST_FIXTURE: &str = r#"<div id="containerPepites"><div><a href="/catalogue/sample"><img src="/sample.jpg"><p>Sample</p></a></div></div>"#;
const SEARCH_FIXTURE: &str = r#"<div id="list_catalog"><div><a href="/catalogue/sample"><img src="/sample.jpg"><p>Sample</p></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 id="titreOeuvre">Sample</h1><img id="coverOeuvre" src="/sample.jpg"><h2>synopsis</h2><p>Synopsis</p><script>panneauAnime("Saison 1", "vostfr")</script>"#;

export_video_source!(SOURCE);
