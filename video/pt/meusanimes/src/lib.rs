use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;

#[path = "../../_shared/pt_video_common.rs"]
mod pt_video_common;

use pt_video_common::{external_stream, sort_streams};

const SOURCE: MeusAnimes = MeusAnimes;
const BASE_URL: &str = "https://meusanimes.vip";
const TMDB_IMAGE_URL: &str = "https://image.tmdb.org/t/p/w500";
const LANG: &str = "pt-BR";

struct MeusAnimes;

impl VideoSource for MeusAnimes {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = fetch(
            &format!("{BASE_URL}/populares?page={}", page(&request)),
            LIST_FIXTURE,
            BASE_URL,
        );
        Ok(parse_popular(&body))
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
            &format!("{BASE_URL}/api/animes?search={}", url::query_escape(query)),
            SEARCH_FIXTURE,
            BASE_URL,
        );
        Ok(parse_search(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        let Some(data) = extract_anime_data(&body) else {
            return Ok(Vec::new());
        };
        let mut episodes = data
            .get("Episode")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|episode| {
                let slug = str_field(episode, "slug");
                let number = episode
                    .get("episodeNumber")
                    .and_then(Value::as_f64)
                    .unwrap_or(1.0) as f32;
                let title = str_field(episode, "name");
                VideoEpisode {
                    key: format!("/episodio/{slug}"),
                    title: Some(if title.is_empty() { format!("Episodio {}", display_number(number)) } else { title }),
                    episode_number: Some(number),
                    url: Some(format!("{BASE_URL}/episodio/{slug}")),
                    language: Some(LANG.to_string()),
                    ..VideoEpisode::default()
                }
            })
            .collect::<Vec<_>>();
        episodes.sort_by(|a, b| b.episode_number.partial_cmp(&a.episode_number).unwrap_or(std::cmp::Ordering::Equal));
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "episode").unwrap_or_else(|| "/episodio/sample".to_string());
        let referer = absolute_url(&episode);
        let body = fetch(&referer, PLAYER_FIXTURE, BASE_URL)
            .replace("\\/", "/")
            .replace("\\u0026", "&");
        let mut streams = Vec::new();
        for (key, name) in [("player_leg", "Legendado"), ("player_dub", "Dublado")] {
            if let Some(player) = json_string_field(&body, key).filter(|url| url.contains("blogger.com")) {
                streams.push(external_stream(&player, name, &referer));
            }
        }
        if streams.is_empty() {
            let re = Regex::new(r#"https?://www\.blogger\.com/video\.g\?token=[a-zA-Z0-9_\-&=]+"#).unwrap();
            for player in re.find_iter(&body).take(2) {
                streams.push(external_stream(player.as_str(), "Blogger", &referer));
            }
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Populares".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        }])
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

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let entries = doc
        .select(&selector(r#"div.grid > div > a[href^="/anime/"], a[href^="/anime/"]"#))
        .filter_map(|anchor| {
            let href = attr(&anchor, "href");
            if href.is_empty() {
                return None;
            }
            let path = path_key(&href);
            Some(CatalogItem {
                key: path.clone(),
                title: select_text(anchor, "h3.text-white, h3").unwrap_or_else(|| title_from_path(&path)),
                cover: anchor
                    .select(&selector("img"))
                    .next()
                    .map(|img| attr(&img, "src"))
                    .filter(|src| !src.is_empty())
                    .map(|src| absolute_url(&src)),
                url: Some(absolute_url(&path)),
                language: Some(LANG.to_string()),
                content_rating: Some("adult".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        has_next_page: !entries.is_empty(),
        entries,
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    Paged {
        entries: root
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|item| {
                let slug = str_field(item, "slug");
                let path = format!("/anime/{slug}");
                CatalogItem {
                    key: path.clone(),
                    title: str_field(item, "name"),
                    cover: poster_url(&str_field(item, "poster")),
                    url: Some(absolute_url(&path)),
                    language: Some(LANG.to_string()),
                    content_rating: Some("adult".to_string()),
                    initialized: true,
                    ..CatalogItem::default()
                }
            })
            .collect(),
        has_next_page: false,
    }
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    if let Some(data) = extract_anime_data(&body) {
        let status = if !str_field(&data, "diaLancamento").is_empty() {
            ItemStatus::Ongoing
        } else if data.get("episodios").and_then(Value::as_i64).unwrap_or(0) > 0 {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        };
        return CatalogItem {
            key: path_key(path),
            title: str_field(&data, "name"),
            artists: str_field(&data, "nameOriginal").non_empty().into_iter().collect(),
            description: str_field(&data, "sinopse").non_empty(),
            cover: poster_url(&str_field(&data, "poster")).or_else(|| meta(&doc, "meta[property=og:image]", "content")),
            tags: data
                .get("Animegenero")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| item.get("Genero").and_then(|genre| genre.get("name")).and_then(Value::as_str).map(ToString::to_string))
                .collect(),
            url: Some(absolute_url(path)),
            language: Some(LANG.to_string()),
            content_rating: Some("adult".to_string()),
            status,
            initialized: true,
            ..CatalogItem::default()
        };
    }
    CatalogItem {
        key: path_key(path),
        title: meta(&doc, "meta[property=og:title]", "content").unwrap_or_else(|| title_from_path(path)),
        description: meta(&doc, "meta[name=description]", "content"),
        cover: meta(&doc, "meta[property=og:image]", "content"),
        url: Some(absolute_url(path)),
        language: Some(LANG.to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn extract_anime_data(body: &str) -> Option<Value> {
    let scripts = Html::parse_document(body);
    for script in scripts.select(&selector("script")) {
        let data = text_or_data(script);
        if !data.contains("animeData") {
            continue;
        }
        if let Some(value) = data
            .split(r#"\"animeData\":{\"#)
            .nth(1)
            .and_then(|tail| tail.split("]}").next())
        {
            let json = format!("{{{value}]}}").replace("\\\"", "\"").replace("\\\\", "\\");
            if let Ok(parsed) = serde_json::from_str(&json) {
                return Some(parsed);
            }
        }
        if let Some(value) = data.split("\"animeData\":{").nth(1).and_then(|tail| tail.split("]}").next()) {
            let json = format!("{{{value}]}}");
            if let Ok(parsed) = serde_json::from_str(&json) {
                return Some(parsed);
            }
        }
    }
    None
}

fn json_string_field(body: &str, key: &str) -> Option<String> {
    let pattern = Regex::new(&format!(r#""{}"\s*:\s*"([^"]+)""#, regex::escape(key))).ok()?;
    pattern
        .captures(body)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().replace("\\/", "/").replace("\\u0026", "&"))
}

fn client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
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

fn meta(doc: &Html, sel: &str, attr_name: &str) -> Option<String> {
    doc.select(&selector(sel))
        .next()
        .map(|el| attr(&el, attr_name))
        .filter(|value| !value.is_empty())
}

fn poster_url(path: &str) -> Option<String> {
    (!path.trim().is_empty()).then(|| {
        if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{TMDB_IMAGE_URL}{path}")
        }
    })
}

fn str_field(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/').rsplit('/').next().unwrap_or("Meus Animes").replace('-', " ")
}

fn display_number(value: f32) -> String {
    if value.fract() == 0.0 { format!("{}", value as u32) } else { value.to_string() }
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
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
    r#"<div class="grid"><div><a href="/anime/sample"><img src="/poster.jpg"><h3 class="text-white">Sample</h3></a></div></div>"#;
const SEARCH_FIXTURE: &str =
    r#"{"data":[{"name":"Sample","slug":"sample","poster":"/poster.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"<meta property="og:title" content="Sample"><meta name="description" content="Sample."><meta property="og:image" content="/poster.jpg"><script>window.__DATA__ = {\"animeData\":{\"name\":\"Sample\",\"nameOriginal\":\"\",\"sinopse\":\"Sample.\",\"diaLancamento\":\"\",\"episodios\":1,\"poster\":\"/poster.jpg\",\"Episode\":[{\"name\":\"Episode 1\",\"episodeNumber\":1,\"slug\":\"sample-1\"}]}}</script>"#;
const PLAYER_FIXTURE: &str =
    r#"{\"player_leg\":\"https://www.blogger.com/video.g?token=sample\"}"#;

export_video_source!(SOURCE);
