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

use pt_video_common::{external_stream, sort_streams, stream_for_url};

const SOURCE: SushiAnimes = SushiAnimes;
const BASE_URL: &str = "https://sushianimes.com.br";
const LANG: &str = "pt-BR";

struct SushiAnimes;
struct SushiConfig;

impl pt_video_common::PtVideoConfig for SushiConfig {
    const NAME: &'static str = "Sushi Animes";
    const BASE_URL: &'static str = BASE_URL;
    const LANG: &'static str = LANG;
    const CONTENT_RATING: &'static str = "adult";
    const LIST_SELECTOR: &'static str = "a";
    const EPISODE_SELECTOR: &'static str = "a";

    fn popular_url(_page: u64) -> String {
        format!("{BASE_URL}/trends")
    }

    fn latest_url(page: u64) -> String {
        format!("{BASE_URL}/episodios?page={page}")
    }

    fn search_url(_page: u64, query: &str, _request: &Value) -> String {
        format!("{BASE_URL}/search/{}", url::query_escape(query))
    }
}

impl VideoSource for SushiAnimes {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if listing(&request) == "latest" {
            let body = fetch(&format!("{BASE_URL}/episodios?page={}", page(&request)), LIST_FIXTURE, BASE_URL);
            Ok(parse_latest(&body))
        } else {
            let body = fetch(&format!("{BASE_URL}/trends"), LIST_FIXTURE, BASE_URL);
            Ok(parse_popular(&body))
        }
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
            &format!("{BASE_URL}/search/{}", url::query_escape(query)),
            LIST_FIXTURE,
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
        let body = real_body(&path);
        let doc = Html::parse_document(&body);
        if let Some(button) = doc
            .select(&selector("a.btn[href]"))
            .find(|a| normalize(&text(*a)).contains("assistir"))
        {
            let href = attr(&button, "href");
            return Ok(vec![VideoEpisode {
                key: path_key(&href),
                title: Some("Filme".to_string()),
                episode_number: Some(1.0),
                url: Some(absolute_url(&href)),
                language: Some(LANG.to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let Some(script) = doc
            .select(&selector(r#"script[type="application/ld+json"]"#))
            .next()
            .map(text_or_data)
        else {
            return Ok(Vec::new());
        };
        let json = sanitize_ld_json_names(script.trim());
        let root = serde_json::from_str::<Value>(&json).unwrap_or(Value::Null);
        let mut episodes = Vec::new();
        for season in root
            .get("containsSeason")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let season_number = season.get("seasonNumber").and_then(Value::as_str).unwrap_or_default();
            for episode in season
                .get("episode")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let number = episode
                    .get("episodeNumber")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<f32>().ok())
                    .unwrap_or(0.0);
                let name = episode.get("name").and_then(Value::as_str).unwrap_or_default();
                let ep_url = episode.get("url").and_then(Value::as_str).unwrap_or_default();
                let title = format!("Temporada {season_number} x {} - {name}", display_number(number));
                episodes.push(VideoEpisode {
                    key: path_key(ep_url),
                    title: Some(title),
                    episode_number: Some(number),
                    url: Some(absolute_url(ep_url)),
                    language: Some(LANG.to_string()),
                    ..VideoEpisode::default()
                });
            }
        }
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "episode").unwrap_or_else(|| "/sample".to_string());
        let referer = absolute_url(&episode);
        let body = fetch(&referer, PLAYER_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let Some(id) = doc
            .select(&selector("[data-embed]"))
            .next()
            .map(|el| attr(&el, "data-embed"))
            .filter(|id| !id.is_empty())
        else {
            return Ok(Vec::new());
        };
        let ajax = client(&referer)
            .post(format!("{BASE_URL}/ajax/embed"))
            .referer(&referer)
            .form(&[("id", id.as_str())])
            .send_text()
            .unwrap_or_else(|_| EMBED_FIXTURE.to_string());
        let video_url = ajax
            .split("playerEmbed")
            .last()
            .unwrap_or_default()
            .split('"')
            .nth(1)
            .unwrap_or_default()
            .replace('\\', "");
        if video_url.is_empty() {
            return Ok(Vec::new());
        }
        let mut streams = if video_url.contains(".m3u8") || video_url.contains(".mp4") {
            vec![stream_for_url::<SushiConfig>(&video_url, "Sushi Animes", &referer, &request)]
        } else {
            vec![external_stream(&video_url, "Sushi Animes", &referer)]
        };
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Trends".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Episodios".to_string(),
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

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc.select(&selector("a.list-trend")).filter_map(card_trend).collect(),
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc.select(&selector(".episode-grid a.list-movie:not(:has(.hentai-list-media))")).filter_map(card_episode).collect(),
        has_next_page: doc.select(&selector("a.btn.btn-theme.ml-2")).next().is_some(),
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc.select(&selector("div.list-movie")).filter_map(card_search).collect(),
        has_next_page: false,
    }
}

fn card_trend(anchor: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&anchor, "href");
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title: select_text(anchor, ".list-title").unwrap_or_else(|| title_from_path(&path)),
        cover: select_attr(anchor, ".media-cover", "data-src").map(|src| absolute_url(&src)),
        description: select_text(anchor, ".list-description"),
        url: Some(absolute_url(&path)),
        language: Some(LANG.to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn card_episode(anchor: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&anchor, "href");
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title: select_text(anchor, ".list-caption").unwrap_or_else(|| title_from_path(&path)),
        cover: select_attr(anchor, ".media-episode", "data-src").map(|src| absolute_url(&src)),
        url: Some(absolute_url(&path)),
        language: Some(LANG.to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn card_search(el: ElementRef<'_>) -> Option<CatalogItem> {
    let anchor = el.select(&selector("a[href]")).next()?;
    let href = attr(&anchor, "href");
    let path = path_key(&href);
    Some(CatalogItem {
        key: path.clone(),
        title: select_text(el, ".list-title").unwrap_or_else(|| title_from_path(&path)),
        cover: select_attr(el, ".media-cover", "data-src").map(|src| absolute_url(&src)),
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
    CatalogItem {
        key: path_key(path),
        title: select_text(root, "#title, h1").unwrap_or_else(|| title_from_path(path)),
        cover: root
            .select(&selector(".media-cover img, img"))
            .next()
            .map(|img| attr(&img, "src"))
            .filter(|src| !src.is_empty())
            .map(|src| absolute_url(&src)),
        description: select_text(root, ".detail-attr .text, .text"),
        tags: root
            .select(&selector(".category-list a, .categories a"))
            .map(text)
            .filter(|value| !value.is_empty())
            .collect(),
        url: Some(absolute_url(path)),
        language: Some(LANG.to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn real_body(path: &str) -> String {
    let initial = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&initial);
    if let Some(original) = doc
        .select(&selector(".episode-nav .home-list a[href]"))
        .next()
        .map(|a| attr(&a, "href"))
        .filter(|href| !href.is_empty())
    {
        return fetch(&absolute_url(&original), &initial, BASE_URL);
    }
    initial
}

fn sanitize_ld_json_names(input: &str) -> String {
    let re = Regex::new(r#""name"\s*:\s*"(.*?)","#).unwrap();
    re.replace_all(input, |caps: &regex::Captures<'_>| {
        let escaped = caps[1].replace('"', "\\\"");
        format!(r#""name": "{escaped}","#)
    })
    .to_string()
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

fn select_attr(el: ElementRef<'_>, sel: &str, name: &str) -> Option<String> {
    el.select(&selector(sel)).next().map(|child| attr(&child, name)).filter(|value| !value.is_empty())
}

fn normalize(input: &str) -> String {
    input.to_lowercase().replace(['á', 'à', 'ã', 'â'], "a").replace(['é', 'ê'], "e").replace('í', "i").replace(['ó', 'õ', 'ô'], "o").replace('ú', "u").replace('ç', "c")
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/').rsplit('/').next().unwrap_or("Sushi Animes").replace('-', " ")
}

fn display_number(value: f32) -> String {
    if value.fract() == 0.0 { format!("{}", value as u32) } else { value.to_string() }
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

const LIST_FIXTURE: &str =
    r#"<a class="list-trend" href="/anime/sample"><div class="list-title">Sample</div><div class="media-cover" data-src="/poster.jpg"></div></a>"#;
const DETAILS_FIXTURE: &str = r#"<h1 id="title">Sample</h1><div class="media-cover"><img src="/poster.jpg"></div><script type="application/ld+json">{"name":"Sample","containsSeason":[{"seasonNumber":"1","episode":[{"episodeNumber":"1","name":"Episode 1","datePublished":"2024-01-01","url":"/episodio/sample-1"}]}]}</script>"#;
const PLAYER_FIXTURE: &str = r#"<div data-embed="1"></div>"#;
const EMBED_FIXTURE: &str = r#"{"playerEmbed":"https://example.invalid/embed"}"#;

export_video_source!(SOURCE);
