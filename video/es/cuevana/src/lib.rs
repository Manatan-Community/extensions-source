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

const SOURCE: Cuevana = Cuevana;
const BASE_URL: &str = "https://ww1.cuevana3.ch";

struct Cuevana;

impl VideoSource for Cuevana {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        Ok(parse_cards(&fetch(
            &format!("{BASE_URL}/peliculas?page={page}"),
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
        let page = page(&request);
        let target = if !query.is_empty() {
            format!(
                "{BASE_URL}/search.html?keyword={}&page={page}",
                url::query_escape(query)
            )
        } else if let Some(genre) = filter(&request, "genre").filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/category/{genre}?page={page}")
        } else {
            format!("{BASE_URL}/peliculas?page={page}")
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/pelicula/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/pelicula/sample".to_string());
        Ok(parse_episodes(
            &fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL),
            &path,
        ))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/pelicula/sample".to_string());
        let body = fetch(&absolute_url(&path), WATCH_FIXTURE, BASE_URL);
        let mut streams = Vec::new();
        let doc = Html::parse_document(&body);
        for item in doc.select(&selector("ul.anime_muti_link li")) {
            let lang = select_text(item, ".cdtr span")
                .unwrap_or_default()
                .to_ascii_lowercase();
            let prefix = if lang.contains("latino") {
                "[LAT]"
            } else if lang.contains("castellano") {
                "[CAST]"
            } else if lang.contains("subtitulado") {
                "[SUB]"
            } else {
                ""
            };
            let embed = attr(&item, "data-video");
            if !embed.is_empty() {
                streams.extend(resolve_embed(
                    &absolute_url(&embed),
                    &format!("{prefix} {}", host_name(&embed)).trim().to_string(),
                    &absolute_url(&path),
                    &request,
                ));
            }
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
        .select(&selector(".MovieList .TPostMv .TPost, article.TPost"))
        .filter_map(card)
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("next page-numbers") || body.contains("pagination"),
    }
}
fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "a", "href")?;
    let title = select_text(el, "a .Title, .Title").or_else(|| select_attr(el, "img", "alt"))?;
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title,
        cover: select_attr(el, "a .Image figure.Objf img, img", "data-src")
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
        title: select_text_doc(&doc, ".TPost header .Title, h1")
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(
            &doc,
            ".backdrop article div.Image figure img, img",
            "data-src",
        )
        .or_else(|| select_attr_doc(&doc, "img", "src"))
        .map(|src| absolute_url(&src)),
        url: Some(absolute_url(path)),
        description: select_text_doc(
            &doc,
            ".backdrop article.TPost div.Description, .Description",
        ),
        tags: select_texts_doc(&doc, "ul.InfoList li:nth-child(1) > a, .InfoList a"),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}
fn parse_episodes(body: &str, item_path: &str) -> Vec<VideoEpisode> {
    if !item_path.contains("/serie/") {
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
    let mut out = Vec::new();
    for season in doc
        .select(&selector("[id*=season-]"))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let season_no = attr(&season, "id").replace("season-", "");
        for (idx, ep) in season
            .select(&selector(".TPostMv article.TPost, article.TPost"))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .enumerate()
        {
            let href = select_attr(ep, "a", "href").unwrap_or_default();
            if href.is_empty() {
                continue;
            }
            let num = select_text(ep, "a div.Image span.Year")
                .and_then(|v| v.split('x').next_back().and_then(|n| n.parse::<f32>().ok()))
                .unwrap_or(idx as f32 + 1.0);
            out.push(VideoEpisode {
                key: path_key(&href),
                title: Some(format!("T{season_no} - Episodio {num}")),
                episode_number: Some(num),
                url: Some(absolute_url(&href)),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            });
        }
    }
    out.reverse();
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
        r#"url\s*=\s*["']([^"']+)["']"#,
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
    let lang = pref(request, "preferred_language", "[LAT]");
    streams.sort_by_key(|s| {
        (
            s.name.clone().unwrap_or_default().contains(&lang),
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
        .unwrap_or("Cuevana")
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

const LIST_FIXTURE: &str = r#"<div class="MovieList"><div class="TPostMv"><article class="TPost"><a href="/pelicula/sample"><div class="Title">Sample</div><div class="Image"><figure class="Objf"><img data-src="/sample.jpg"></figure></div></a></article></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<article class="TPost"><header><h1 class="Title">Sample</h1></header><div class="Description">Sample description.</div><div class="Image"><figure><img data-src="/sample.jpg"></figure></div></article>"#;
const WATCH_FIXTURE: &str = r#"<ul class="anime_muti_link"><li data-video="https://example.invalid/embed"><div class="cdtr"><span>Latino</span></div></li></ul>"#;
