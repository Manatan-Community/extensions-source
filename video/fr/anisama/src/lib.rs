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

const SOURCE: AniSama = AniSama;
const BASE_URL: &str = "https://v1.animesz.xyz";

struct AniSama;

impl VideoSource for AniSama {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let path = if listing(&request) == "latest" {
            format!("/recently-added/?page={page}")
        } else {
            format!("/most-popular/?page={page}")
        };
        Ok(parse_listing(&fetch(&absolute_url(&path), LIST_FIXTURE, BASE_URL)))
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
            "{BASE_URL}/filter?keyword={}&page={page}",
            url::query_escape(query)
        );
        for key in ["sort", "language"] {
            if let Some(value) = filter(&request, key).filter(|value| !value.is_empty()) {
                target.push('&');
                target.push_str(key);
                target.push('=');
                target.push_str(&url::query_escape(&value));
            }
        }
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample-1".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample-1".to_string());
        let id = path.rsplit('-').next().unwrap_or_default();
        let body = client(&absolute_url(&path))
            .get(format!("{BASE_URL}/ajax/episode/list/{id}"))
            .referer(&absolute_url(&path))
            .xhr()
            .send_text()
            .unwrap_or_else(|_| EPISODES_FIXTURE.to_string());
        let html = serde_json::from_str::<HtmlResponse>(&body)
            .map(|res| res.html)
            .unwrap_or(body);
        let doc = Html::parse_document(&html);
        let mut episodes = doc
            .select(&selector(".ep-item"))
            .filter_map(|el| {
                let number = attr(&el, "data-number")
                    .and_then(|n| n.parse::<f32>().ok())
                    .unwrap_or(1.0);
                let href = attr(&el, "href")?;
                let id = href.split('=').next_back().unwrap_or_default();
                Some(VideoEpisode {
                    key: format!("/ajax/episode/servers?episodeId={id}"),
                    title: attr(&el, "title").or_else(|| Some(format!("Episode {}", trim_float(number)))),
                    episode_number: Some(number),
                    language: Some("fr".to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect::<Vec<_>>();
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let endpoint = request_key(&request, "episode")
            .unwrap_or_else(|| "/ajax/episode/servers?episodeId=1".to_string());
        let episode_id = endpoint.split('=').next_back().unwrap_or("1").to_string();
        let body = client(BASE_URL)
            .get(absolute_url(&endpoint))
            .referer(BASE_URL)
            .xhr()
            .send_text()
            .unwrap_or_else(|_| SERVERS_FIXTURE.to_string());
        let html = serde_json::from_str::<HtmlResponse>(&body)
            .map(|res| res.html)
            .unwrap_or(body);
        let doc = Html::parse_document(&html);
        let mut streams = Vec::new();
        for server in doc.select(&selector(".server-item")) {
            let Some(id) = attr(&server, "data-id") else {
                continue;
            };
            let lang = attr(&server, "data-type").unwrap_or_default().to_uppercase();
            let source = client(BASE_URL)
                .get(format!(
                    "{BASE_URL}/ajax/episode/sources?id={id}&epid={episode_id}"
                ))
                .referer(BASE_URL)
                .xhr()
                .send_text()
                .unwrap_or_else(|_| SOURCE_FIXTURE.to_string());
            let link = serde_json::from_str::<PlayerInfo>(&source)
                .ok()
                .map(|info| info.link)
                .unwrap_or_default();
            if !link.is_empty() {
                let name = format!("{} {}", server_label(&link), lang).trim().to_string();
                streams.extend(resolve_link(&link, &name, BASE_URL, &request));
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
                title: "Most popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Recently added".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
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

#[derive(Deserialize)]
struct HtmlResponse {
    html: String,
}

#[derive(Deserialize)]
struct PlayerInfo {
    link: String,
}

fn client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_cookies_for(BASE_URL)
        .with_header("Origin", BASE_URL)
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
            .select(&selector(".film_list article"))
            .filter_map(card)
            .collect(),
        has_next_page: doc
            .select(&selector(".ap__-btn-next a:not(.disabled), .pagination a[rel=next]"))
            .next()
            .is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, ".film-poster-ahref, a", "href")?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_text(el, ".dynamic-name")
            .or_else(|| select_attr(el, "img", "alt"))
            .unwrap_or_else(|| title_from_path(&key)),
        cover: select_attr(el, ".film-poster-img, img", "data-src")
            .or_else(|| select_attr(el, ".film-poster-img, img", "src"))
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
    let details = doc
        .select(&selector(".anime-detail"))
        .next()
        .unwrap_or_else(|| doc.root_element());
    CatalogItem {
        key: path_key(path),
        title: select_text(details, ".dynamic-name")
            .map(|v| v.trim_end_matches(" VOSTFR").trim_end_matches(" VF").to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr(details, ".film-poster-img, img", "src")
            .or_else(|| select_attr(details, ".film-poster-img, img", "data-src"))
            .map(|src| absolute_url(&src)),
        description: select_text(details, ".shorting, .description"),
        tags: meta(details, "Genre")
            .map(|value| split_csv(&value))
            .unwrap_or_default(),
        authors: meta(details, "Studio").map(|value| vec![value]).unwrap_or_default(),
        url: Some(absolute_url(path)),
        language: Some("fr".to_string()),
        content_rating: Some("safe".to_string()),
        status: match meta(details, "Status").as_deref() {
            Some("Terminer") => ItemStatus::Completed,
            Some("En cours") => ItemStatus::Ongoing,
            _ => ItemStatus::Unknown,
        },
        initialized: true,
        ..CatalogItem::default()
    }
}

fn resolve_link(link: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    if link.contains("cdn2.vidcdn.xyz") {
        if let Ok(res) = serde_json::from_str::<CdnResponse>(&fetch(link, "", referer)) {
            return res
                .sources
                .into_iter()
                .map(|source| stream(&absolute_protocol(&source.file), name, "auto", referer))
                .collect();
        }
    }
    if link.contains(".m3u8") {
        return parse_hls(link, name, referer, request);
    }
    if link.contains(".mp4") || link.contains(".webm") {
        return vec![stream(link, name, &preference(request, "preferred_quality", "auto"), referer)];
    }
    vec![external(link, name, referer)]
}

#[derive(Deserialize)]
struct CdnResponse {
    sources: Vec<CdnSource>,
}

#[derive(Deserialize)]
struct CdnSource {
    file: String,
}

fn parse_hls(url: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = fetch(url, "", referer);
    if !body.contains("#EXT-X-STREAM-INF") {
        return vec![stream(
            url,
            name,
            &preference(request, "preferred_quality", "auto"),
            referer,
        )];
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
            let line = block
                .lines()
                .find(|line| !line.trim().is_empty() && !line.starts_with('#'))?;
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
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
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

fn meta(el: ElementRef<'_>, name: &str) -> Option<String> {
    select_text(el, &format!(".item:has(.item-title:contains({name})) > .item-content"))
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).unwrap()
}

fn select_text(el: ElementRef<'_>, selector_value: &str) -> Option<String> {
    el.select(&selector(selector_value))
        .next()
        .map(text)
        .filter(|value| !value.is_empty())
}

fn select_attr(el: ElementRef<'_>, selector_value: &str, name: &str) -> Option<String> {
    el.select(&selector(selector_value)).next().and_then(|e| attr(&e, name))
}

fn attr(el: &ElementRef<'_>, name: &str) -> Option<String> {
    el.value().attr(name).map(|v| v.to_string()).filter(|v| !v.is_empty())
}

fn text(el: ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn page(request: &Value) -> u32 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1) as u32
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .as_str()
                .or_else(|| value.get("key").and_then(Value::as_str))
        })
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
    input
        .strip_prefix(BASE_URL)
        .map(path_key)
        .filter(|path| path.starts_with("/anime/"))
}

fn path_key(input: &str) -> String {
    let value = input.split('#').next().unwrap_or(input);
    if value.starts_with("http") {
        let without_host = value.split('/').skip(3).collect::<Vec<_>>().join("/");
        format!("/{}", without_host.trim_start_matches('/')).trim_end_matches('/').to_string()
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

fn absolute_protocol(path: &str) -> String {
    if path.starts_with("//") {
        format!("https:{path}")
    } else {
        path.to_string()
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

fn split_csv(value: &str) -> Vec<String> {
    value
        .split([',', ';'])
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn title_from_path(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .replace(['-', '_'], " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn trim_float(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i32)
    } else {
        value.to_string()
    }
}

fn server_label(link: &str) -> String {
    let lower = link.to_ascii_lowercase();
    for (needle, label) in [
        ("toonanime", "CDN"),
        ("filemoon", "Filemoon"),
        ("sibnet", "Sibnet"),
        ("sendvid", "Sendvid"),
        ("voe", "Voe"),
        ("dood", "Dood"),
        ("vidhide", "VidHide"),
    ] {
        if lower.contains(needle) {
            return label.to_string();
        }
    }
    "External".to_string()
}

const LIST_FIXTURE: &str = r#"
<div class="film_list"><article><a class="film-poster-ahref" href="/anime/sample-1"><img class="film-poster-img" data-src="/sample.jpg"></a><h3 class="dynamic-name">Sample</h3></article></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="anime-detail"><h2 class="dynamic-name">Sample</h2><img class="film-poster-img" src="/sample.jpg"><div class="shorting">Synopsis</div><div class="item"><span class="item-title">Status</span><span class="item-content">En cours</span></div></div>
"#;
const EPISODES_FIXTURE: &str =
    r#"{"html":"<a class='ep-item' data-number='1' title='Episode 1' href='/watch?id=1'></a>"}"#;
const SERVERS_FIXTURE: &str =
    r#"{"html":"<div class='server-item' data-id='1' data-type='sub'>Server</div>"}"#;
const SOURCE_FIXTURE: &str = r#"{"link":"https://example.invalid/embed"}"#;

export_video_source!(SOURCE);
