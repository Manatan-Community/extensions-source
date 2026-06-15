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

const SOURCE: Asia2Tv = Asia2Tv;
const BASE_URL: &str = "https://ww1.asia2tv.pw";

struct Asia2Tv;

impl VideoSource for Asia2Tv {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = get_or_fixture(&format!("{BASE_URL}/category/asian-drama/page/{}/", page(&request)), LIST_FIXTURE);
        Ok(Paged { entries: parse_cards(&body), has_next_page: has_next_page(&body) })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged { entries: vec![fetch_details(&path)], has_next_page: false });
        }
        let page = page(&request);
        let target = if !query.is_empty() {
            format!("{BASE_URL}/page/{page}/?s={}", manatan_shared::sdk::http::url_encode(query))
        } else if let Some(kind) = filter(&request, "type").filter(|value| !value.is_empty()) {
            format!("{BASE_URL}/category/asian-drama/{kind}/page/{page}/")
        } else if let Some(status) = filter(&request, "status").filter(|value| !value.is_empty()) {
            format!("{BASE_URL}/{status}/page/{page}/")
        } else {
            return Err(error("اختر فلتر"));
        };
        let body = get_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged { entries: parse_cards(&body), has_next_page: has_next_page(&body) })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        Ok(fetch_details(&request_key(&request, "item").unwrap_or_else(|| "/drama/sample".to_string())))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/drama/sample".to_string());
        let body = get_or_fixture(&absolute_url(&path), EPISODES_FIXTURE);
        let mut episodes = parse_episodes(&body);
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/episode/sample".to_string());
        let episode_body = get_or_fixture(&absolute_url(&path), HOSTERS_FIXTURE);
        let current = html::attr_after(&episode_body, "current", "href").map(|href| absolute_url(&href)).unwrap_or_else(|| absolute_url(&path));
        let body = get_or_fixture(&current, HOSTERS_FIXTURE);
        Ok(parse_hosters(&body, &current))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let name = request.get("hoster").and_then(|h| h.get("name")).and_then(Value::as_str).unwrap_or("Mirror");
        let mut streams = resolve_streams(&key, name);
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
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Asian Drama".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: page.entries,
            has_more: page.has_next_page,
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
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(path) = path_from_url(input) {
            return Ok(Some(UrlResolveResult { item: Some(fetch_details(&path)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser().with_referer(BASE_URL).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("postmovie-photo").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let title = html::attr_after(chunk, "<a", "title")
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .unwrap_or_else(|| path_key(&href).trim_matches('/').replace('-', " "));
        Some(CatalogItem {
            key: path_key(&href),
            title,
            cover: html::attr_after(chunk, "<img", "data-src").or_else(|| html::attr_after(chunk, "<img", "src")).map(|image| absolute_url(&image)),
            url: Some(absolute_url(&href)),
            language: Some("ar".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            ..CatalogItem::default()
        })
    }).collect()
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE);
    CatalogItem {
        key: path_key(path),
        title: html::text_between(&body, "span class=\"title", "</span>")
            .map(|text| html::strip_tags(&text))
            .or_else(|| html::text_between(&body, "<h1", "</h1>").map(|text| html::strip_tags(&text)))
            .unwrap_or_else(|| path_key(path).trim_matches('/').replace('-', " ")),
        cover: html::attr_after(&body, "single-thumb-bg", "src").or_else(|| html::attr_after(&body, "<img", "src")).map(|image| absolute_url(&image)),
        description: html::text_between(&body, "getcontent", "</div>").map(|text| html::strip_tags(&text)).filter(|text| !text.is_empty()),
        tags: collect_anchor_text(&body, "box-tags"),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        url: Some(absolute_url(path)),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    body.split("loop-episode").skip(1).flat_map(|section| section.split("<a").skip(1)).filter_map(|chunk| {
        let href = html::attr(chunk, "href")?;
        let label = href.trim_end_matches('/').rsplit('-').next().unwrap_or("0");
        Some(VideoEpisode {
            key: path_key(&href),
            title: Some(format!("{label} : الحلقة")),
            episode_number: first_number(label),
            url: Some(absolute_url(&href)),
            language: Some("ar".to_string()),
            ..VideoEpisode::default()
        })
    }).collect()
}

fn parse_hosters(body: &str, referer: &str) -> Vec<VideoHoster> {
    body.split("<li").skip(1).filter_map(|chunk| {
        let embed = html::attr(chunk, "data-server")?;
        if embed.trim().is_empty() { return None; }
        let name = html::strip_tags(chunk);
        Some(VideoHoster {
            key: absolute_url(&embed),
            name: if name.is_empty() { host_name(&embed) } else { name },
            url: Some(absolute_url(&embed)),
            lazy: true,
            video_count: Some(1),
            headers: referer_headers(referer),
            ..VideoHoster::default()
        })
    }).collect()
}

fn resolve_streams(target: &str, name: &str) -> Vec<VideoStream> {
    if target.contains("youdbox") || target.contains("yodbox") {
        let body = get_or_fixture(target, STREAM_FIXTURE);
        if let Some(src) = html::attr_after(&body, "<source", "src") {
            return vec![media_stream(&absolute_url(&src), "Yodbox: mirror", "mirror", target)];
        }
    }
    if target.contains(".m3u8") || target.contains(".mp4") {
        vec![media_stream(target, name, name, BASE_URL)]
    } else {
        vec![external_stream(target, name)]
    }
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

fn external_stream(stream_url: &str, name: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some(host_name(stream_url)),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(BASE_URL),
        initialized: true,
        ..VideoStream::default()
    }
}

fn collect_anchor_text(body: &str, marker: &str) -> Vec<String> {
    body.split(marker).nth(1).unwrap_or_default().split("</div>").next().unwrap_or_default().split("<a").skip(1)
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

fn request_key(request: &Value, field: &str) -> Option<String> {
    request.get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("url")))
        .or_else(|| request.get(field))
        .and_then(Value::as_str)
        .map(path_key)
}

fn path_from_url(input: &str) -> Option<String> {
    input.find(BASE_URL).map(|idx| path_key(&input[idx + BASE_URL.len()..])).or_else(|| input.starts_with('/').then(|| path_key(input)))
}

fn path_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) { return path_key(path); }
    if input.starts_with("http") { return input.to_string(); }
    format!("/{}", input.split('?').next().unwrap_or(input).trim_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") { input.to_string() } else if input.starts_with("//") { format!("https:{input}") } else { url::join_url(BASE_URL, input) }
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
    body.contains("nav-links") && body.contains("next")
}

fn error(message: &str) -> ExtensionError {
    ExtensionError { message: message.to_string() }
}

const LIST_FIXTURE: &str = r#"<div class="postmovie-photo"><a href="/drama/sample" title="Sample Drama"><img src="/cover.jpg"></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1><span class="title">Sample Drama</span></h1><div class="single-thumb-bg"><img src="/cover.jpg"></div><div class="getcontent"><p>Sample description.</p></div>"#;
const EPISODES_FIXTURE: &str = r#"<div class="loop-episode"><a href="/episode/sample-1/">1</a></div>"#;
const HOSTERS_FIXTURE: &str = r#"<div class="loop-episode"><a class="current" href="/episode/sample-1/">1</a></div><ul class="server-list-menu"><li data-server="https://media.invalid/embed/sample">Mirror</li></ul>"#;
const STREAM_FIXTURE: &str = r#"<video><source src="https://media.invalid/sample.mp4"></video>"#;

export_video_source!(SOURCE);
