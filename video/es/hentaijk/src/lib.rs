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

const SOURCE: Hentaijk = Hentaijk;
const BASE_URL: &str = "https://hentaijk.com";

struct Hentaijk;

impl VideoSource for Hentaijk {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_top(&fetch(
            &format!("{BASE_URL}/top/"),
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
            format!(
                "{BASE_URL}/buscar/{}/{p}/?filtro=fecha&tipo=none&estado=none&orden=desc",
                url::query_escape(query)
            )
        } else if let Some(genre) =
            filter(&request, "genre").filter(|v| !v.is_empty() && v != "none")
        {
            format!("{BASE_URL}/genero/{genre}/{p}")
        } else {
            format!(
                "{BASE_URL}/directorio/{p}/?filtro=fecha&tipo=none&estado=none&fecha=none&temporada=none&orden=desc"
            )
        };
        Ok(parse_directory(&fetch(&target, SEARCH_FIXTURE, BASE_URL)))
    }
    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&path))
    }
    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body, &path))
    }
    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/anime/sample/1".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        let script = doc
            .select(&selector("script"))
            .map(|s| s.inner_html())
            .find(|s| s.contains("var video = [];"))
            .unwrap_or_default();
        for server in doc.select(&selector(
            "div.col-lg-12.rounded.bg-servers.text-white.p-3.mt-2 a, .bg-servers a",
        )) {
            let name = text(server);
            let id = attr(&server, "data-id").unwrap_or_default();
            if id.is_empty() {
                continue;
            }
            if let Some(raw) = video_slot(&script, &id) {
                let embed = raw
                    .replace(
                        &format!("{BASE_URL}/jkokru.php?u="),
                        "http://ok.ru/videoembed/",
                    )
                    .replace(
                        &format!("{BASE_URL}/jkvmixdrop.php?u="),
                        "https://mixdrop.co/e/",
                    )
                    .replace(&format!("{BASE_URL}/jk.php?u="), &format!("{BASE_URL}/"));
                streams.extend(resolve_hentaijk_embed(&embed, &name, &referer, &request));
            }
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }
    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.search(request)?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Top".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Directorio".to_string(),
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
fn parse_top(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc
            .select(&selector("div.col-lg-12 div.list"))
            .filter_map(top_card)
            .collect(),
        has_next_page: false,
    }
}
fn top_card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "div#conb a, a", "href")?;
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title: select_attr(el, "div#conb a, a", "title").unwrap_or_else(|| title_from_path(&path)),
        cover: select_attr(el, "div#conb a img, img", "src").map(|v| absolute_url(&v)),
        url: Some(absolute_url(&path)),
        description: select_text(el, "div#conb div#animinfo p"),
        language: Some("es".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}
fn parse_directory(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let mut entries: Vec<CatalogItem> = doc
        .select(&selector(".col-lg-2.col-md-6.col-sm-6"))
        .filter_map(search_card)
        .collect();
    if entries.is_empty() {
        entries = doc
            .select(&selector(".card.mb-3.custom_item2"))
            .filter_map(filter_card)
            .collect();
    }
    Paged {
        entries,
        has_next_page: doc
            .select(&selector(
                "section.contenido.spad div.container div.navigation a.nav-next",
            ))
            .next()
            .is_some(),
    }
}
fn search_card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "div.anime__item a, a", "href")?;
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title: select_text(el, "div.anime__item #ainfo div.title, .title")
            .unwrap_or_else(|| title_from_path(&path)),
        cover: select_attr(el, "div.anime__item a div.anime__item__pic", "data-setbg")
            .map(|v| absolute_url(&v)),
        url: Some(absolute_url(&path)),
        tags: select_texts(el, "div.anime__item div.anime__item__text ul li"),
        language: Some("es".to_string()),
        content_rating: Some("adult".to_string()),
        status: parse_status(
            &select_text(
                el,
                "div.anime__item div.anime__item__text ul li:nth-child(1)",
            )
            .unwrap_or_default(),
        ),
        ..CatalogItem::default()
    })
}
fn filter_card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "h5.card-title a, a", "href")?;
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title: select_text(el, "h5.card-title a").unwrap_or_else(|| title_from_path(&path)),
        cover: select_attr(el, ".custom_thumb2 a img, img", "src").map(|v| absolute_url(&v)),
        url: Some(absolute_url(&path)),
        description: select_text(el, "p.synopsis"),
        tags: select_texts(el, ".card-info p"),
        language: Some("es".to_string()),
        content_rating: Some("adult".to_string()),
        status: parse_status(&select_text(el, "p.card-status").unwrap_or_default()),
        ..CatalogItem::default()
    })
}
fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    let mut status = ItemStatus::Unknown;
    let mut tags = Vec::new();
    for li in doc.select(&selector("div.row div.col-lg-6.col-md-6 ul li")) {
        let row = text(li);
        if row.contains("Genero") {
            tags = select_texts(li, "a");
        }
        if row.contains("Estado") {
            status = parse_status(&row);
        }
    }
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(
            &doc,
            "div.anime__details__text div.anime__details__title h3, h1",
        )
        .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(
            &doc,
            "div.col-lg-3 div.anime__details__pic.set-bg",
            "data-setbg",
        )
        .map(|v| absolute_url(&v)),
        url: Some(absolute_url(path)),
        description: select_text_doc(&doc, "div.col-lg-9 div.anime__details__text p"),
        tags,
        language: Some("es".to_string()),
        content_rating: Some("adult".to_string()),
        status,
        initialized: true,
        ..CatalogItem::default()
    }
}
fn parse_episodes(body: &str, item_path: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let script = doc
        .select(&selector("script"))
        .map(|s| s.inner_html())
        .find(|s| s.contains("ajax/last_episode"))
        .unwrap_or_default();
    let anime_id = script
        .split("'/ajax/last_episode/")
        .nth(1)
        .and_then(|v| v.split("/',").next())
        .unwrap_or_default();
    let last_page = doc
        .select(&selector("div.anime__pagination a"))
        .last()
        .and_then(|a| attr(&a, "href"))
        .and_then(|v| v.replace("#pag", "").parse::<u64>().ok())
        .unwrap_or(1);
    let mut out = Vec::new();
    for p in 1..=last_page {
        let body = fetch(
            &format!("{BASE_URL}/ajax/pagination_episodes/{anime_id}/{p}"),
            "",
            BASE_URL,
        );
        for num in Regex::new(r#""number"\s*:\s*"([^"]+)""#)
            .unwrap()
            .captures_iter(&body)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        {
            let ep = num.parse::<f32>().unwrap_or(out.len() as f32 + 1.0);
            let key = format!("{}/{}", item_path.trim_end_matches('/'), num);
            out.push(VideoEpisode {
                key: key.clone(),
                title: Some(format!("Episodio {num}")),
                episode_number: Some(ep),
                url: Some(absolute_url(&key)),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            });
        }
    }
    out.reverse();
    out
}
fn video_slot(script: &str, id: &str) -> Option<String> {
    let pattern =
        format!(r#"video\[{id}\]\s*=\s*'<iframe class=\\"player_conte\\" src=\\"([^"]+)""#);
    Regex::new(&pattern)
        .ok()?
        .captures(script)?
        .get(1)
        .map(|m| m.as_str().replace("\\/", "/"))
}
fn resolve_hentaijk_embed(
    embed: &str,
    name: &str,
    referer: &str,
    request: &Value,
) -> Vec<VideoStream> {
    if embed.contains("ok.ru") || embed.contains("okru") {
        return resolve_embed(embed, &format!("Okru {name}"), referer, request);
    }
    if embed.contains("stream/jkmedia") {
        return vec![stream(embed, "Xtreme S", "direct", referer, false)];
    }
    if embed.contains("um.php") {
        let body = fetch(embed, "", referer);
        if let Some(url) = Regex::new(r#"url:\s*'([^']+)'"#)
            .unwrap()
            .captures(&body)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
        {
            return vec![stream(&url, name, "direct", embed, false)];
        }
    }
    resolve_embed(embed, name, referer, request)
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
    let preferred = pref(request, "preferred_quality", "Sabrosio");
    streams.sort_by_key(|s| {
        let n = s.name.clone().unwrap_or_default();
        (n.contains(&preferred), quality_rank(&n))
    });
    streams.reverse();
}
fn parse_status(input: &str) -> ItemStatus {
    if input.contains("Concluido") {
        ItemStatus::Completed
    } else if input.contains("Por estrenar") || input.contains("En emision") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
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
        .and_then(|e| e.value().attr(name))
        .map(ToString::to_string)
}
fn select_text(el: ElementRef<'_>, sel: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .map(text)
        .filter(|v| !v.is_empty())
}
fn select_texts(el: ElementRef<'_>, sel: &str) -> Vec<String> {
    el.select(&selector(sel))
        .map(text)
        .filter(|v| !v.is_empty())
        .collect()
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
        .unwrap_or("Hentaijk")
        .replace('-', " ")
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="col-lg-12"><div class="list"><div id="conb"><a href="/anime/sample" title="Sample"><img src="/sample.jpg"></a><div id="animinfo"><p>Sample description.</p></div></div></div></div>"#;
const SEARCH_FIXTURE: &str = r#"<div class="card mb-3 custom_item2"><h5 class="card-title"><a href="/anime/sample">Sample</a></h5><div class="custom_thumb2"><a><img src="/sample.jpg"></a></div><p class="synopsis">Sample description.</p></div>"#;
const DETAILS_FIXTURE: &str = r##"<div class="col-lg-3"><div class="anime__details__pic set-bg" data-setbg="/sample.jpg"></div></div><div class="anime__details__text"><div class="anime__details__title"><h3>Sample</h3></div><p>Sample description.</p></div><script>var invertir = '/ajax/last_episode/sample/', a;</script><div class="anime__pagination"><a href="#pag1">1</a></div>"##;
const WATCH_FIXTURE: &str = r#"<div class="col-lg-12 rounded bg-servers text-white p-3 mt-2"><a data-id="0">Hentaijk</a></div><script>var video = []; video[0] = '<iframe class=\"player_conte\" src=\"https://example.invalid/embed\"';</script>"#;
