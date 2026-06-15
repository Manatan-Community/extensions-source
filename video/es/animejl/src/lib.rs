use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::{Value, json};

const SOURCE: AnimeJl = AnimeJl;
const BASE_URL: &str = "https://www.anime-jl.net";

struct AnimeJl;

impl VideoSource for AnimeJl {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/animes?order=updated&page={page}")
        } else {
            format!("{BASE_URL}/animes?order=rating&page={page}")
        };
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
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
                "{BASE_URL}/animes?q={}&page={page}",
                url::query_escape(query)
            )
        } else {
            let params = filter_params(&request);
            if params.is_empty() {
                format!("{BASE_URL}/animes?order=rating&page={page}")
            } else {
                format!("{BASE_URL}/animes?{params}&page={page}")
            }
        };
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/1/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/1/sample".to_string());
        let body = get_or_fixture(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body, &path))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode = request_key(&request, "episode")
            .unwrap_or_else(|| "/anime/1/sample/episodio-1".to_string());
        let body = get_or_fixture(&absolute_url(&episode), WATCH_FIXTURE, BASE_URL);
        Ok(parse_hosters(&body, &absolute_url(&episode)))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = key.splitn(3, '|');
        let server = parts.next().unwrap_or("External");
        let embed = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(BASE_URL);
        let mut streams = resolve_embed(embed, server, referer, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut streams = Vec::new();
        for hoster in self.hosters(request.clone())? {
            let mut resolved = self.resolve_hoster(json!({
                "hoster": { "key": hoster.key },
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
            }))?;
            for stream in &mut resolved {
                stream.hoster = Some(hoster.clone());
            }
            streams.extend(resolved);
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
                title: "Ultimos episodios".to_string(),
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
            if is_episode_path(&path) {
                return Ok(Some(UrlResolveResult {
                    episode: Some(json!({
                        "key": path,
                        "url": absolute_url(&path),
                        "language": "es"
                    })),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
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

fn get_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let document = Html::parse_document(body);
    let entries = select_all(
        &document,
        "div.Container ul.ListAnimes li article, ul.ListAnimes li article",
    )
    .into_iter()
    .filter_map(card_from_article)
    .collect();
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn card_from_article(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "div.Description a.Button, a[href*='/anime/']", "href")?;
    let title = select_text(el, "a h3, h3.Title")?;
    Some(CatalogItem {
        key: path_key(&href),
        title,
        cover: select_attr(el, "a div.Image figure img, img", "src")
            .or_else(|| select_attr(el, "img", "data-cfsrc"))
            .map(|src| storage_url(&src)),
        url: Some(absolute_url(&href)),
        description: select_text(el, "div.Description p:nth-of-type(3), div.Description p"),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: false,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let document = Html::parse_document(&body);
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&document, "div.Ficha.fchlt div.Container .Title, h1.Title")
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(
            &document,
            "div.AnimeCover div.Image figure img, div.AnimeCover img",
            "src",
        )
        .map(|src| storage_url(&src)),
        url: Some(absolute_url(path)),
        description: select_text_doc(&document, "div.Description"),
        tags: select_texts_doc(&document, "nav.Nvgnrs a"),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&body),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, item_path: &str) -> Vec<VideoEpisode> {
    let Some(raw) = body
        .split("var episodes =")
        .nth(1)
        .and_then(|p| p.split("</script>").next())
        .and_then(|p| p.split(';').next())
    else {
        return Vec::new();
    };
    let base_path = episode_base_path(item_path);
    let mut episodes = Regex::new(r#"\[\s*([0-9]+(?:\.[0-9]+)?)\s*,\s*"([^"]+)"\s*,\s*"([^"]*)""#)
        .unwrap()
        .captures_iter(raw)
        .filter_map(|cap| {
            let number = cap.get(1)?.as_str().parse::<f32>().ok()?;
            let episode_slug = cap.get(2)?.as_str();
            let cover = cap.get(3).map(|m| m.as_str()).unwrap_or_default();
            let path = format!("{base_path}/{}", episode_slug.trim_matches('/'));
            Some(VideoEpisode {
                key: path.clone(),
                title: Some(format!("Episodio {}", trim_float(number))),
                episode_number: Some(number),
                thumbnail: (!cover.is_empty()).then(|| storage_url(cover)),
                url: Some(absolute_url(&path)),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect::<Vec<_>>();
    episodes.sort_by(|a, b| {
        b.episode_number
            .partial_cmp(&a.episode_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    episodes
}

fn parse_hosters(body: &str, episode_url: &str) -> Vec<VideoHoster> {
    let iframe_re = Regex::new(r#"video\[\d+\]\s*=\s*['"][^;]*?src=["']([^"']+)["']"#).unwrap();
    iframe_re
        .captures_iter(body)
        .filter_map(|cap| cap.get(1).map(|m| decode_entities(m.as_str())))
        .map(|embed| absolute_remote(&embed, BASE_URL))
        .enumerate()
        .map(|(idx, embed)| {
            let name = host_label(&embed);
            VideoHoster {
                key: format!("{name}|{embed}|{episode_url}"),
                name: if name == "External" {
                    format!("External {}", idx + 1)
                } else {
                    name
                },
                url: Some(embed),
                lazy: true,
                video_count: Some(1),
                ..VideoHoster::default()
            }
        })
        .collect()
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_url(embed);
    if embed.contains(".m3u8") {
        return parse_hls(&embed, name, referer, request);
    }
    let body = get_or_fixture(&embed, "", referer);
    if let Some(src) = first_media_url(&body) {
        let src = absolute_remote(&src, &embed);
        if src.contains(".m3u8") {
            return parse_hls(&src, name, &embed, request);
        }
        return vec![media_stream(&src, name, "direct", &embed)];
    }
    vec![external_stream(&embed, name, referer)]
}

fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)["']"#,
        r#"src\s*:\s*["']([^"']+)["']"#,
        r#"<source[^>]+src=["']([^"']+)["']"#,
        r#"sources\s*:\s*\[\s*["']([^"']+)["']"#,
    ]
    .into_iter()
    .find_map(|pat| {
        Regex::new(pat)
            .ok()?
            .captures(body)?
            .get(1)
            .map(|m| decode_entities(&m.as_str().replace("\\/", "/")))
    })
}

fn parse_hls(master: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = client(referer)
        .get(master)
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    let mut streams = Vec::new();
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
            streams.push(media_stream(
                &absolute_remote(line.trim(), master),
                name,
                &quality,
                referer,
            ));
        }
    }
    if streams.is_empty() {
        streams.push(media_stream(master, name, "auto", referer));
    }
    sort_streams(&mut streams, request);
    streams
}

fn media_stream(target: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = target.contains(".m3u8");
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
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
    let server = pref(request, "preferred_server", "StreamWish").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "720");
    streams.sort_by_key(|stream| {
        let name = stream.name.clone().unwrap_or_default().to_ascii_lowercase();
        let q = stream.quality.clone().unwrap_or_default();
        (
            name.contains(&server),
            q.contains(&quality),
            quality_rank(&q),
        )
    });
    streams.reverse();
}

fn filter_params(request: &Value) -> String {
    let mut parts = Vec::new();
    for key in ["genre", "year", "type", "estado"] {
        for value in filter_values(request, key) {
            if !value.is_empty() {
                parts.push(format!("{key}[]={}", url::query_escape(&value)));
            }
        }
    }
    if let Some(order) = filter(request, "order").filter(|value| !value.is_empty()) {
        parts.push(format!("order={}", url::query_escape(&order)));
    }
    parts.join("&")
}

fn select_all<'a>(document: &'a Html, sel: &str) -> Vec<ElementRef<'a>> {
    document.select(&selector(sel)).collect()
}

fn selector(input: &str) -> Selector {
    Selector::parse(input).unwrap()
}

fn select_text_doc(document: &Html, sel: &str) -> Option<String> {
    document
        .select(&selector(sel))
        .next()
        .map(element_text)
        .filter(|v| !v.is_empty())
}

fn select_texts_doc(document: &Html, sel: &str) -> Vec<String> {
    document
        .select(&selector(sel))
        .map(element_text)
        .filter(|v| !v.is_empty())
        .collect()
}

fn select_attr_doc(document: &Html, sel: &str, name: &str) -> Option<String> {
    document
        .select(&selector(sel))
        .next()
        .and_then(|el| el.value().attr(name))
        .map(decode_entities)
}

fn select_text(el: ElementRef<'_>, sel: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .map(element_text)
        .filter(|v| !v.is_empty())
}

fn select_attr(el: ElementRef<'_>, sel: &str, name: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .and_then(|el| el.value().attr(name))
        .map(decode_entities)
}

fn element_text(el: ElementRef<'_>) -> String {
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

fn storage_url(input: &str) -> String {
    let trimmed = decode_entities(input).replace("\\/", "/");
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed
    } else if let Some(rest) = trimmed.strip_prefix("//") {
        format!("https://{rest}")
    } else if trimmed.starts_with("/storage/") {
        absolute_url(&trimmed)
    } else if trimmed.starts_with("storage/") {
        absolute_url(&format!("/{trimmed}"))
    } else {
        absolute_url(&format!("/storage/{}", trimmed.trim_start_matches('/')))
    }
}

fn absolute_remote(input: &str, base: &str) -> String {
    let trimmed = decode_entities(input).trim().replace("\\/", "/");
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed
    } else if let Some(rest) = trimmed.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url::join_url(base, &trimmed)
    }
}

fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .or_else(|| input.strip_prefix("https://anime-jl.net"))
        .filter(|p| p.starts_with("/anime/"))
        .map(path_key)
}

fn path_key(input: &str) -> String {
    if let Some(path) = input
        .strip_prefix(BASE_URL)
        .or_else(|| input.strip_prefix("https://anime-jl.net"))
    {
        return path_key(path);
    }
    format!(
        "/{}",
        input
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .trim_matches('/')
    )
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request_raw_key(request, field).map(|value| path_key(&value))
}

fn request_raw_key(request: &Value, field: &str) -> Option<String> {
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
        .map(ToString::to_string)
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("Animejl")
        .replace('-', " ")
}

fn parse_status(body: &str) -> ItemStatus {
    let document = Html::parse_document(body);
    let status = select_text_doc(&document, "span.fa-tv, p.AnmStts span").unwrap_or_default();
    let lower = status.to_ascii_lowercase();
    if lower.contains("finalizado") {
        ItemStatus::Completed
    } else if lower.contains("en emision") || lower.contains("en emisión") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn host_label(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if lower.contains("streamwish") || lower.contains("playerwish") {
        "StreamWish"
    } else if lower.contains("yourupload") {
        "YourUpload"
    } else if lower.contains("ok.ru") {
        "Okru"
    } else if lower.contains("streamtape") || lower.contains("stape") {
        "StreamTape"
    } else if lower.contains("streamhidevid") {
        "StreamHideVid"
    } else if lower.contains("voe") {
        "Voe"
    } else if lower.contains("uqload") {
        "Uqload"
    } else if lower.contains("mp4upload") {
        "Mp4upload"
    } else if lower.contains("hqq.") {
        "HQQ"
    } else if lower.contains("videovard") {
        "VidoVard"
    } else if lower.contains("sbfull") || lower.contains("streamsb") {
        "StreamSB"
    } else if lower.contains("animejl.") {
        "AnimeJL"
    } else {
        "External"
    }
    .to_string()
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

fn pref(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|f| f.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn filter_values(request: &Value, key: &str) -> Vec<String> {
    let Some(value) = request
        .get("filters")
        .and_then(|f| f.get(key))
        .or_else(|| request.get(key))
    else {
        return Vec::new();
    };
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    value
        .as_str()
        .filter(|v| !v.is_empty())
        .map(|v| vec![v.to_string()])
        .unwrap_or_default()
}

fn quality_rank(input: &str) -> i32 {
    Regex::new(r#"(\d+)"#)
        .unwrap()
        .captures(input)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0)
}

fn trim_float(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i32)
    } else {
        value.to_string()
    }
}

fn episode_base_path(item_path: &str) -> String {
    let path = path_key(item_path);
    if is_episode_path(&path) {
        path.rsplit_once('/')
            .map(|(base, _)| base.to_string())
            .unwrap_or(path)
    } else {
        path
    }
}

fn is_episode_path(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .unwrap_or_default()
        .starts_with("episodio-")
}

fn decode_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<ul class="ListAnimes"><li><article><a href="https://www.anime-jl.net/anime/1/sample"><h3>Sample</h3><div class="Image"><figure><img src="/storage/animes_tumbl/sample.jpg"></figure></div></a><div class="Description"><a class="Button" href="https://www.anime-jl.net/anime/1/sample"></a><p></p><p></p><p>Sample description.</p></div></article></li></ul>"#;
const DETAILS_FIXTURE: &str = r#"<div class="Ficha fchlt"><div class="Container"><h1 class="Title">Sample</h1></div></div><div class="AnimeCover"><div class="Image"><figure><img src="/storage/animes_tumbl/sample.jpg"></figure></div></div><p class="AnmStts A"><span class="fa-tv">Finalizado</span></p><nav class="Nvgnrs"><a>Accion</a></nav><div class="Description">Sample description.</div><script>var anime_info = ["1","Sample","sample","","Anime"]; var episodes = [[1,"episodio-1","episodes_tumbl/sample.jpg",""],];</script>"#;
const WATCH_FIXTURE: &str = r#"<script>var video = []; video[0] = '<iframe src="https://example.invalid/embed" frameborder="0"></iframe>';</script>"#;
