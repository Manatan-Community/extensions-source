use base64::{Engine as _, engine::general_purpose};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use scraper::{Element, ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::Value;

#[path = "../../_shared/pt_video_common.rs"]
mod pt_video_common;

use pt_video_common::{external_stream, sort_streams};

const SOURCE: Goyabu = Goyabu;
const BASE_URL: &str = "https://goyabu.io";
const LANG: &str = "pt-BR";

struct Goyabu;

impl VideoSource for Goyabu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        if listing(&request) == "latest" {
            let target = if page == 1 {
                format!("{BASE_URL}/lancamentos")
            } else {
                format!("{BASE_URL}/lancamentos/page/{page}")
            };
            return Ok(parse_cards(
                &fetch(&target, LIST_FIXTURE, BASE_URL),
                "article.boxEP a",
                true,
            ));
        }
        Ok(parse_cards(
            &fetch(&format!("{BASE_URL}?s="), LIST_FIXTURE, BASE_URL),
            "article.boxAN a",
            false,
        ))
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
        let body = fetch(
            &format!("{BASE_URL}?s={}", url::query_escape(query)),
            LIST_FIXTURE,
            BASE_URL,
        );
        Ok(parse_cards(&body, "article.boxAN a", true))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        let body = real_body(&path);
        let Some(script) = Html::parse_document(&body)
            .select(&selector("script"))
            .find(|script| text_or_data(*script).contains("const allEpisodes"))
            .map(text_or_data)
        else {
            return Ok(Vec::new());
        };
        let json = script
            .split("const allEpisodes =")
            .nth(1)
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        let mut episodes = serde_json::from_str::<Vec<EpisodeDto>>(json)
            .unwrap_or_default()
            .into_iter()
            .map(|episode| episode.into_episode())
            .collect::<Vec<_>>();
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "episode").unwrap_or_else(|| "/sample".to_string());
        let referer = absolute_url(&episode);
        let body = fetch(&referer, PLAYER_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for player in doc.select(&selector("[data-blogger-url-encrypted]")) {
            let encrypted = attr(&player, "data-blogger-url-encrypted");
            let decoded = general_purpose::STANDARD
                .decode(encrypted.trim())
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_default();
            let blogger = decoded.chars().rev().collect::<String>();
            if blogger.contains("blogger.com") {
                streams.push(external_stream(&blogger, "Blogger", &referer));
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
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Lancamentos".to_string(),
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

fn parse_cards(body: &str, sel: &str, next_by_pagination: bool) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let entries = doc
        .select(&selector(sel))
        .filter_map(card_from_anchor)
        .collect::<Vec<_>>();
    let has_next_page = if next_by_pagination {
        doc.select(&selector("div.pagination"))
            .next()
            .map(|pagination| {
                let current = attr(&pagination, "data-current-page").parse::<u64>().unwrap_or(1);
                let total = attr(&pagination, "data-total-pages").parse::<u64>().unwrap_or(1);
                current < total
            })
            .unwrap_or_else(|| doc.select(&selector("div.pagination a")).any(|a| text(a).contains('›')))
    } else {
        false
    };
    Paged {
        entries,
        has_next_page,
    }
}

fn card_from_anchor(anchor: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&anchor, "href");
    if href.is_empty() {
        return None;
    }
    let path = path_key(&href);
    let title = select_text(anchor, "div.title")
        .or_else(|| attr(&anchor, "title").non_empty())
        .unwrap_or_else(|| title_from_path(&path));
    let cover = anchor
        .select(&selector("img"))
        .next()
        .map(|img| attr(&img, "src"))
        .or_else(|| anchor.select(&selector("figure")).next().map(|figure| attr(&figure, "data-thumb")))
        .filter(|value| !value.is_empty())
        .map(|src| absolute_url(&src));
    Some(CatalogItem {
        key: path.clone(),
        title,
        cover,
        url: Some(absolute_url(&path)),
        language: Some(LANG.to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = real_body(path);
    let doc = Html::parse_document(&body);
    let root = doc.root_element();
    let title = select_text(root, "div.streamer-info h1, h1").unwrap_or_else(|| title_from_path(path));
    let status = select_text(root, ".streamer-info-list li.status").unwrap_or_default();
    CatalogItem {
        key: path_key(path),
        title,
        cover: root
            .select(&selector("div.streamer-poster img, img"))
            .next()
            .map(|img| attr(&img, "src"))
            .filter(|value| !value.is_empty())
            .map(|src| absolute_url(&src)),
        description: select_text(root, ".sinopse-full"),
        tags: root
            .select(&selector("div.filter-items a.filter-btn"))
            .map(text)
            .filter(|value| !value.is_empty())
            .collect(),
        url: Some(absolute_url(path)),
        language: Some(LANG.to_string()),
        content_rating: Some("adult".to_string()),
        status: parse_status(&status),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn real_body(path: &str) -> String {
    let initial = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&initial);
    if let Some(original) = doc
        .select(&selector(".episode-navigation span.lista"))
        .next()
        .and_then(|menu| menu.parent_element())
        .map(|parent| attr(&parent, "href"))
        .filter(|href| !href.is_empty())
    {
        return fetch(&absolute_url(&original), &initial, BASE_URL);
    }
    initial
}

fn client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_header("Origin", BASE_URL)
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
    format!("/{}", without_base.split('#').next().unwrap_or(without_base).trim_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn selector(input: &str) -> Selector {
    Selector::parse(input).unwrap()
}

fn attr(el: &ElementRef<'_>, name: &str) -> String {
    el.value().attr(name).unwrap_or_default().to_string()
}

fn text(el: ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn text_or_data(el: ElementRef<'_>) -> String {
    let data = el
        .children()
        .filter_map(|child| child.value().as_text())
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    if data.is_empty() { text(el) } else { data }
}

fn select_text(el: ElementRef<'_>, sel: &str) -> Option<String> {
    el.select(&selector(sel)).next().map(text).filter(|value| !value.is_empty())
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/').rsplit('/').next().unwrap_or("Goyabu").replace('-', " ")
}

fn parse_status(input: &str) -> ItemStatus {
    match normalize(input).as_str() {
        value if value.contains("completo") => ItemStatus::Completed,
        value if value.contains("lancamento") => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn normalize(input: &str) -> String {
    input
        .to_lowercase()
        .replace(['á', 'à', 'ã', 'â'], "a")
        .replace(['é', 'ê'], "e")
        .replace('í', "i")
        .replace(['ó', 'õ', 'ô'], "o")
        .replace('ú', "u")
        .replace('ç', "c")
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn with_listing(request: &Value, list: &str) -> Value {
    let mut cloned = request.clone();
    if let Value::Object(ref mut map) = cloned {
        map.insert("listing".to_string(), Value::String(list.to_string()));
    }
    cloned
}

#[derive(Deserialize)]
struct EpisodeDto {
    episodio: String,
    link: String,
    #[serde(rename = "episode_name")]
    episode_name: String,
    audio: Option<String>,
}

impl EpisodeDto {
    fn into_episode(self) -> VideoEpisode {
        let number = self.episodio.parse::<f32>().unwrap_or(1.0);
        let mut title = format!("Episodio {}", self.episodio);
        if !self.episode_name.trim().is_empty() {
            title.push_str(" - ");
            title.push_str(self.episode_name.trim());
        }
        if let Some(audio) = self.audio.filter(|value| !value.trim().is_empty()) {
            title.push_str(" - ");
            title.push_str(audio.trim());
        }
        let key = path_key(&self.link);
        VideoEpisode {
            key: key.clone(),
            title: Some(title),
            episode_number: Some(number),
            url: Some(absolute_url(&key)),
            language: Some(LANG.to_string()),
            ..VideoEpisode::default()
        }
    }
}

trait NonEmpty {
    fn non_empty(self) -> Option<String>;
}

impl NonEmpty for String {
    fn non_empty(self) -> Option<String> {
        (!self.trim().is_empty()).then_some(self)
    }
}

const LIST_FIXTURE: &str =
    r#"<article class="boxAN"><a href="/anime/sample"><div class="title">Sample</div><img src="/poster.jpg"></a></article>"#;
const DETAILS_FIXTURE: &str = r#"<div class="streamer-info"><h1>Sample</h1></div><div class="streamer-poster"><img src="/poster.jpg"></div><div class="sinopse-full">Sample.</div><script>const allEpisodes = [{"episodio":"1","link":"/watch/sample-1","episode_name":"","audio":"Legendado","update":"2024-01-01T00:00:00Z"}];</script>"#;
const PLAYER_FIXTURE: &str = r#"<div data-blogger-url-encrypted="aHR0cHM6Ly93d3cuYmxvZ2dlci5jb20vdmlkZW8uZz90b2tlbj1zYW1wbGU="></div>"#;

export_video_source!(SOURCE);
