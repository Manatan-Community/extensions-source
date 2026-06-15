use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{SearchRequest, http::HttpClient},
    url,
    video::referer_headers,
};
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: JetAnime = JetAnime;
const BASE_URL: &str = "https://ssl.jetanimes.com";

struct JetAnime;

impl VideoSource for JetAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if listing(&request) == "latest" {
            let page = page(&request);
            let body = fetch(&format!("{BASE_URL}/episodes/page/{page}/"), LIST_FIXTURE, BASE_URL);
            return Ok(parse_latest(&body));
        }
        let body = fetch(BASE_URL, LIST_FIXTURE, BASE_URL);
        Ok(Paged {
            entries: Html::parse_document(&body)
                .select(&selector("aside#dtw_content_views-2 div.dtw_content > article, article"))
                .filter_map(card)
                .collect(),
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
        let target = if !query.is_empty() {
            format!("{BASE_URL}/page/{page}/?s={}", url::query_escape(query))
        } else if let Some(path) = filter(&request, "subpage")
            .or_else(|| filter(&request, "year"))
            .filter(|v| !v.is_empty())
        {
            format!("{BASE_URL}{}{}", path.trim_end_matches('/'), if page > 1 { format!("/page/{page}/") } else { "/".to_string() })
        } else {
            format!("{BASE_URL}/serie/page/{page}/")
        };
        let body = fetch(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_search(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/serie/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/serie/sample".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut episodes = doc
            .select(&selector(".episodios li a, .episodios a, a[href*='/episodes/']"))
            .filter_map(|el| {
                let href = attr(&el, "href")?;
                let key = path_key(&href);
                let number = key
                    .split("-episode-")
                    .nth(1)
                    .and_then(|v| v.split(['-', '/']).next())
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(1.0);
                Some(VideoEpisode {
                    key: key.clone(),
                    title: Some(text(el).if_empty(format!("Episode {}", trim_float(number)))),
                    episode_number: Some(number),
                    url: Some(absolute_url(&key)),
                    language: Some("fr".to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect::<Vec<_>>();
        if episodes.is_empty() {
            episodes.push(VideoEpisode {
                key: path.clone(),
                title: Some("Movie".to_string()),
                episode_number: Some(1.0),
                url: Some(absolute_url(&path)),
                language: Some("fr".to_string()),
                ..VideoEpisode::default()
            });
        }
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/episodes/sample".to_string());
        let page_url = absolute_url(&path);
        let body = fetch(&page_url, PLAYER_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for player in doc.select(&selector("ul#playeroptionsul li, #playeroptions li, .dooplay_player_option")) {
            if attr(&player, "data-nume").as_deref() == Some("trailer") {
                continue;
            }
            let post = attr(&player, "data-post").unwrap_or_default();
            let nume = attr(&player, "data-nume").unwrap_or_else(|| "1".to_string());
            let kind = attr(&player, "data-type").unwrap_or_else(|| "movie".to_string());
            let name = text(player).if_empty(format!("Server {nume}"));
            let response = fetch(
                &format!("{BASE_URL}/wp-json/dooplayer/v1/post/{post}?type={kind}&source={nume}"),
                EMBED_FIXTURE,
                &page_url,
            );
            let embed = serde_json::from_str::<EmbedResponse>(&response)
                .map(|res| res.embed_url)
                .unwrap_or_else(|_| {
                    response
                        .split("\"embed_url\":\"")
                        .nth(1)
                        .and_then(|v| v.split('"').next())
                        .unwrap_or_default()
                        .to_string()
                })
                .replace("\\/", "/");
            if !embed.is_empty() {
                streams.extend(resolve_link(&embed, &name, &page_url, &request));
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
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Episodes".to_string(),
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

#[derive(Deserialize)]
struct EmbedResponse {
    embed_url: String,
}

trait IfEmpty {
    fn if_empty(self, fallback: String) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: String) -> String {
        if self.is_empty() { fallback } else { self }
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

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc
            .select(&selector("article, .items article"))
            .filter_map(|el| {
                let href = select_attr(el, "a[href]", "href")?;
                let slug = href
                    .split("/episodes/")
                    .nth(1)
                    .unwrap_or(&href)
                    .split("-episode")
                    .next()
                    .unwrap_or(&href)
                    .split("-saison")
                    .next()
                    .unwrap_or(&href);
                let key = format!("/serie/{}", slug.trim_matches('/'));
                Some(CatalogItem {
                    key: key.clone(),
                    title: select_attr(el, "img", "alt")
                        .or_else(|| select_text(el, "h1, h2, h3"))
                        .unwrap_or_else(|| title_from_path(&key)),
                    cover: select_attr(el, "img", "data-src").or_else(|| select_attr(el, "img", "src")).map(|src| absolute_url(&src)),
                    url: Some(absolute_url(&key)),
                    language: Some("fr".to_string()),
                    content_rating: Some("safe".to_string()),
                    status: ItemStatus::Unknown,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: doc.select(&selector("div.pagination > span.current + a, .pagination a.next")).next().is_some(),
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc
            .select(&selector("div.search-page div.result-item div.image a, div#archive-content article div.poster, div.content div.items article div.poster, article"))
            .filter_map(card)
            .collect(),
        has_next_page: doc.select(&selector("div.pagination > span.current + a, .pagination a.next")).next().is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href").or_else(|| select_attr(el, "a[href]", "href"))?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_attr(el, "img", "alt")
            .or_else(|| select_text(el, "h1, h2, h3"))
            .unwrap_or_else(|| title_from_path(&key)),
        cover: select_attr(el, "img", "data-src")
            .or_else(|| select_attr(el, "img", "src"))
            .map(|src| absolute_url(&src)),
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
        title: select_text_doc(&doc, ".sheader .data h1, h1").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, ".poster img, img", "data-src")
            .or_else(|| select_attr_doc(&doc, ".poster img, img", "src"))
            .map(|src| absolute_url(&src)),
        description: select_text_doc(&doc, "#info, .wp-content, .description"),
        tags: doc.select(&selector(".sgeneros a, a[href*='/genre/']")).map(text).filter(|v| !v.is_empty()).collect(),
        url: Some(absolute_url(path)),
        language: Some("fr".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
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
            parse_hls(&media, &format!("{} {}", server_label(link), name), link, request)
        } else {
            vec![stream(&media, &format!("{} {}", server_label(link), name), "auto", link)]
        };
    }
    vec![external(link, &format!("{} {}", server_label(link), name), referer)]
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
    let quality = preference(request, "preferred_quality", "1080p");
    streams.sort_by_key(|stream| {
        let name = stream.name.as_deref().unwrap_or_default();
        let q = stream.quality.as_deref().unwrap_or_default();
        (name.contains(&server), q.contains(&quality))
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
    input.strip_prefix(BASE_URL).map(path_key).filter(|p| p != "/")
}

fn path_key(input: &str) -> String {
    let value = input.split('?').next().unwrap_or(input).split('#').next().unwrap_or(input);
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

fn trim_float(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i32)
    } else {
        value.to_string()
    }
}

fn server_label(link: &str) -> &'static str {
    let lower = link.to_ascii_lowercase();
    if lower.contains("sentinel") {
        "Sentinel"
    } else if lower.contains("hdsplay") {
        "Hdsplay"
    } else {
        "External"
    }
}

const LIST_FIXTURE: &str = r#"<article><a href="/serie/sample"><img alt="Sample" src="/sample.jpg"></a></article>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><ul class="episodios"><li><a href="/episodes/sample-episode-1">Episode 1</a></li></ul>"#;
const PLAYER_FIXTURE: &str = r#"<ul id="playeroptionsul"><li data-post="1" data-nume="1" data-type="movie">External</li></ul>"#;
const EMBED_FIXTURE: &str = r#"{"embed_url":"https://example.invalid/embed"}"#;

export_video_source!(SOURCE);
