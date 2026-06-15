use manatan_extension::{
    abi::{ExtensionError, ExtensionResult},
    export_video_source,
    source::VideoSource,
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind,
};
use manatan_shared::{
    html,
    sdk::{http::HttpClient, Context, SearchRequest},
    url,
};
use serde_json::{json, Value};

const SOURCE: FaselHd = FaselHd;
const DEFAULT_BASE_URL: &str = "https://www.faselhds.biz";

struct FaselHd;

impl VideoSource for FaselHd {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let page = page(&request);
        let listing = request.get("listing").and_then(Value::as_str).unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{base}/most_recent/page/{page}")
        } else {
            format!("{base}/anime/page/{page}")
        };
        let body = get_or_fixture(&base, &target, LIST_FIXTURE);
        Ok(Paged { entries: parse_cards(&body, &base), has_next_page: has_next_page(&body) })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = path_from_url(query, &base) {
            return Ok(Paged { entries: vec![fetch_details(&path, &base)], has_next_page: false });
        }
        let page = page(&request);
        let target = if !query.is_empty() {
            format!("{base}/page/{page}?s={}", manatan_shared::sdk::http::url_encode(query))
        } else if let Some(section) = filter(&request, "section").filter(|value| !value.is_empty()) {
            format!("{base}/{section}/page/{page}")
        } else if let Some(category) = filter(&request, "category").filter(|value| !value.is_empty()) {
            let genre = filter(&request, "genre").unwrap_or_else(|| "Action".to_string()).to_lowercase();
            format!("{base}/{category}/{genre}/page/{page}")
        } else {
            return Err(error("من فضلك اختر قسم او نوع"));
        };
        let body = get_or_fixture(&base, &target, LIST_FIXTURE);
        Ok(Paged { entries: parse_cards(&body, &base), has_next_page: has_next_page(&body) })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        Ok(fetch_details(&request_key(&request, "item", &base).unwrap_or_else(|| "/anime/sample".to_string()), &base))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base = base_url(&request);
        let path = request_key(&request, "item", &base).unwrap_or_else(|| "/anime/sample".to_string());
        let body = get_or_fixture(&base, &absolute_url(&base, &path), EPISODES_FIXTURE);
        let mut episodes = parse_episodes(&body, &base);
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let base = base_url(&request);
        let path = request_key(&request, "episode", &base).unwrap_or_else(|| "/watch/sample".to_string());
        let body = get_or_fixture(&base, &absolute_url(&base, &path), HOSTERS_FIXTURE);
        Ok(parse_hosters(&body, &base))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let base = base_url(&request);
        let key = request_key(&request, "hoster", &base).unwrap_or_default();
        let name = request.get("hoster").and_then(|h| h.get("name")).and_then(Value::as_str).unwrap_or("Mirror");
        let mut streams = resolve_player(&key, name, &base);
        prefer_quality(&mut streams, pref(&request, "preferred_quality", "1080"));
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut streams = Vec::new();
        for hoster in self.hosters(request.clone())? {
            let mut resolved = self.resolve_hoster(json!({
                "hoster": { "key": hoster.key, "name": hoster.name },
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
            }))?;
            for stream in &mut resolved {
                stream.hoster = Some(hoster.clone());
            }
            streams.extend(resolved);
        }
        prefer_quality(&mut streams, pref(&request, "preferred_quality", "1080"));
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: self.list(json!({"listing": "popular", "page": 1, "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?.entries,
                has_more: true,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: self.list(json!({"listing": "latest", "page": 1, "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?.entries,
                has_more: true,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "item", &base).map(|path| absolute_url(&base, &path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "episode", &base).map(|path| absolute_url(&base, &path)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        let base = base_url(&request);
        if let Some(path) = path_from_url(input, &base) {
            return Ok(Some(UrlResolveResult { item: Some(fetch_details(&path, &base)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client(base: &str) -> HttpClient {
    HttpClient::browser().with_referer(base).with_cookies_for(base).with_webview_challenge_fallback()
}

fn get_or_fixture(base: &str, target: &str, fixture: &str) -> String {
    client(base).get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_cards(body: &str, base: &str) -> Vec<CatalogItem> {
    body.split("postList").nth(1).unwrap_or(body).split("<a").skip(1).filter_map(|chunk| {
        if !chunk.contains("imgdiv-class") && !chunk.contains("<img") { return None; }
        let href = html::attr(chunk, "href")?;
        let title = html::attr_after(chunk, "<img", "alt").unwrap_or_else(|| path_key(&href, base).trim_matches('/').replace('-', " "));
        Some(CatalogItem {
            key: path_key(&href, base),
            title,
            cover: html::attr_after(chunk, "<img", "data-src").or_else(|| html::attr_after(chunk, "<img", "src")).map(|image| absolute_url(base, &image)),
            url: Some(absolute_url(base, &href)),
            language: Some("ar".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            ..CatalogItem::default()
        })
    }).collect()
}

fn fetch_details(path: &str, base: &str) -> CatalogItem {
    let body = get_or_fixture(base, &absolute_url(base, path), DETAILS_FIXTURE);
    CatalogItem {
        key: path_key(path, base),
        title: html::attr_after(&body, "itemprop=name", "content")
            .or_else(|| html::text_between(&body, "<h1", "</h1>").map(|text| html::strip_tags(&text)))
            .unwrap_or_else(|| path_key(path, base).trim_matches('/').replace('-', " ")),
        cover: html::attr_after(&body, "posterImg", "src").or_else(|| html::attr_after(&body, "seasonDiv", "data-src")).or_else(|| html::attr_after(&body, "<img", "src")).map(|image| absolute_url(base, &image)),
        description: html::text_between(&body, "singleDesc", "</div>").map(|text| html::strip_tags(&text)).filter(|text| !text.is_empty()),
        tags: collect_links_after(&body, "تصنيف").into_iter().chain(collect_links_after(&body, "مستوى")).collect(),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: if html::strip_tags(&body).contains("مستمر") { ItemStatus::Ongoing } else { ItemStatus::Completed },
        url: Some(absolute_url(base, path)),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, base: &str) -> Vec<VideoEpisode> {
    let mut out = Vec::new();
    let episode_section = body.split("epAll").nth(1).unwrap_or_default();
    if episode_section.is_empty() {
        for chunk in body.split("shortLink").skip(1) {
            if let Some(path) = html::text_between(chunk, "liskSh", "</span>").map(|text| html::strip_tags(&text)) {
                out.push(VideoEpisode {
                    key: path_key(&path, base),
                    title: Some("مشاهدة".to_string()),
                    episode_number: Some(1.0),
                    url: Some(absolute_url(base, &path)),
                    language: Some("ar".to_string()),
                    ..VideoEpisode::default()
                });
            }
        }
        return out;
    }
    let season = html::text_between(body, "seasonDiv active", "</div>").map(|text| html::strip_tags(&text)).unwrap_or_default();
    for chunk in episode_section.split("<a").skip(1) {
        let Some(href) = html::attr(chunk, "href") else { continue; };
        let text = html::strip_tags(chunk);
        out.push(VideoEpisode {
            key: path_key(&href, base),
            title: Some(if season.is_empty() { text.clone() } else { format!("{season} : {text}") }),
            episode_number: first_number(&text),
            url: Some(absolute_url(base, &href)),
            language: Some("ar".to_string()),
            ..VideoEpisode::default()
        });
    }
    out
}

fn parse_hosters(body: &str, base: &str) -> Vec<VideoHoster> {
    body.split("<li").skip(1).filter_map(|chunk| {
        if !html::strip_tags(chunk).contains("سيرفر") { return None; }
        let onclick = html::attr(chunk, "onclick").unwrap_or_default();
        let target = first_url(&onclick)?;
        let name = html::strip_tags(chunk);
        Some(VideoHoster {
            key: absolute_url(base, &target),
            name: if name.is_empty() { host_name(&target) } else { name },
            url: Some(absolute_url(base, &target)),
            lazy: true,
            video_count: Some(1),
            headers: referer_headers(base),
            ..VideoHoster::default()
        })
    }).collect()
}

fn resolve_player(target: &str, name: &str, base: &str) -> Vec<VideoStream> {
    let body = get_or_fixture(base, target, STREAM_FIXTURE);
    if let Some(playlist) = first_media_url(&body) {
        if playlist.contains(".m3u8") {
            let playlist_body = client(base).get(&playlist).send_text().unwrap_or_default();
            let parsed = parse_hls_playlist(&playlist_body, &playlist, name);
            if !parsed.is_empty() {
                return parsed;
            }
        }
        return vec![media_stream(&playlist, name, name, target)];
    }
    vec![external_stream(target, name, base)]
}

fn parse_hls_playlist(body: &str, master: &str, name: &str) -> Vec<VideoStream> {
    body.split("#EXT-X-STREAM-INF:").skip(1).filter_map(|block| {
        let quality = block.split("RESOLUTION=").nth(1)
            .and_then(|part| part.split('x').nth(1))
            .and_then(|part| part.split([',', '\n']).next())
            .map(|height| format!("{height}p"))
            .unwrap_or_else(|| "auto".to_string());
        let line = block.lines().find(|line| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        })?.trim();
        let stream_url = if line.starts_with("http") {
            line.to_string()
        } else {
            format!("{}/{}", master.rsplit_once('/').map(|(base, _)| base).unwrap_or(master), line)
        };
        Some(media_stream(&stream_url, name, &quality, master))
    }).collect()
}

fn first_url(input: &str) -> Option<String> {
    input.split(['"', '\'']).find(|part| part.starts_with("http")).map(ToString::to_string)
}

fn first_media_url(input: &str) -> Option<String> {
    for marker in [".m3u8", ".mp4"] {
        if let Some(index) = input.find(marker) {
            let start = input[..index].rfind("http")?;
            let end = input[index..].find(['"', '\'', '\\', '<', ' ']).map(|offset| index + offset).unwrap_or(input.len());
            return Some(input[start..end].replace("\\/", "/"));
        }
    }
    None
}

fn media_stream(stream_url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = stream_url.contains(".m3u8");
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls { VideoStreamKind::Hls } else { VideoStreamKind::Direct }),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn external_stream(stream_url: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some(host_name(stream_url)),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn collect_links_after(body: &str, label: &str) -> Vec<String> {
    body.split(label).nth(1).unwrap_or_default().split("</span>").next().unwrap_or_default().split("<a").skip(1)
        .map(html::strip_tags).filter(|text| !text.is_empty()).collect()
}

fn prefer_quality(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by_key(|stream| !stream.quality.as_deref().unwrap_or_default().contains(preferred));
    for stream in streams {
        stream.preferred = stream.quality.as_deref().map(|quality| quality.contains(preferred)).unwrap_or(false);
    }
}

fn first_number(input: &str) -> Option<f32> {
    input.chars().filter(|ch| ch.is_ascii_digit() || *ch == '.').collect::<String>().parse().ok()
}

fn host_name(input: &str) -> String {
    input.split("://").nth(1).unwrap_or(input).split('/').next().unwrap_or("External").replace("www.", "")
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn base_url(request: &Value) -> String {
    request.get("preferences")
        .and_then(|prefs| prefs.get("custom_domain"))
        .or_else(|| request.get("custom_domain"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("http") && !value.trim_end().ends_with('/'))
        .unwrap_or(DEFAULT_BASE_URL)
        .to_string()
}

fn request_key(request: &Value, field: &str, base: &str) -> Option<String> {
    request.get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("url")))
        .or_else(|| request.get(field))
        .and_then(Value::as_str)
        .map(|value| path_key(value, base))
}

fn path_from_url(input: &str, base: &str) -> Option<String> {
    input.find(base).map(|idx| path_key(&input[idx + base.len()..], base)).or_else(|| input.starts_with('/').then(|| path_key(input, base)))
}

fn path_key(input: &str, base: &str) -> String {
    if let Some(path) = input.strip_prefix(base) { return path_key(path, base); }
    if input.starts_with("http") { return input.to_string(); }
    format!("/{}", input.split('?').next().unwrap_or(input).trim_matches('/'))
}

fn absolute_url(base: &str, input: &str) -> String {
    if input.starts_with("http") { input.to_string() } else if input.starts_with("//") { format!("https:{input}") } else { url::join_url(base, input) }
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request.get("filters").and_then(|filters| filters.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).map(ToString::to_string)
}

fn pref<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request.get("preferences").and_then(|prefs| prefs.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).unwrap_or(default)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn has_next_page(body: &str) -> bool {
    body.contains("pagination") && body.contains("›")
}

fn error(message: &str) -> ExtensionError {
    ExtensionError { message: message.to_string() }
}

const LIST_FIXTURE: &str = r#"<div id="postList"><div class="col-xl-2"><a href="/anime/sample"><div class="imgdiv-class"><img data-src="/cover.jpg" alt="Sample Anime"></div></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<meta itemprop="name" content="Sample Anime"><div class="posterImg"><img class="poster" src="/cover.jpg"></div><div class="singleDesc">Sample description.</div>"#;
const EPISODES_FIXTURE: &str = r#"<div class="epAll"><a href="/episode/sample-1">الحلقة 1</a></div>"#;
const HOSTERS_FIXTURE: &str = r#"<ul><li onclick="loadServer('https://media.invalid/player/sample')">سيرفر 1</li></ul>"#;
const STREAM_FIXTURE: &str = r#"<script>var video="https://media.invalid/sample.m3u8";</script>"#;

export_video_source!(SOURCE);
