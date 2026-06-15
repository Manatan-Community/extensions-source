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
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Hentaila = Hentaila;
const BASE_URL: &str = "https://hentaila.com";
const CDN_BASE_URL: &str = "https://cdn.hentaila.com";

struct Hentaila;

impl VideoSource for Hentaila {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let order = if listing(&request) == "latest" {
            "latest_released"
        } else {
            "popular"
        };
        Ok(parse_listing(&fetch(
            &format!(
                "{BASE_URL}/catalogo/__data.json?order={order}&page={}",
                page(&request)
            ),
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
        let mut params = vec![format!("page={}", page(&request))];
        if !query.is_empty() {
            params.push(format!("search={}", url::query_escape(query)));
        } else {
            for (key, out_key) in [
                ("genre", "genre"),
                ("order", "filter"),
                ("status", "status"),
            ] {
                if let Some(value) = filter(&request, key).filter(|v| !v.is_empty()) {
                    params.push(format!("{out_key}={}", url::query_escape(&value)));
                }
            }
            if filter(&request, "uncensored")
                .filter(|v| !v.is_empty())
                .is_some()
            {
                params.push("uncensored=".to_string());
            }
        }
        Ok(parse_listing(&fetch(
            &format!("{BASE_URL}/catalogo/__data.json?{}", params.join("&")),
            LIST_FIXTURE,
            BASE_URL,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/media/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/media/sample".to_string());
        Ok(parse_episodes(
            &fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL),
            &path,
        ))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/media/sample/1".to_string());
        let body = fetch(
            &format!("{}{}{}", BASE_URL, path, "/__data.json"),
            WATCH_FIXTURE,
            BASE_URL,
        );
        let mut streams = Vec::new();
        for (server, embed) in parse_video_data(&body) {
            streams.extend(resolve_hentaila_embed(
                &embed,
                &server,
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
                title: "Recientes".to_string(),
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

#[derive(Deserialize)]
struct DataRoot {
    nodes: Vec<Option<DataNode>>,
}
#[derive(Deserialize)]
struct DataNode {
    data: Option<Vec<Value>>,
    uses: Option<Value>,
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
    if let Ok(root) = serde_json::from_str::<DataRoot>(body) {
        for node in root.nodes.into_iter().flatten() {
            if node
                .uses
                .as_ref()
                .is_some_and(|u| u.get("search_params").is_some())
            {
                if let Some(page) = listing_from_data(node.data.unwrap_or_default()) {
                    return page;
                }
            }
        }
    }
    parse_cards_html(body)
}
fn listing_from_data(data: Vec<Value>) -> Option<Paged<CatalogItem>> {
    let root = data.first()?.as_object()?;
    let ids = data
        .get(root.get("results")?.as_i64()? as usize)?
        .as_array()?;
    let pagination = root
        .get("pagination")
        .and_then(Value::as_i64)
        .and_then(|i| data.get(i as usize))
        .and_then(Value::as_object);
    let current = pagination
        .and_then(|p| p.get("currentPage"))
        .and_then(Value::as_i64)
        .and_then(|i| data.get(i as usize))
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let total = pagination
        .and_then(|p| p.get("totalPages"))
        .and_then(Value::as_i64)
        .and_then(|i| data.get(i as usize))
        .and_then(Value::as_i64)
        .unwrap_or(current);
    let entries = ids
        .iter()
        .filter_map(|id| {
            let obj = data.get(id.as_i64()? as usize)?.as_object()?;
            let title = data_string(&data, obj.get("title")?)?;
            let slug = data_string(&data, obj.get("slug")?)?;
            let media_id = data_string(&data, obj.get("id")?);
            Some(CatalogItem {
                key: format!("/media/{slug}"),
                title,
                cover: media_id.map(|id| format!("{CDN_BASE_URL}/covers/{id}.jpg")),
                description: obj.get("synopsis").and_then(|v| data_string(&data, v)),
                url: Some(format!("{BASE_URL}/media/{slug}")),
                language: Some("es".to_string()),
                content_rating: Some("adult".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect();
    Some(Paged {
        entries,
        has_next_page: current < total,
    })
}
fn data_string(data: &[Value], value: &Value) -> Option<String> {
    value.as_str().map(ToString::to_string).or_else(|| {
        value
            .as_i64()
            .and_then(|i| data.get(i as usize))
            .and_then(Value::as_str)
            .map(ToString::to_string)
    })
}
fn parse_cards_html(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc
            .select(&selector("article"))
            .filter_map(card_from_article)
            .collect(),
        has_next_page: body.contains("totalPages") || body.contains("pagination"),
    }
}
fn card_from_article(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "a[href]", "href")?;
    let title = select_text(el, "h3, h2").or_else(|| select_attr(el, "img", "alt"))?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: select_attr(el, "img", "src").map(|s| absolute_url(&s)),
        url: Some(absolute_url(&key)),
        language: Some("es".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}
fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, ".grid.items-start h1.text-lead, h1")
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(
            &doc,
            "img.object-cover.w-full.aspect-poster, img[src*='/covers/']",
            "src",
        )
        .map(|s| absolute_url(&s)),
        description: select_text_doc(&doc, ".entry.text-lead.text-sm p, .entry p"),
        tags: select_texts_doc(
            &doc,
            ".flex-wrap.items-center .btn.btn-xs.rounded-full:not(.sm\\:w-auto), a[href*='genre=']",
        ),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("adult".to_string()),
        status: if body.contains("En emisión") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Completed
        },
        initialized: true,
        ..CatalogItem::default()
    }
}
fn parse_episodes(body: &str, item_path: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let slug = item_path
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("sample");
    let mut out = Vec::new();
    for article in doc.select(&selector("article.group\\/item, article")) {
        let num = select_text(article, "div.bg-line.text-subs span, span")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(out.len() as f32 + 1.0);
        let key = format!("/media/{slug}/{}", trim_float(num));
        out.push(VideoEpisode {
            key: key.clone(),
            title: Some(format!("Episodio {}", trim_float(num))),
            episode_number: Some(num),
            url: Some(absolute_url(&key)),
            language: Some("es".to_string()),
            ..VideoEpisode::default()
        });
    }
    if out.is_empty() {
        for cap in Regex::new(r#"href=["'](/media/[^"']+/([0-9]+(?:\.[0-9]+)?))"#)
            .unwrap()
            .captures_iter(body)
        {
            let key = cap[1].to_string();
            let num = cap[2].parse::<f32>().ok();
            out.push(VideoEpisode {
                key: key.clone(),
                title: num.map(|n| format!("Episodio {}", trim_float(n))),
                episode_number: num,
                url: Some(absolute_url(&key)),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            });
        }
    }
    out.reverse();
    out
}
fn parse_video_data(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(root) = serde_json::from_str::<DataRoot>(body) {
        for node in root.nodes.into_iter().flatten() {
            if !node
                .uses
                .as_ref()
                .is_some_and(|u| u.get("params").is_some())
            {
                continue;
            }
            let data = node.data.unwrap_or_default();
            let Some(root) = data.first().and_then(Value::as_object) else {
                continue;
            };
            let Some(embed_obj) = root
                .get("embeds")
                .and_then(Value::as_i64)
                .and_then(|i| data.get(i as usize))
                .and_then(Value::as_object)
            else {
                continue;
            };
            for lang in ["SUB", "LAT", "CAST", "DUB"] {
                let Some(arr) = embed_obj
                    .get(lang)
                    .and_then(Value::as_i64)
                    .and_then(|i| data.get(i as usize))
                    .and_then(Value::as_array)
                else {
                    continue;
                };
                for item in arr {
                    let Some(obj) = item
                        .as_i64()
                        .and_then(|i| data.get(i as usize))
                        .and_then(Value::as_object)
                    else {
                        continue;
                    };
                    if let (Some(server), Some(link)) = (
                        obj.get("server").and_then(|v| data_string(&data, v)),
                        obj.get("url").and_then(|v| data_string(&data, v)),
                    ) {
                        out.push((server, link));
                    }
                }
            }
        }
    }
    if out.is_empty() {
        for cap in Regex::new(r#"\{server:"([^"]+)",url:"([^"]+)"}"#)
            .unwrap()
            .captures_iter(body)
        {
            out.push((cap[1].to_string(), cap[2].replace("\\u0026", "&")));
        }
    }
    out
}
fn resolve_hentaila_embed(
    embed: &str,
    server: &str,
    referer: &str,
    request: &Value,
) -> Vec<VideoStream> {
    if server.eq_ignore_ascii_case("vip") {
        return resolve_embed(&embed.replace("/play/", "/m3u8/"), "VIP", referer, request);
    }
    if server.eq_ignore_ascii_case("arc") {
        let target = embed.split('#').nth(1).unwrap_or(embed);
        return vec![stream(target, "Arc", "direct", referer, false)];
    }
    resolve_embed(embed, server, referer, request)
}
fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_remote(embed, BASE_URL);
    if embed.contains(".m3u8") {
        return parse_hls(&embed, name, referer, request);
    }
    let body = fetch(&embed, "", referer);
    if let Some(src) = first_media_url(&body).map(|s| absolute_remote(&s, &embed)) {
        if src.contains(".m3u8") {
            parse_hls(&src, name, &embed, request)
        } else {
            vec![stream(&src, name, "direct", &embed, false)]
        }
    } else {
        vec![external_stream(&embed, name, referer)]
    }
}
fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)"#,
        r#"src\s*:\s*["']([^"']+)"#,
        r#"<source[^>]+src=["']([^"']+)"#,
        r#"url\s*:\s*["']([^"']+)"#,
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
    streams.sort_by_key(|s| {
        let n = s.name.clone().unwrap_or_default().to_ascii_lowercase();
        (n.contains(&server), n.contains(&quality), quality_rank(&n))
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
        .filter(|p| p.starts_with("/media/"))
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
fn referer_headers(referer: &str) -> Context {
    let mut h = Context::new();
    h.insert("Referer".to_string(), referer.to_string());
    h
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
        .unwrap_or("Hentaila")
        .replace('-', " ")
}
fn trim_float(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i32)
    } else {
        value.to_string()
    }
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"type":"data","nodes":[{"type":"data","uses":{"search_params":["page"]},"data":[{"results":1,"pagination":5},[2],{"id":3,"title":4,"slug":6,"synopsis":7},"1","Sample",{"currentPage":8,"totalPages":9},"sample","Sample synopsis",1,1]}]}"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="text-lead">Sample</h1><div class="entry text-lead text-sm"><p>Sample synopsis.</p></div><img class="object-cover w-full aspect-poster" src="https://cdn.hentaila.com/covers/1.jpg"><article class="group/item"><div class="bg-line text-subs"><span>1</span></div></article>"#;
const WATCH_FIXTURE: &str = r#"{"type":"data","nodes":[{"type":"data","uses":{"params":["number"]},"data":[{"embeds":1},{"SUB":2},[3],{"server":4,"url":5},"Arc","https://example.invalid/video.mp4#https://example.invalid/video.mp4"]}]}"#;
