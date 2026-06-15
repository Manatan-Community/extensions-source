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
use serde_json::{Value, json};

const SOURCE: CineCalidad = CineCalidad;
const BASE_URL: &str = "https://www.cinecalidad.ec";

struct CineCalidad;

impl VideoSource for CineCalidad {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/fecha-de-lanzamiento/2024/page/{page}")
        } else {
            format!("{BASE_URL}/page/{page}")
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
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
        } else if let Some(genre) = filter(&request, "genre").filter(|v| !v.is_empty()) {
            if genre.contains("fecha-de-lanzamiento") {
                format!("{BASE_URL}/{genre}/2024/page/{page}")
            } else {
                format!("{BASE_URL}/{genre}/page/{page}")
            }
        } else {
            format!("{BASE_URL}/page/{page}")
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path =
            request_key(&request, "item").unwrap_or_else(|| "/ver-pelicula/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path =
            request_key(&request, "item").unwrap_or_else(|| "/ver-pelicula/sample".to_string());
        Ok(parse_episodes(
            &fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL),
            &path,
        ))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/ver-pelicula/sample".to_string());
        let body = fetch(&absolute_url(&path), WATCH_FIXTURE, BASE_URL);
        let mut streams = Vec::new();
        for embed in player_embeds(&body, &absolute_url(&path)) {
            streams.extend(resolve_embed(
                &embed,
                &host_name(&embed),
                &absolute_url(&path),
                &request,
            ));
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
                title: "Populares".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Estrenos".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|p| absolute_url(&p)))
    }
    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|p| absolute_url(&p)))
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

fn parse_cards(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let entries = doc
        .select(&selector(
            ".item[data-cf] .custom, article .custom, .custom",
        ))
        .filter_map(card_from_anchor)
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("nextpostslink"),
    }
}
fn card_from_anchor(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "a", "href").unwrap_or_else(|| attr(&el, "href"));
    let title = select_attr(el, "img", "alt").or_else(|| select_text(el, ".Title"))?;
    let lang = attr(&el, "class").to_ascii_lowercase();
    let prefix = if lang.contains("sub") || href.contains("-sub") {
        "[SUB] "
    } else if lang.contains("lat") || href.contains("-lat") {
        "[LAT] "
    } else if lang.contains("esp") || href.contains("-es") {
        "[ES] "
    } else {
        ""
    };
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title: format!("{prefix}{title}"),
        cover: select_attr(el, "img", "data-src")
            .or_else(|| select_attr(el, "img", "src"))
            .map(|src| absolute_url(&src)),
        url: Some(absolute_url(&path)),
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
        title: select_text_doc(&doc, "h1, .single_left .title")
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, ".single_left table img, img", "data-src")
            .or_else(|| select_attr_doc(&doc, ".single_left table img, img", "src"))
            .map(|src| absolute_url(&src)),
        url: Some(absolute_url(path)),
        description: select_text_doc(&doc, ".single_left table p")
            .map(|v| v.trim_matches('"').to_string()),
        tags: select_texts_doc(&doc, ".sgeneros a, a[href*='genero-de-la-pelicula']"),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: if path.contains("ver-pelicula") {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        },
        initialized: true,
        ..CatalogItem::default()
    }
}
fn parse_episodes(body: &str, item_path: &str) -> Vec<VideoEpisode> {
    if item_path.contains("ver-pelicula") {
        return vec![VideoEpisode {
            key: item_path.to_string(),
            title: Some("PELICULA".to_string()),
            episode_number: Some(1.0),
            url: Some(absolute_url(item_path)),
            language: Some("es".to_string()),
            ..VideoEpisode::default()
        }];
    }
    let doc = Html::parse_document(body);
    let mut idx = 1.0;
    let mut out = Vec::new();
    for (i, row) in doc.select(&selector(".mark-1")).enumerate() {
        let href = select_attr(row, ".episodiotitle a", "href").unwrap_or_default();
        if href.is_empty() {
            continue;
        }
        let title =
            select_text(row, ".episodiotitle a").unwrap_or_else(|| format!("Episodio {}", i + 1));
        let num = select_text(row, ".numerando").unwrap_or_default();
        out.push(VideoEpisode {
            key: path_key(&href),
            title: Some(format!("{num} {title}").trim().to_string()),
            episode_number: Some(idx),
            url: Some(absolute_url(&href)),
            language: Some("es".to_string()),
            ..VideoEpisode::default()
        });
        idx += 1.0;
    }
    out.reverse();
    out
}
fn player_embeds(body: &str, _referer: &str) -> Vec<String> {
    let re = Regex::new(r#"src=["']([^"']+)["']"#).unwrap();
    let mut out = Vec::new();
    for option in body
        .split("playeroptionsul")
        .skip(1)
        .flat_map(|b| b.split("<li").skip(1))
    {
        if let Some(embed) = Regex::new(r#"data-option=["']([^"']+)["']"#)
            .unwrap()
            .captures(option)
            .and_then(|c| c.get(1))
            .map(|m| absolute_remote(m.as_str(), BASE_URL))
        {
            out.push(embed);
        }
    }
    if out.is_empty() {
        out.extend(
            re.captures_iter(body)
                .filter_map(|c| c.get(1))
                .map(|m| absolute_remote(m.as_str(), BASE_URL)),
        );
    }
    out
}
fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    if embed.contains(".m3u8") {
        return parse_hls(embed, name, referer, request);
    }
    let body = fetch(embed, "", referer);
    if let Some(src) = first_media_url(&body).map(|s| absolute_remote(&s, embed)) {
        if src.contains(".m3u8") {
            parse_hls(&src, name, embed, request)
        } else {
            vec![stream(&src, name, "direct", embed, false)]
        }
    } else {
        vec![external_stream(embed, name, referer)]
    }
}
fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)["']"#,
        r#"src\s*:\s*["']([^"']+)["']"#,
        r#"<source[^>]+src=["']([^"']+)["']"#,
    ]
    .into_iter()
    .find_map(|p| {
        Regex::new(p)
            .ok()?
            .captures(body)?
            .get(1)
            .map(|m| m.as_str().replace("\\/", "/"))
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
                .and_then(|v| v.split('x').nth(1))
                .and_then(|v| v.split(',').next())
                .map(|v| format!("{v}p"))
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
        ..VideoStream::default()
    }
}
fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let server = pref(request, "preferred_server", "Voe").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|s| {
        (
            s.name
                .clone()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .contains(&server),
            s.quality.clone().unwrap_or_default().contains(&quality),
            quality_rank(&s.quality.clone().unwrap_or_default()),
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
fn select_texts_doc(doc: &Html, sel: &str) -> Vec<String> {
    doc.select(&selector(sel))
        .map(text)
        .filter(|v| !v.is_empty())
        .collect()
}
fn select_attr_doc(doc: &Html, sel: &str, name: &str) -> Option<String> {
    doc.select(&selector(sel))
        .next()
        .and_then(|e| e.value().attr(name))
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
        .and_then(|e| e.value().attr(name))
        .map(ToString::to_string)
}
fn attr(el: &ElementRef<'_>, name: &str) -> String {
    el.value().attr(name).unwrap_or_default().to_string()
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
    let t = input.trim().replace("\\/", "/");
    if t.starts_with("http") {
        t
    } else if let Some(rest) = t.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url::join_url(base, &t)
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
        .and_then(|v| {
            v.get("key")
                .or_else(|| v.get("url"))
                .and_then(Value::as_str)
                .or_else(|| v.as_str())
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
fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}
fn with_listing(request: &Value, listing: &str) -> Value {
    json!({ "listing": listing, "preferences": request.get("preferences").cloned().unwrap_or(Value::Null) })
}
fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|f| f.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}
fn pref(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}
fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("CineCalidad")
        .replace('-', " ")
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
fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}
fn quality_rank(input: &str) -> i32 {
    Regex::new(r#"(\d+)"#)
        .unwrap()
        .captures(input)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0)
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="item" data-cf><div class="custom"><a href="/ver-pelicula/sample"><img alt="Sample" data-src="/sample.jpg"></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><div class="single_left"><table><tr><td><img data-src="/sample.jpg"><p>Sample description.</p></td></tr></table></div>"#;
const WATCH_FIXTURE: &str =
    r#"<ul id="playeroptionsul"><li data-option="https://example.invalid/embed"></li></ul>"#;
