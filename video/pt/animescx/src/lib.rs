use base64::{Engine as _, engine::general_purpose};
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
use serde::{Deserialize, Serialize};
use serde_json::Value;

const SOURCE: AnimesCx = AnimesCx;
const BASE_URL: &str = "https://animescx.com.br";

struct AnimesCx;

impl VideoSource for AnimesCx {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let path = if listing(&request) == "latest" {
            "doramas-em-lancamento"
        } else {
            "doramas-legendados"
        };
        Ok(parse_listing(&fetch(
            &format!("{BASE_URL}/{path}/page/{page}"),
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
        let target = format!(
            "{BASE_URL}/page/{}/?s={}",
            page(&request),
            url::query_escape(query)
        );
        Ok(parse_search(&fetch(&target, SEARCH_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        Ok(fetch_all_episodes(&path))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request
            .get("episode")
            .and_then(|value| {
                value
                    .get("key")
                    .or_else(|| value.get("url"))
                    .and_then(Value::as_str)
                    .or_else(|| value.as_str())
            })
            .or_else(|| request.get("key").and_then(Value::as_str))
            .unwrap_or("[]");
        let hosts = serde_json::from_str::<Vec<QualityHosts>>(key).unwrap_or_default();
        let mut streams = Vec::new();
        for group in hosts {
            for host in group.hosts {
                streams.extend(resolve_host(&group.quality, &host, &request));
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
                title: "Doramas legendados".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Em lancamento".to_string(),
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
        let key = request
            .get("episode")
            .and_then(|value| {
                value
                    .get("url")
                    .and_then(Value::as_str)
                    .or_else(|| value.as_str())
            })
            .or_else(|| request.get("url").and_then(Value::as_str));
        Ok(key.map(ToString::to_string))
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
            .select(&selector(
                "div.listaAnimes_Riverlab_Container > a[href], a[href*='animescx.com.br']",
            ))
            .filter_map(list_card)
            .collect(),
        has_next_page: has_next_page(&doc),
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc
            .select(&selector("article.rl_episodios"))
            .filter_map(search_card)
            .collect(),
        has_next_page: doc
            .select(&selector("a.next.page-numbers"))
            .next()
            .is_some(),
    }
}

fn list_card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href");
    if href.is_empty() {
        return None;
    }
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title: select_text(
            el,
            "div.infolistaAnimes_RiverLab, .infolistaAnimes_RiverLab",
        )
        .unwrap_or_else(|| title_from_path(&path)),
        cover: select_attr(el, "img", "src").map(|src| absolute_url(&src)),
        url: Some(absolute_url(&path)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn search_card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "a[href]", "href")?;
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title: select_text(el, "a[href], h2, header").unwrap_or_else(|| title_from_path(&path)),
        cover: select_attr(el, "img", "src").map(|src| absolute_url(&src)),
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
    let infos = doc.select(&selector("div.rl_anime_metadados")).next();
    let root = infos.unwrap_or_else(|| doc.root_element());
    let status_text = info_value(root, "Status");
    CatalogItem {
        key: path_key(path),
        title: select_text(root, ".rl_nome_anime, h1").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr(root, "img", "src").map(|src| absolute_url(&src)),
        description: info_value(root, "Sinopse"),
        tags: info_value(root, "Generos")
            .map(|v| {
                v.split(';')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        url: Some(absolute_url(path)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&status_text.unwrap_or_default()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_all_episodes(path: &str) -> Vec<VideoEpisode> {
    let mut target = absolute_url(path);
    let mut out = Vec::new();
    for _ in 0..20 {
        let body = fetch(&target, DETAILS_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        out.extend(parse_episodes_page(&doc, &target));
        let Some(next) = doc
            .select(&selector("a.rl_anime_pagination"))
            .last()
            .map(|el| attr(&el, "href"))
            .filter(|href| !href.is_empty())
        else {
            break;
        };
        let next = absolute_url(&next);
        if page_number(&next) == page_number(&target) {
            break;
        }
        target = next;
    }
    out.reverse();
    out
}

fn parse_episodes_page(doc: &Html, page_url: &str) -> Vec<VideoEpisode> {
    doc.select(&selector(
        ".rl_anime_episodios > article.rl_episodios, article.rl_episodios",
    ))
    .filter_map(|el| {
        let title = select_text(el, "header, h2").unwrap_or_else(|| "Episodio".to_string());
        let number = title
            .rsplit(' ')
            .next()
            .and_then(|v| v.parse::<f32>().ok())
            .or_else(|| first_number(&title))
            .unwrap_or(0.0);
        let hosts = quality_hosts(el);
        let key = serde_json::to_string(&hosts).unwrap_or_else(|_| "[]".to_string());
        Some(VideoEpisode {
            key,
            title: Some(if number > 0.0 {
                format!("Episodio {}", display_number(number))
            } else {
                title
            }),
            episode_number: Some(number),
            url: Some(page_url.to_string()),
            language: Some("pt-BR".to_string()),
            ..VideoEpisode::default()
        })
    })
    .collect()
}

fn quality_hosts(el: ElementRef<'_>) -> Vec<QualityHosts> {
    let mut out = Vec::new();
    for quality in el.select(&selector("div.rl_episodios_opcnome[onclick]")) {
        let label = text(quality).if_empty("Video");
        let item_id = attr(&quality, "onclick")
            .split("rlToggle('")
            .nth(1)
            .and_then(|v| v.split('\'').next())
            .unwrap_or_default()
            .to_string();
        if item_id.is_empty() {
            continue;
        }
        let hosts = el
            .select(&selector(&format!("#{item_id} a.rl_episodios_link[href]")))
            .filter_map(|link| {
                let name = text(link);
                if name.eq_ignore_ascii_case("Mega") {
                    return None;
                }
                let encoded = attr(&link, "href")
                    .split("id=")
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                let url = decode_host_url(&encoded)?;
                Some(VideoHost { name, url })
            })
            .collect::<Vec<_>>();
        if !hosts.is_empty() {
            out.push(QualityHosts {
                quality: label,
                hosts,
            });
        }
    }
    out
}

fn resolve_host(quality: &str, host: &VideoHost, request: &Value) -> Vec<VideoStream> {
    if host.name.eq_ignore_ascii_case("MediaFire") {
        let body = fetch(&host.url, "", BASE_URL);
        if let Some(src) = Html::parse_document(&body)
            .select(&selector("a#downloadButton[href]"))
            .next()
            .map(|el| attr(&el, "href"))
            .filter(|href| !href.is_empty())
        {
            return vec![stream(
                &absolute_remote(&src, &host.url),
                &format!("MediaFire - {quality}"),
                quality,
                &host.url,
                false,
            )];
        }
    }
    if host.url.contains(".m3u8") {
        return vec![stream(
            &host.url,
            &format!("{} - {quality}", host.name),
            quality,
            BASE_URL,
            true,
        )];
    }
    let preferred = preference(request, "pref_quality_key", "FULL HD");
    vec![VideoStream {
        url: host.url.clone(),
        name: Some(format!("{} - {quality}", host.name)),
        quality: Some(quality.to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(BASE_URL),
        preferred: quality.contains(&preferred),
        initialized: true,
        ..VideoStream::default()
    }]
}

fn stream(src: &str, name: &str, quality: &str, referer: &str, is_hls: bool) -> VideoStream {
    VideoStream {
        url: src.to_string(),
        name: Some(name.to_string()),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(referer),
        preferred: quality.contains("FULL HD"),
        initialized: true,
        ..VideoStream::default()
    }
}

fn decode_host_url(input: &str) -> Option<String> {
    let bytes = general_purpose::STANDARD
        .decode(input)
        .or_else(|_| general_purpose::URL_SAFE.decode(input))
        .ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    Some(decoded.chars().rev().collect())
}

fn has_next_page(doc: &Html) -> bool {
    doc.select(&selector("a.rl_anime_pagination"))
        .last()
        .map(|el| attr(&el, "href"))
        .filter(|href| !href.is_empty())
        .map(|href| page_number(&href) > 1)
        .unwrap_or(false)
}

fn page_number(input: &str) -> u64 {
    input
        .split("/page/")
        .nth(1)
        .and_then(|v| v.trim_matches('/').parse::<u64>().ok())
        .unwrap_or(1)
}

fn info_value(el: ElementRef<'_>, label: &str) -> Option<String> {
    let simplified = normalize_label(label);
    for row in el.select(&selector(".rl_anime_meta")) {
        let value = text(row);
        if normalize_label(&value).contains(&simplified) {
            let cleaned = value.replace(label, "").replace(':', "").trim().to_string();
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }
    None
}

fn parse_status(input: &str) -> ItemStatus {
    match input {
        "Completo" => ItemStatus::Completed,
        "Lancando" | "Sendo Legendado!" => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let quality = preference(request, "pref_quality_key", "FULL HD");
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
fn first_number(input: &str) -> Option<f32> {
    Regex::new(r"\d+(?:\.\d+)?")
        .ok()?
        .find(input)?
        .as_str()
        .parse()
        .ok()
}
fn display_number(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as u32)
    } else {
        value.to_string()
    }
}
fn normalize_label(input: &str) -> String {
    input
        .to_lowercase()
        .replace('ê', "e")
        .replace('é', "e")
        .replace('á', "a")
        .replace('ã', "a")
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
        .unwrap_or("Animes CX")
        .replace('-', " ")
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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct QualityHosts {
    quality: String,
    hosts: Vec<VideoHost>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct VideoHost {
    name: String,
    url: String,
}

const LIST_FIXTURE: &str = r#"<div class="listaAnimes_Riverlab_Container"><a href="/sample"><img src="/sample.jpg"><div class="infolistaAnimes_RiverLab">Sample</div></a></div>"#;
const SEARCH_FIXTURE: &str = r#"<article class="rl_episodios"><a href="/sample">Sample</a><img class="rl_AnimeIndexImg" src="/sample.jpg"></article>"#;
const DETAILS_FIXTURE: &str = r#"<div class="rl_anime_metadados"><img src="/sample.jpg"><div class="rl_nome_anime">Sample</div><div class="rl_anime_meta">Generos Action; Drama</div><div class="rl_anime_meta">Status Completo</div><div class="rl_anime_meta">Sinopse Sample details.</div></div><div class="rl_anime_episodios"><article class="rl_episodios"><header>Episodio 1</header><div class="rl_episodios_opcnome" onclick="rlToggle('host1')">FULL HD</div><div id="host1"><a class="rl_episodios_link" href="?id=bW9jLmVscG1heGUvLzpzcHR0aA==">MediaFire</a></div></article></div>"#;

export_video_source!(SOURCE);
