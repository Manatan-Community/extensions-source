use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;

const SOURCE: PelisPlusPh = PelisPlusPh;
const BASE_URL: &str = "https://www.pelisplushd.la";

struct PelisPlusPh;

impl VideoSource for PelisPlusPh {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_listing(&fetch(
            &format!("{BASE_URL}/peliculas?page={}", page(&request)),
            LIST_FIXTURE,
            BASE_URL,
        )))
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
        let p = page(&request);
        let target = if !query.is_empty() {
            format!("{BASE_URL}/search?s={}&page={p}", url::query_escape(query))
        } else if let Some(genre) = filter(&request, "genre").filter(|value| !value.is_empty()) {
            format!("{BASE_URL}/{genre}?page={p}")
        } else {
            format!("{BASE_URL}/peliculas?page={p}")
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/pelicula/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/pelicula/sample".to_string());
        let referer = absolute_url(&path);
        if path.contains("/pelicula/") {
            return Ok(vec![VideoEpisode {
                key: path.clone(),
                title: Some("PELICULA".to_string()),
                episode_number: Some(1.0),
                url: Some(referer),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let body = fetch(&referer, DETAILS_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut episodes = doc
            .select(&selector(".tab-content a"))
            .enumerate()
            .filter_map(|(idx, a)| {
                let href = attr(&a, "href")?;
                let key = path_key(&href);
                Some(VideoEpisode {
                    key: key.clone(),
                    title: Some(text(a)),
                    episode_number: Some((idx + 1) as f32),
                    url: Some(absolute_url(&key)),
                    language: Some("es".to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect::<Vec<_>>();
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/pelicula/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for item in doc.select(&selector(".TbVideoNv li")) {
            let label = attr(&item, "data-name").unwrap_or_default();
            let lang = if label.contains("Subtitulado") {
                "[SUB]"
            } else if label.contains("Latino") {
                "[LAT]"
            } else {
                "[CAST]"
            };
            let Some(embed) = attr(&item, "data-url").filter(|value| !value.is_empty()) else {
                continue;
            };
            let name = [lang.to_string(), text(item)]
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            streams.extend(resolve_embed(&embed, &name, &referer, &request));
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Peliculas".to_string(),
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
    let entries = doc
        .select(&selector(".Posters-link"))
        .filter_map(card)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: !entries.is_empty(),
        entries,
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href")?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_text(el, ".listing-content > p, p").unwrap_or_else(|| title_from_path(&key)),
        cover: select_attr(el, "img", "src").map(|src| absolute_url(&src)),
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
    let mut item = CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, ".card-body h1, h1").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, ".card-body img, img", "src").map(|src| absolute_url(&src)),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: if path.contains("/serie/") {
            ItemStatus::Unknown
        } else {
            ItemStatus::Completed
        },
        initialized: true,
        ..CatalogItem::default()
    };
    for p in doc.select(&selector(".card-body p")) {
        let line = text(p);
        if line.contains("Sinopsis:") {
            item.description = p.next_sibling_element().map(text);
        } else if line.contains("Generos:") || line.contains("Géneros:") {
            item.tags = p
                .select(&selector(".content-type-a a, a"))
                .map(text)
                .filter(|value| !value.is_empty())
                .collect();
        } else if line.contains("Reparto:") || line.contains("Actores:") {
            let credit = p
                .select(&selector(".content-type ~ span, span"))
                .next()
                .map(text)
                .unwrap_or_default()
                .split(',')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            if !credit.is_empty() {
                item.artists = vec![credit];
            }
        }
    }
    item
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_remote(embed, referer);
    if embed.contains(".m3u8") {
        return parse_hls(&embed, name, referer, request);
    }
    let body = fetch(&embed, "", referer);
    if let Some(src) = first_media_url(&body) {
        let src = absolute_remote(&src, &embed);
        if src.contains(".m3u8") {
            return parse_hls(&src, name, &embed, request);
        }
        return vec![stream(&src, name, "direct", &embed, false)];
    }
    vec![external_stream(&embed, name, referer)]
}

fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)["']"#,
        r#"src\s*:\s*["']([^"']+)["']"#,
        r#"<source[^>]+src=["']([^"']+)["']"#,
        r#"https?://[^\s'"\\]+\.m3u8[^\s'"\\]*"#,
    ]
    .into_iter()
    .find_map(|pattern| {
        Regex::new(pattern)
            .ok()?
            .captures(body)
            .and_then(|captures| captures.get(1).or_else(|| captures.get(0)))
            .map(|value| value.as_str().replace("\\/", "/"))
    })
}

fn parse_hls(master: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = client(referer)
        .get(master)
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    let mut out = Vec::new();
    let mut quality = "auto".to_string();
    for line in body.lines() {
        if line.starts_with("#EXT-X-STREAM-INF") {
            quality = line
                .split("RESOLUTION=")
                .nth(1)
                .and_then(|value| value.split('x').nth(1))
                .and_then(|value| value.split(',').next())
                .map(|value| format!("{value}p"))
                .unwrap_or_else(|| "auto".to_string());
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            out.push(stream(
                &absolute_remote(line.trim(), master),
                name,
                &quality,
                referer,
                true,
            ));
        }
    }
    if out.is_empty() {
        out.push(stream(master, name, "auto", referer, true));
    }
    sort_streams(&mut out, request);
    out
}

fn stream(target: &str, name: &str, quality: &str, referer: &str, hls: bool) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} {quality}")),
        quality: Some(quality.to_string()),
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

fn external_stream(target: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} External")),
        quality: Some(name.to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let server = pref(request, "preferred_server", "VidHide").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    let language = pref(request, "preferred_language", "[LAT]");
    streams.sort_by_key(|stream| {
        let name = stream.name.clone().unwrap_or_default();
        (
            name.contains(&language),
            name.to_ascii_lowercase().contains(&server),
            name.contains(&quality),
            quality_rank(&name),
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
        .filter(|v| !v.is_empty())
}

fn select_attr_doc(doc: &Html, sel: &str, name: &str) -> Option<String> {
    doc.select(&selector(sel))
        .next()
        .and_then(|element| element.value().attr(name))
        .map(ToString::to_string)
}

fn select_text(el: ElementRef<'_>, sel: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .map(text)
        .filter(|v| !v.is_empty())
}

fn select_attr(el: ElementRef<'_>, sel: &str, name: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .and_then(|element| element.value().attr(name))
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
    let value = input.trim().replace("\\/", "/").replace("&amp;", "&");
    if value.starts_with("http://") || value.starts_with("https://") {
        value
    } else if let Some(rest) = value.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url::join_url(base, &value)
    }
}

fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .filter(|p| p.starts_with('/'))
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
        .map(path_key)
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
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
        .and_then(|captures| captures.get(1))
        .and_then(|value| value.as_str().parse().ok())
        .unwrap_or(0)
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("PelisPlusPh")
        .replace('-', " ")
}

trait NextSiblingElement {
    fn next_sibling_element(&self) -> Option<ElementRef<'_>>;
}

impl NextSiblingElement for ElementRef<'_> {
    fn next_sibling_element(&self) -> Option<ElementRef<'_>> {
        let mut sibling = self.next_sibling();
        while let Some(node) = sibling {
            if let Some(element) = ElementRef::wrap(node) {
                return Some(element);
            }
            sibling = node.next_sibling();
        }
        None
    }
}

const LIST_FIXTURE: &str = r#"
<a class="Posters-link" href="/pelicula/sample"><img src="/cover.jpg"><div class="listing-content"><p>Sample Movie</p></div></a>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="card-body"><h1>Sample Movie</h1><img src="/cover.jpg"><p>Sinopsis:</p><p>Fixture details for smoke tests.</p><p><span class="content-type">Generos:</span><span class="content-type-a"><a>Drama</a></span></p><div class="tab-content"><a href="/episodio/sample-1">Episodio 1</a></div></div>
"#;

const WATCH_FIXTURE: &str = r#"
<ul class="TbVideoNv"><li data-name="Latino" data-url="https://invalid.local/embed">Server</li></ul>
"#;

export_video_source!(SOURCE);
