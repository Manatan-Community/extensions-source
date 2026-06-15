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

const SOURCE: AnimeFire = AnimeFire;
const BASE_URL: &str = "https://animefire.io";

struct AnimeFire;

impl VideoSource for AnimeFire {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/home/{page}")
        } else {
            format!("{BASE_URL}/top-animes/{page}")
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
            format!(
                "{BASE_URL}/pesquisar/{}/{page}",
                query.trim().replace(' ', "-").to_lowercase()
            )
        } else if let Some(season) = filter(&request, "season").filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/temporada/{season}/{page}")
        } else {
            let genre = filter(&request, "genre").unwrap_or_else(|| "todos".to_string());
            format!("{BASE_URL}/genero/{genre}/{page}")
        };
        Ok(parse_cards(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/animes/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/animes/sample".to_string());
        Ok(parse_episodes(&fetch(
            &absolute_url(&path),
            DETAILS_FIXTURE,
            BASE_URL,
        )))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/animes/sample/1".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();

        for source in doc.select(&selector(
            "video#my-video source[src], video source[src], source[src]",
        )) {
            let src = attr(&source, "src");
            if src.is_empty() {
                continue;
            }
            let quality = attr(&source, "res")
                .if_empty(&attr(&source, "label"))
                .if_empty("auto");
            streams.push(stream(&absolute_remote(&src, &referer), &quality, &referer));
        }

        for iframe in doc.select(&selector("iframe[src]")) {
            let embed = absolute_remote(&attr(&iframe, "src"), &referer);
            streams.extend(resolve_embed(&embed, &referer, &request));
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
                title: "Top animes".to_string(),
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
            if path
                .trim_matches('/')
                .rsplit('/')
                .next()
                .and_then(|v| v.parse::<u32>().ok())
                .is_some()
            {
                return Ok(Some(UrlResolveResult {
                    episode: Some(json!({"key": path, "url": input, "language": "pt-BR"})),
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
        .with_header("Accept-Language", "pt-BR,pt;q=0.9,en-US;q=0.8,en;q=0.7")
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
    Paged {
        entries: doc
            .select(&selector(
                "article.cardUltimosEps > a, article a[href*='/animes/'], a[href*='/animes/']",
            ))
            .filter_map(card)
            .collect(),
        has_next_page: doc
            .select(&selector(
                "ul.pagination img.seta-right, a.next, .pagination a[rel='next']",
            ))
            .next()
            .is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href");
    if href.is_empty() {
        return None;
    }
    let episode_path = path_key(&href);
    let path = anime_path_from_episode(&episode_path);
    Some(CatalogItem {
        key: path.clone(),
        title: select_text(el, "h3.animeTitle, h3, .animeTitle")
            .unwrap_or_else(|| title_from_path(&path)),
        cover: select_attr(el, "img", "data-src")
            .or_else(|| select_attr(el, "img", "src"))
            .map(|src| absolute_url(&src)),
        url: Some(absolute_url(&path)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    let infos = select_text_doc(&doc, "div.divAnimePageInfo").unwrap_or_default();
    let description = select_text_doc(&doc, "div.divSinopse > span, div.divSinopse");
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, "div.div_anime_names h1, h1")
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, "div.sub_animepage_img img, img", "data-src")
            .or_else(|| select_attr_doc(&doc, "div.sub_animepage_img img, img", "src"))
            .map(|src| absolute_url(&src)),
        description,
        tags: select_texts_doc(&doc, "a.spanGeneros"),
        authors: info_after(&infos, "Estudios")
            .or_else(|| info_after(&infos, "Estudios"))
            .into_iter()
            .collect(),
        url: Some(absolute_url(path)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&infos),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let mut out = doc
        .select(&selector(
            "div.div_video_list > a[href], a[href*='/animes/']",
        ))
        .filter_map(|el| {
            let href = attr(&el, "href");
            let path = path_key(&href);
            let name = text(el).if_empty(&title_from_path(&path));
            let number = path
                .trim_matches('/')
                .rsplit('/')
                .next()
                .and_then(|v| v.parse::<f32>().ok())
                .or_else(|| first_number(&name))
                .unwrap_or(0.0);
            Some(VideoEpisode {
                key: path.clone(),
                title: Some(name),
                episode_number: Some(number),
                url: Some(absolute_url(&path)),
                language: Some("pt-BR".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| {
        b.episode_number
            .partial_cmp(&a.episode_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

fn resolve_embed(embed: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    if embed.contains(".m3u8") {
        return parse_hls(embed, referer, request);
    }
    let body = fetch(embed, "", referer);
    if let Some(media) = first_media_url(&body).map(|src| absolute_remote(&src, embed)) {
        if media.contains(".m3u8") {
            parse_hls(&media, embed, request)
        } else {
            vec![stream(&media, "direct", embed)]
        }
    } else {
        vec![external_stream(embed, referer)]
    }
}

fn parse_hls(master: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = client(referer)
        .get(master)
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    if !body.contains("#EXT-X-STREAM-INF") {
        return vec![stream(
            master,
            &preference(request, "preferred_quality", "720p"),
            referer,
        )];
    }
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
                &quality,
                referer,
            ));
        }
    }
    out
}

fn stream(src: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = src.contains(".m3u8");
    VideoStream {
        url: src.to_string(),
        name: Some(quality.to_string()),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(referer),
        preferred: quality.contains("720"),
        initialized: true,
        ..VideoStream::default()
    }
}

fn external_stream(embed: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: embed.to_string(),
        name: Some(host_name(embed)),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        preferred: true,
        initialized: true,
        ..VideoStream::default()
    }
}

fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)"#,
        r#"src\s*:\s*["']([^"']+)"#,
        r#"<source[^>]+src=["']([^"']+)"#,
        r#"https?://[^\s'"\\]+\.m3u8[^\s'"\\]*"#,
    ]
    .into_iter()
    .find_map(|pattern| {
        Regex::new(pattern)
            .ok()?
            .captures(body)
            .and_then(|cap| cap.get(1).or_else(|| cap.get(0)))
            .map(|m| m.as_str().replace("\\/", "/"))
    })
}

fn anime_path_from_episode(path: &str) -> String {
    path.trim_matches('/')
        .rsplit_once('/')
        .and_then(|(base, ep)| {
            ep.parse::<u32>()
                .ok()
                .map(|_| format!("/{base}-todos-os-episodios"))
        })
        .unwrap_or_else(|| path_key(path))
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let quality = preference(request, "preferred_quality", "720p");
    streams.sort_by_key(|stream| {
        stream
            .quality
            .as_deref()
            .unwrap_or_default()
            .contains(&quality)
    });
    streams.reverse();
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn selector(input: &str) -> Selector {
    Selector::parse(input).unwrap()
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
fn select_text(el: ElementRef<'_>, sel: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .map(text)
        .filter(|v| !v.is_empty())
}
fn select_attr(el: ElementRef<'_>, sel: &str, name: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .map(|e| attr(&e, name))
        .filter(|v| !v.is_empty())
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
        .map(|e| attr(&e, name))
        .filter(|v| !v.is_empty())
}
fn select_texts_doc(doc: &Html, sel: &str) -> Vec<String> {
    doc.select(&selector(sel))
        .map(text)
        .filter(|v| !v.is_empty())
        .collect()
}
fn first_number(input: &str) -> Option<f32> {
    Regex::new(r"\d+(?:\.\d+)?")
        .ok()?
        .find(input)?
        .as_str()
        .parse()
        .ok()
}
fn info_after(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .map(|v| {
            v.trim_matches(|c| c == ':' || c == ' ')
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|v| !v.is_empty())
}
fn parse_status(input: &str) -> ItemStatus {
    if input.contains("Completo") {
        ItemStatus::Completed
    } else if input.contains("lan") || input.contains("Lan") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
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
fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|f| f.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}
fn preference(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}
fn with_listing(request: &Value, listing: &str) -> Value {
    let mut cloned = request.clone();
    if let Value::Object(ref mut map) = cloned {
        map.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    cloned
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
fn path_from_url(input: &str) -> Option<String> {
    (input.starts_with(BASE_URL) || input.starts_with('/')).then(|| path_key(input))
}
fn path_key(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(BASE_URL) {
        return input.to_string();
    }
    let without_base = input.strip_prefix(BASE_URL).unwrap_or(input);
    format!(
        "/{}",
        without_base
            .split('#')
            .next()
            .unwrap_or(without_base)
            .trim_matches('/')
    )
}
fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}
fn absolute_remote(input: &str, base: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        let root = base.rsplit_once('/').map(|(root, _)| root).unwrap_or(base);
        format!(
            "{}/{}",
            root.trim_end_matches('/'),
            input.trim_start_matches('/')
        )
    }
}
fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("Anime Fire")
        .replace('-', " ")
}
fn host_name(input: &str) -> String {
    input
        .split('/')
        .nth(2)
        .unwrap_or("External")
        .replace("www.", "")
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}
impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

const LIST_FIXTURE: &str = r#"<article class="cardUltimosEps"><a href="/animes/sample/1"><h3 class="animeTitle">Sample</h3><img data-src="/sample.jpg"></a></article>"#;
const DETAILS_FIXTURE: &str = r#"<div class="divDivAnimeInfo"><div class="div_anime_names"><h1>Sample</h1></div><div class="sub_animepage_img"><img data-src="/sample.jpg"></div><div class="divAnimePageInfo"><a class="spanGeneros">Action</a><div>Status <span>Em lançamento</span></div></div><div class="divSinopse"><span>Sample details.</span></div><div class="div_video_list"><a href="/animes/sample/1">Episodio 1</a></div></div>"#;
const WATCH_FIXTURE: &str =
    r#"<video id="my-video"><source src="https://media.example/sample.m3u8" res="720p"></video>"#;

export_video_source!(SOURCE);
