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

const SOURCE: Hackstore = Hackstore;
const BASE_URL: &str = "https://www.hackstore.fo";

struct Hackstore;

impl VideoSource for Hackstore {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_cards(&fetch(
            &format!("{BASE_URL}/peliculas/page/{}/", page(&request)),
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
            format!("{BASE_URL}/page/{p}/?s={}", url::query_escape(query))
        } else if let Some(kind) = filter(&request, "genre").filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/{kind}/page/{p}/")
        } else {
            format!("{BASE_URL}/peliculas/page/{p}/")
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/peliculas/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/peliculas/sample".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body, &path))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/peliculas/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for tab in doc.select(&selector("ul.TbVideoNv li.pres")) {
            let Some(playr) = tab.select(&selector("a.playr")).next() else {
                continue;
            };
            let server = text(playr);
            let lang_attr = attr(&playr, "data-lang")
                .unwrap_or_default()
                .to_ascii_lowercase();
            let prefix = if lang_attr.contains("latino") {
                "[LAT]"
            } else if lang_attr.contains("sub") || lang_attr.contains("japon") {
                "[SUB]"
            } else {
                "[CAST]"
            };
            let Some(deco) = attr(&playr, "data-href") else {
                continue;
            };
            let redirect_page = absolute_url(&deco);
            let redirect = extract_don_redirect(&redirect_page).unwrap_or(redirect_page);
            streams.extend(resolve_embed(
                &redirect,
                &format!("{prefix} {server}"),
                &referer,
                &request,
            ));
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
fn extract_don_redirect(url: &str) -> Option<String> {
    let body = fetch(url, "", BASE_URL);
    Regex::new(r#"window\.location\.href\s*=\s*'([^']+)'"#)
        .ok()?
        .captures(&body)?
        .get(1)
        .map(|m| m.as_str().to_string())
}
fn parse_cards(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc
            .select(&selector("div.movie-thumbnail"))
            .filter_map(card)
            .collect(),
        has_next_page: doc
            .select(&selector("div.wp-pagenavi .current ~ a"))
            .next()
            .is_some(),
    }
}
fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, ".movie-thumbnail a, a", "href")?;
    let title = select_attr(el, ".movie-title", "title")
        .or_else(|| select_text(el, ".movie-title"))
        .unwrap_or_else(|| title_from_path(&href));
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title,
        cover: select_attr(el, ".poster-pad img, img", "data-src")
            .or_else(|| select_attr(el, "img", "src"))
            .map(|v| absolute_url(&v)),
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
    let movie = path.contains("/peliculas/");
    let info = select_text_doc(&doc, ".watch-content .watch-text strong ~ p").unwrap_or_default();
    CatalogItem {
        key: path_key(path),
        title: if movie {
            info.split("Título Latino:")
                .nth(1)
                .and_then(|v| v.split(')').next())
                .map(|v| format!("{})", v.trim()))
                .unwrap_or_else(|| title_from_path(path))
        } else {
            select_text_doc(&doc, ".serieee h2, h1").unwrap_or_else(|| title_from_path(path))
        },
        cover: select_attr_doc(&doc, ".watch-content img, .imghacks, img", "data-src")
            .or_else(|| select_attr_doc(&doc, "img", "src"))
            .map(|v| absolute_url(&v)),
        url: Some(absolute_url(path)),
        description: select_text_doc(
            &doc,
            ".watch-content .watch-text p:nth-child(1), #pcontent > p, #zcontent > p",
        )
        .map(|v| v.trim_matches('"').to_string()),
        tags: if movie {
            csv_after(&info, "Genero:", "País")
        } else {
            select_texts_doc(&doc, "#ggenre [rel=tag]")
        },
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: if movie {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        },
        initialized: true,
        ..CatalogItem::default()
    }
}
fn parse_episodes(body: &str, path: &str) -> Vec<VideoEpisode> {
    if path.contains("/peliculas/") {
        return vec![VideoEpisode {
            key: path.to_string(),
            title: Some("PELICULA".to_string()),
            episode_number: Some(1.0),
            url: Some(absolute_url(path)),
            language: Some("es".to_string()),
            ..VideoEpisode::default()
        }];
    }
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    for (idx, thumb) in doc.select(&selector(".movie-thumbnail")).enumerate() {
        let href = select_attr(thumb, "a", "href").unwrap_or_default();
        if href.is_empty() {
            continue;
        }
        let (season, ep) = Regex::new(r#"-(\d+)x(\d+)/?$"#)
            .unwrap()
            .captures(&href)
            .map(|c| {
                (
                    c[1].parse::<i32>().unwrap_or(0),
                    c[2].parse::<i32>().unwrap_or(0),
                )
            })
            .unwrap_or((0, idx as i32 + 1));
        let key = path_key(&href);
        out.push(VideoEpisode {
            key: key.clone(),
            title: Some(format!("T{season} - E{ep}")),
            episode_number: Some((idx + 1) as f32),
            url: Some(absolute_url(&key)),
            language: Some("es".to_string()),
            ..VideoEpisode::default()
        });
    }
    out.reverse();
    out
}
fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    if embed.contains(".m3u8") {
        return parse_hls(embed, name, referer, request);
    }
    let body = fetch(embed, "", referer);
    if let Some(media) = first_media_url(&body).map(|v| absolute_remote(&v, embed)) {
        if media.contains(".m3u8") {
            parse_hls(&media, name, embed, request)
        } else {
            vec![stream(&media, name, "direct", embed, false)]
        }
    } else {
        vec![external_stream(embed, name, referer)]
    }
}
fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)"#,
        r#"src\s*:\s*["']([^"']+)"#,
        r#"<source[^>]+src=["']([^"']+)"#,
        r#"url\s*=\s*["']([^"']+)"#,
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
    let mut quality = pref(request, "preferred_quality", "auto");
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
        out.push(stream(master, name, &quality, referer, true));
    }
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
    let lang = pref(request, "preferred_language", "[LAT]");
    let server = pref(request, "preferred_server", "StreamWish").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|s| {
        let n = s.name.clone().unwrap_or_default();
        (
            n.contains(&lang),
            n.to_ascii_lowercase().contains(&server),
            n.contains(&quality),
            quality_rank(&n),
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
fn csv_after(input: &str, start: &str, end: &str) -> Vec<String> {
    input
        .split(start)
        .nth(1)
        .and_then(|v| v.split(end).next())
        .unwrap_or_default()
        .split(',')
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}
fn quality_rank(input: &str) -> i32 {
    Regex::new(r#"(\d+)"#)
        .unwrap()
        .captures(input)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0)
}
fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("Hackstore")
        .replace('-', " ")
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="movie-thumbnail"><a href="/peliculas/sample/"><div class="movie-title" title="Sample"></div><div class="poster-pad"><img data-src="/sample.jpg"></div></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="watch-content"><img data-src="/sample.jpg"><div class="watch-text"><p>Sample description.</p><strong></strong><p>Título Latino: Sample) Genero: Accion País</p></div></div>"#;
const WATCH_FIXTURE: &str = r##"<ul class="TbVideoNv"><li class="pres"><a class="playr" data-lang="latino" data-href="/redirect" href="#">streamwish</a></li></ul>"##;
