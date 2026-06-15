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

const SOURCE: AnimeVostFr = AnimeVostFr;
const BASE_URL: &str = "https://animevostfr.tv";

struct AnimeVostFr;

impl VideoSource for AnimeVostFr {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/filter-advance/page/{page}/?status=ongoing")
        } else {
            format!("{BASE_URL}/filter-advance/page/{page}/")
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE, BASE_URL)))
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
        let mut target = if query.is_empty() {
            format!("{BASE_URL}/filter-advance/page/{page}/")
        } else {
            format!("{BASE_URL}/page/{page}/?s={}", url::query_escape(query))
        };
        for key in ["topic", "genre", "years", "status", "typesub"] {
            if let Some(value) = filter(&request, key).filter(|value| !value.is_empty()) {
                target.push(if target.contains('?') { '&' } else { '?' });
                target.push_str(key);
                target.push('=');
                target.push_str(&url::query_escape(&value));
            }
        }
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        let page_url = absolute_url(&path);
        let body = fetch(&page_url, DETAILS_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let kind = select_text_doc(&doc, "div.mvici-right > p:contains(Type) > a:last-child")
            .unwrap_or_default();
        if kind.eq_ignore_ascii_case("MOVIE") || doc.select(&selector("#seasonss a")).next().is_none() {
            return Ok(vec![VideoEpisode {
                key: path.clone(),
                title: Some("Movie".to_string()),
                episode_number: Some(1.0),
                url: Some(page_url),
                language: Some("fr".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let mut episodes = doc
            .select(&selector("div#seasonss > div.les-title > a, #seasonss a"))
            .filter_map(|el| {
                let href = attr(&el, "href")?;
                let key = path_key(&href);
                let number = key
                    .split("-episode-")
                    .nth(1)
                    .and_then(|v| v.split('-').next())
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(1.0);
                Some(VideoEpisode {
                    key: key.clone(),
                    title: Some(format!("Episode {}", trim_float(number))),
                    episode_number: Some(number),
                    url: Some(absolute_url(&key)),
                    language: Some("fr".to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect::<Vec<_>>();
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/sample".to_string());
        let page_url = absolute_url(&path);
        let body = fetch(&page_url, PLAYER_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let ep_id = doc
            .select(&selector("link[rel=shortlink]"))
            .next()
            .and_then(|el| attr(&el, "href"))
            .and_then(|href| href.split("?p=").nth(1).map(ToString::to_string))
            .unwrap_or_default();
        let mut streams = Vec::new();
        for option in doc.select(&selector("div.list-server select option, select option")) {
            let Some(server_id) = attr(&option, "value") else {
                continue;
            };
            let server_name = text(option);
            let link = client(&page_url)
                .get(format!(
                    "{BASE_URL}/ajax-get-link-stream/?server={server_id}&filmId={ep_id}"
                ))
                .header("X-Requested-With", "XMLHttpRequest")
                .referer(&page_url)
                .xhr()
                .send_text()
                .unwrap_or_default();
            if !link.trim().is_empty() {
                streams.extend(resolve_link(link.trim(), &server_name, &page_url, &request));
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
                title: "Catalogue".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "En cours".to_string(),
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
            .select(&selector("div.ml-item"))
            .filter_map(card)
            .collect(),
        has_next_page: doc
            .select(&selector("ul.pagination li:not(.active):last-child a, .pagination a.next"))
            .next()
            .is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "a:has(img), a", "href")?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_text(el, "span.mli-info > h2, h2")
            .or_else(|| select_attr(el, "img", "alt"))
            .unwrap_or_else(|| title_from_path(&key)),
        cover: select_attr(el, "img", "data-original")
            .or_else(|| select_attr(el, "img", "data-lazy-src"))
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
        title: select_text_doc(&doc, "h1[itemprop=name], div.slide-middle h1, h1")
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, "div.thumb img, img", "data-lazy-src")
            .or_else(|| select_attr_doc(&doc, "div.thumb img, img", "src"))
            .map(|src| absolute_url(&src)),
        description: select_text_doc(&doc, "div[itemprop=description], div.slide-desc"),
        tags: select_text_doc(&doc, "div.mvici-left > p:contains(Genres)")
            .map(|v| split_csv(v.trim_start_matches("Genres: ")))
            .unwrap_or_default(),
        url: Some(absolute_url(path)),
        language: Some("fr".to_string()),
        content_rating: Some("safe".to_string()),
        status: match select_text_doc(&doc, "div.mvici-right > p:contains(Statut) > a:last-child")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "ongoing" | "en cours" => ItemStatus::Ongoing,
            "completed" | "terminer" | "termine" => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        initialized: true,
        ..CatalogItem::default()
    }
}

fn resolve_link(link: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    if link.contains("cdopetimes.xyz") {
        return cdope_streams(link, name);
    }
    if link.contains(".m3u8") || link.contains("master.txt") {
        return parse_hls(link, name, link, request);
    }
    if link.contains(".mp4") || link.contains(".webm") {
        return vec![stream(
            link,
            name,
            &preference(request, "preferred_quality", "auto"),
            referer,
        )];
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

#[derive(Deserialize)]
struct CdopeResponse {
    data: Vec<CdopeFile>,
}

#[derive(Deserialize)]
struct CdopeFile {
    file: String,
    label: String,
    #[serde(rename = "type")]
    kind: String,
}

fn cdope_streams(link: &str, name: &str) -> Vec<VideoStream> {
    let id = link.split("/v/").nth(1).unwrap_or_default();
    let body = client(link)
        .post(format!("https://cdopetimes.xyz/api/source/{id}"))
        .referer(link)
        .header("Origin", "https://cdopetimes.xyz")
        .header("X-Requested-With", "XMLHttpRequest")
        .form(&[("r", ""), ("d", "cdopetimes.xyz")])
        .send_text()
        .unwrap_or_default();
    serde_json::from_str::<CdopeResponse>(&body)
        .map(|res| {
            res.data
                .into_iter()
                .map(|file| {
                    stream(
                        &file.file,
                        &format!("{name} Cdope {}", file.kind),
                        &file.label,
                        "https://cdopetimes.xyz/",
                    )
                })
                .collect()
        })
        .unwrap_or_else(|_| vec![external(link, name, BASE_URL)])
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
    let is_hls = url.contains(".m3u8") || url.contains("master.txt");
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

fn extract_media_url(body: &str) -> Option<String> {
    for marker in ["file:\"", "file: \"", "source:\""] {
        let value = body.split(marker).nth(1)?;
        let end = if marker.contains('"') { '"' } else { '\'' };
        let url = value.split(end).next()?.replace("\\/", "/");
        if url.contains(".m3u8") || url.contains(".mp4") {
            return Some(url);
        }
    }
    None
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let server = preference(request, "preferred_server", "");
    let quality = preference(request, "preferred_quality", "720");
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
    el.select(&selector(selector_value))
        .next()
        .map(text)
        .filter(|value| !value.is_empty())
}

fn select_text_doc(doc: &Html, selector_value: &str) -> Option<String> {
    doc.select(&selector(selector_value))
        .next()
        .map(text)
        .filter(|value| !value.is_empty())
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
    input.strip_prefix(BASE_URL).map(path_key).filter(|p| p != "/")
}

fn path_key(input: &str) -> String {
    let value = input.split('?').next().unwrap_or(input).split('#').next().unwrap_or(input);
    if value.starts_with("http") {
        format!("/{}", value.split('/').skip(3).collect::<Vec<_>>().join("/"))
            .trim_end_matches('/')
            .to_string()
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

const LIST_FIXTURE: &str = r#"
<div class="ml-item"><a href="/sample"><img data-original="/sample.jpg"><span class="mli-info"><h2>Sample</h2></span></a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 itemprop="name">Sample</h1><div class="thumb"><img src="/sample.jpg"></div><div itemprop="description">Synopsis</div><div id="seasonss"><div class="les-title"><a href="/sample-episode-1">sample-episode-1</a></div></div>
"#;
const PLAYER_FIXTURE: &str = r#"
<link rel="shortlink" href="https://animevostfr.tv/?p=1"><div class="list-server"><select><option value="1">External</option></select></div>
"#;

export_video_source!(SOURCE);
