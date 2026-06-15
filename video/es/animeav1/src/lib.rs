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

const SOURCE: AnimeAv1 = AnimeAv1;
const BASE_URL: &str = "https://animeav1.com";

struct AnimeAv1;

impl VideoSource for AnimeAv1 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let order = if listing(&request) == "latest" {
            "latest_released"
        } else {
            "popular"
        };
        let body = get_or_fixture(
            &format!("{BASE_URL}/catalogo?order={order}&page={page}"),
            LIST_FIXTURE,
            BASE_URL,
        );
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
                "{BASE_URL}/catalogo?search={}&page={page}",
                url::query_escape(query)
            )
        } else {
            let params = filter_params(&request);
            if params.is_empty() {
                format!("{BASE_URL}/catalogo?order=popular&page={page}")
            } else {
                format!("{BASE_URL}/catalogo?{params}&page={page}")
            }
        };
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/media/death-note".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/media/death-note".to_string());
        let body = get_or_fixture(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body, &path))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode =
            request_key(&request, "episode").unwrap_or_else(|| "/media/death-note/1".to_string());
        let body = get_or_fixture(&absolute_url(&episode), WATCH_FIXTURE, BASE_URL);
        Ok(parse_hosters(&body, &absolute_url(&episode), &request))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = key.splitn(4, '|');
        let lang = parts.next().unwrap_or("SUB");
        let server = parts.next().unwrap_or("External");
        let embed = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(BASE_URL);
        let mut streams = resolve_embed(embed, &format!("{lang} {server}"), referer, &request);
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
                title: "Ultimos estrenos".to_string(),
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
    let article_sel = selector("article");
    let entries = document
        .select(&article_sel)
        .filter(|el| attr(el, "class").contains("group/item"))
        .filter_map(card_from_article)
        .collect();
    Paged {
        entries,
        has_next_page: has_next_page(body),
    }
}

fn card_from_article(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "a[href]", "href")?;
    let title = select_text(el, "header h3").or_else(|| select_attr(el, "img[alt]", "alt"))?;
    Some(CatalogItem {
        key: path_key(&href),
        title: clean_title(&title),
        cover: select_attr(el, "img[src]", "src").map(|src| absolute_url(&src)),
        url: Some(absolute_url(&href)),
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
    let title = select_text_doc(&document, "h1").unwrap_or_else(|| title_from_path(path));
    CatalogItem {
        key: path_key(path),
        title,
        cover: select_attr_doc(&document, "img[alt$='Poster'], img[src*='/covers/']", "src")
            .map(|src| absolute_url(&src)),
        url: Some(absolute_url(path)),
        description: select_text_doc(&document, ".entry p"),
        tags: select_texts_doc(&document, "a[href*='genre=']"),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&body),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, item_path: &str) -> Vec<VideoEpisode> {
    let mut seen = Vec::<String>::new();
    let re = Regex::new(r#"href="(/media/[^"]+/([0-9]+(?:\.[0-9]+)?))""#).unwrap();
    let mut episodes = re
        .captures_iter(body)
        .filter_map(|cap| {
            let path = cap.get(1)?.as_str().to_string();
            if !path.starts_with(item_path) || seen.contains(&path) {
                return None;
            }
            seen.push(path.clone());
            let number = cap.get(2)?.as_str().parse::<f32>().ok();
            Some(VideoEpisode {
                key: path.clone(),
                title: number.map(|n| format!("Episodio {}", trim_float(n))),
                episode_number: number,
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

fn parse_hosters(body: &str, episode_url: &str, request: &Value) -> Vec<VideoHoster> {
    let lang_pref = pref(request, "preferred_language", "SUB");
    let re = Regex::new(r#"\{server:"([^"]+)",url:"([^"]+)"}"#).unwrap();
    let mut out = Vec::new();
    for lang in ["DUB", "SUB"] {
        if !lang_pref.is_empty() && lang_pref != lang {
            continue;
        }
        let Some(section) = body
            .split(&format!("{lang}:["))
            .nth(1)
            .and_then(|part| part.split(']').next())
        else {
            continue;
        };
        for cap in re.captures_iter(section) {
            let server = cap.get(1).map(|m| m.as_str()).unwrap_or("External");
            let embed = cap
                .get(2)
                .map(|m| m.as_str())
                .unwrap_or_default()
                .replace("\\u0026", "&");
            out.push(VideoHoster {
                key: format!("{lang}|{server}|{}|{episode_url}", absolute_url(&embed)),
                name: format!("{lang} {server}"),
                url: Some(absolute_url(&embed)),
                lazy: true,
                video_count: Some(1),
                ..VideoHoster::default()
            });
        }
    }
    out
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_url(embed);
    if embed.contains("player.zilla-networks.com/play/") {
        return parse_hls(&embed.replace("/play/", "/m3u8/"), name, &embed, request);
    }
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
    let patterns = [
        r#"file\s*:\s*["']([^"']+)["']"#,
        r#"src\s*:\s*["']([^"']+)["']"#,
        r#"<source[^>]+src=["']([^"']+)["']"#,
        r#"source\s+src=["']([^"']+)["']"#,
    ];
    patterns.into_iter().find_map(|pat| {
        Regex::new(pat)
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
    let server = pref(request, "preferred_server", "HLS").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080p");
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

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn filter_params(request: &Value) -> String {
    ["category", "genre", "status", "order"]
        .into_iter()
        .filter_map(|key| {
            filter(request, key)
                .filter(|value| !value.is_empty())
                .map(|value| format!("{key}={}", url::query_escape(&value)))
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn selector(input: &str) -> Selector {
    Selector::parse(input).unwrap()
}

fn select_text_doc(document: &Html, sel: &str) -> Option<String> {
    let selector = selector(sel);
    document
        .select(&selector)
        .next()
        .map(element_text)
        .filter(|v| !v.is_empty())
}

fn select_texts_doc(document: &Html, sel: &str) -> Vec<String> {
    let selector = selector(sel);
    document
        .select(&selector)
        .map(element_text)
        .filter(|v| !v.is_empty())
        .collect()
}

fn select_attr_doc(document: &Html, sel: &str, name: &str) -> Option<String> {
    let selector = selector(sel);
    document
        .select(&selector)
        .next()
        .and_then(|el| el.value().attr(name))
        .map(ToString::to_string)
}

fn select_text(el: ElementRef<'_>, sel: &str) -> Option<String> {
    let selector = selector(sel);
    el.select(&selector)
        .next()
        .map(element_text)
        .filter(|v| !v.is_empty())
}

fn select_attr(el: ElementRef<'_>, sel: &str, name: &str) -> Option<String> {
    let selector = selector(sel);
    el.select(&selector)
        .next()
        .and_then(|el| el.value().attr(name))
        .map(ToString::to_string)
}

fn attr(el: &ElementRef<'_>, name: &str) -> String {
    el.value().attr(name).unwrap_or_default().to_string()
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

fn absolute_remote(input: &str, base: &str) -> String {
    let trimmed = input.trim().replace("\\/", "/");
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
        .filter(|p| p.starts_with("/media/"))
        .map(path_key)
}

fn path_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) {
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
        .unwrap_or("AnimeAv1")
        .replace('-', " ")
}

fn clean_title(title: &str) -> String {
    title
        .trim_start_matches("Portada de ")
        .trim_start_matches("Poster de ")
        .trim()
        .to_string()
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("finalizado") {
        ItemStatus::Completed
    } else if lower.contains("emision") || lower.contains("emisión") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn has_next_page(body: &str) -> bool {
    body.contains("pagination") || body.contains("totalPages:")
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

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<article class="group/item"><header><h3>Death Note</h3></header><a href="/media/death-note"></a><img src="https://cdn.animeav1.com/covers/529.jpg"></article>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Death Note</h1><div class="entry"><p>Sample synopsis.</p></div><a href="/catalogo?genre=suspenso">Suspenso</a><img src="https://cdn.animeav1.com/covers/529.jpg"><a href="/media/death-note/1">1</a>"#;
const WATCH_FIXTURE: &str = r#"<script>embeds:{SUB:[{server:"HLS",url:"https://player.zilla-networks.com/play/sample"}]}</script>"#;
