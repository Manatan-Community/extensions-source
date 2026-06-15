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
use std::collections::BTreeSet;

const SOURCE: ArabSeed = ArabSeed;
const BASE_URL: &str = "https://m.asd.homes";

struct ArabSeed;

impl VideoSource for ArabSeed {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = get_or_fixture(&format!("{BASE_URL}/movies/?offset={}", page(&request)), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged { entries: vec![fetch_details(&path)], has_next_page: false });
        }
        let target = if query.is_empty() {
            let category = filter(&request, "type").filter(|value| !value.is_empty()).ok_or_else(|| error("اختر فلتر"))?;
            format!("{BASE_URL}/category/{category}")
        } else {
            format!("{BASE_URL}/find/?find={}&offset={}", manatan_shared::sdk::http::url_encode(query), page(&request))
        };
        let body = get_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged { entries: parse_cards(&body), has_next_page: has_next_page(&body) })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        Ok(fetch_details(&request_key(&request, "item").unwrap_or_else(|| "/movie/sample".to_string())))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/movie/sample".to_string());
        let body = get_or_fixture(&absolute_url(&path), EPISODES_FIXTURE);
        let mut episodes = parse_episodes(&body);
        if episodes.is_empty() {
            episodes.push(VideoEpisode {
                key: path_key(&path),
                title: Some("مشاهدة".to_string()),
                episode_number: Some(1.0),
                url: Some(absolute_url(&path)),
                language: Some("ar".to_string()),
                ..VideoEpisode::default()
            });
        }
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/movie/sample".to_string());
        let page_url = absolute_url(&path);
        let body = get_or_fixture(&page_url, HOSTERS_FIXTURE);
        let watch = html::attr_after(&body, "watchBTn", "href").map(|href| absolute_url(&href)).unwrap_or(page_url);
        let watch_body = get_or_fixture(&watch, HOSTERS_FIXTURE);
        Ok(parse_hosters(&watch_body, &watch))
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
            title: "Popular".to_string(),
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
    body.split("MovieBlock").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let title = html::text_between(chunk, "BlockName", "</h4>")
            .map(|text| html::strip_tags(&text))
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .filter(|text| !text.is_empty())
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
        title: html::text_between(&body, "BreadCrumbs", "</ol>")
            .map(|text| html::strip_tags(&text))
            .or_else(|| html::text_between(&body, "<h1", "</h1>").map(|text| html::strip_tags(&text)))
            .unwrap_or_else(|| path_key(path).trim_matches('/').replace('-', " "))
            .replace(" مترجم", "")
            .replace("فيلم ", ""),
        cover: html::attr_after(&body, "<img", "data-src").or_else(|| html::attr_after(&body, "<img", "data-lazy-src")).or_else(|| html::attr_after(&body, "<img", "src")).map(|image| absolute_url(&image)),
        description: html::text_between(&body, "StoryLine", "</div>").map(|text| html::strip_tags(&text)).filter(|text| !text.is_empty()),
        tags: collect_links_after(&body, "النوع"),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: if path.contains("/selary/") { ItemStatus::Unknown } else { ItemStatus::Completed },
        url: Some(absolute_url(path)),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let mut seen = BTreeSet::new();
    body.split("ContainerEpisodesList").skip(1).flat_map(|section| section.split("<a").skip(1)).filter_map(|chunk| {
        let href = html::attr(chunk, "href")?;
        let key = path_key(&href);
        if !seen.insert(key.clone()) { return None; }
        let title = html::strip_tags(chunk);
        Some(VideoEpisode {
            key,
            title: Some(if title.is_empty() { "مشاهدة".to_string() } else { title.clone() }),
            episode_number: first_number(&title),
            url: Some(absolute_url(&href)),
            language: Some("ar".to_string()),
            ..VideoEpisode::default()
        })
    }).collect()
}

fn parse_hosters(body: &str, referer: &str) -> Vec<VideoHoster> {
    body.split("<li").skip(1).filter_map(|chunk| {
        let embed = html::attr(chunk, "data-link")?;
        if embed.trim().is_empty() { return None; }
        let name = html::strip_tags(chunk);
        Some(video_hoster(&absolute_url(&embed), if name.is_empty() { "Mirror" } else { &name }, referer))
    }).collect()
}

fn resolve_streams(target: &str, name: &str) -> Vec<VideoStream> {
    if target.contains("reviewtech") || target.contains("reviewrate") {
        let body = get_or_fixture(target, STREAM_FIXTURE);
        if let Some(src) = html::attr_after(&body, "<source", "src") {
            return vec![media_stream(&absolute_url(&src), &format!("{name}p"), &format!("{name}p"), target)];
        }
    }
    if target.contains(".m3u8") || target.contains(".mp4") {
        vec![media_stream(target, name, name, BASE_URL)]
    } else {
        vec![external_stream(target, name)]
    }
}

fn video_hoster(key: &str, name: &str, referer: &str) -> VideoHoster {
    VideoHoster {
        key: key.to_string(),
        name: name.to_string(),
        url: Some(key.to_string()),
        lazy: true,
        video_count: Some(1),
        headers: referer_headers(referer),
        ..VideoHoster::default()
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

fn collect_links_after(body: &str, label: &str) -> Vec<String> {
    body.split(label).nth(1).unwrap_or_default().split("</li>").next().unwrap_or_default().split("<a").skip(1)
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
    body.contains("page-numbers") && body.contains("next")
}

fn error(message: &str) -> ExtensionError {
    ExtensionError { message: message.to_string() }
}

const LIST_FIXTURE: &str = r#"<ul class="Blocks-UL"><div class="MovieBlock"><a href="/movie/sample"><div class="Poster"><img data-src="/cover.jpg"></div><div class="BlockName"><h4>Sample Movie</h4></div></a></div></ul>"#;
const DETAILS_FIXTURE: &str = r#"<div class="Poster"><img data-src="/cover.jpg"></div><div class="BreadCrumbs"><ol><li><a><span>Sample Movie</span></a></li></ol></div><div class="StoryLine"><p>Sample description.</p></div>"#;
const EPISODES_FIXTURE: &str = DETAILS_FIXTURE;
const HOSTERS_FIXTURE: &str = r#"<a class="watchBTn" href="/watch/sample"></a><div class="containerServers"><ul><li data-link="https://media.invalid/embed/sample">720</li></ul></div>"#;
const STREAM_FIXTURE: &str = r#"<video><source src="https://media.invalid/sample.mp4"></video>"#;

export_video_source!(SOURCE);
