use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::Value;

#[path = "../../_shared/pt_video_common.rs"]
mod pt_video_common;

use pt_video_common::{PtVideoConfig, absolute_remote, external_stream, sort_streams, stream_for_url};

const SOURCE: HentaisTube = HentaisTube;
const BASE_URL: &str = "https://www.hentaistube.com";
const LANG: &str = "pt-BR";

struct HentaisTube;
struct HentaisTubeConfig;

impl PtVideoConfig for HentaisTubeConfig {
    const NAME: &'static str = "HentaisTube";
    const BASE_URL: &'static str = BASE_URL;
    const LANG: &'static str = LANG;
    const CONTENT_RATING: &'static str = "adult";
    const LIST_SELECTOR: &'static str = "ul.ul_sidebar > li";
    const EPISODE_SELECTOR: &'static str = "a";

    fn popular_url(page: u64) -> String {
        format!("{BASE_URL}/ranking-hentais?paginacao={page}")
    }

    fn latest_url(page: u64) -> String {
        format!("{BASE_URL}/page/{page}/")
    }

    fn search_url(_page: u64, query: &str, _request: &Value) -> String {
        format!("{BASE_URL}/?s={}", url::query_escape(query))
    }
}

impl VideoSource for HentaisTube {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = if listing(&request) == "latest" {
            fetch(&format!("{BASE_URL}/page/{}/", page(&request)), LIST_FIXTURE, BASE_URL)
        } else {
            fetch(
                &format!("{BASE_URL}/ranking-hentais?paginacao={}", page(&request)),
                LIST_FIXTURE,
                BASE_URL,
            )
        };
        Ok(if listing(&request) == "latest" {
            parse_latest(&body)
        } else {
            parse_popular(&body)
        })
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
        let body = client(BASE_URL)
            .get(format!("{BASE_URL}/json-lista-capas.php"))
            .xhr()
            .send_text()
            .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
        let items = serde_json::from_str::<ItemsListDto>(&body).unwrap_or_default().items;
        Ok(search_items(items, query, page(&request), &request))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut episodes = doc
            .select(&selector("ul.pagAniListaContainer > li > a"))
            .filter_map(episode_from_anchor)
            .collect::<Vec<_>>();
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "episode").unwrap_or_else(|| "/sample-1".to_string());
        let referer = absolute_url(&episode);
        let body = fetch(&referer, PLAYER_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for iframe in doc.select(&selector("iframe.meu-player[src], iframe.meu-player[data-src]")) {
            let raw = attr(&iframe, "src").if_empty(&attr(&iframe, "data-src"));
            let iframe_url = absolute_remote(&raw, &referer);
            streams.extend(resolve_player_iframe(&iframe_url, &request));
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
                title: "Ranking".to_string(),
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

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc
            .select(&selector("ul.ul_sidebar > li"))
            .filter_map(|li| {
                let anchor = li.select(&selector("div.rt a.series, a.series")).next()?;
                let href = attr(&anchor, "href");
                let path = path_key(&href);
                Some(CatalogItem {
                    key: path.clone(),
                    title: text(anchor).split(" - Episodios").next().unwrap_or_default().to_string(),
                    cover: li
                        .select(&selector("img"))
                        .next()
                        .map(|img| attr(&img, "src"))
                        .filter(|src| !src.is_empty())
                        .map(|src| absolute_url(&src)),
                    url: Some(absolute_url(&path)),
                    language: Some(LANG.to_string()),
                    content_rating: Some("adult".to_string()),
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: doc.select(&selector("div.paginacao > a")).any(|a| text(a).contains('»')),
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: doc
            .select(&selector("div.epiContainer:first-child div.epiItem > a"))
            .filter_map(|anchor| {
                let href = attr(&anchor, "href");
                let series = href.rsplit_once('-').map(|(base, _)| format!("{base}s")).unwrap_or(href);
                let path = path_key(&series);
                Some(CatalogItem {
                    key: path.clone(),
                    title: attr(&anchor, "title").if_empty(&pt_video_common::title_from_path::<HentaisTubeConfig>(&path)),
                    cover: anchor
                        .select(&selector("img"))
                        .next()
                        .map(|img| attr(&img, "src"))
                        .filter(|src| !src.is_empty())
                        .map(|src| absolute_url(&src)),
                    url: Some(absolute_url(&path)),
                    language: Some(LANG.to_string()),
                    content_rating: Some("adult".to_string()),
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: doc.select(&selector("div.paginacao > a")).any(|a| text(a).contains('»')),
    }
}

fn search_items(items: Vec<SearchItemDto>, query: &str, page: u64, request: &Value) -> Paged<CatalogItem> {
    let letter = filter(request, "letter").unwrap_or_default().to_lowercase();
    let include_genres = split_filter(request, "include_genres");
    let exclude_genres = split_filter(request, "exclude_genres");
    let include_studios = split_filter(request, "include_studios");
    let exclude_studios = split_filter(request, "exclude_studios");
    let query = query.to_lowercase();
    let mut entries = items
        .into_iter()
        .filter(|item| query.is_empty() || item.title.to_lowercase().contains(&query))
        .filter(|item| letter.is_empty() || item.title.to_lowercase().starts_with(&letter))
        .filter(|item| contains_all(&item.tags, &include_genres))
        .filter(|item| contains_none(&item.tags, &exclude_genres))
        .filter(|item| contains_all(&item.studios, &include_studios))
        .filter(|item| contains_none(&item.studios, &exclude_studios))
        .collect::<Vec<_>>();
    entries.sort_by_key(|item| item.title.to_lowercase());
    let start = ((page.max(1) - 1) * 30) as usize;
    let slice = entries.iter().skip(start).take(30);
    Paged {
        entries: slice
            .map(|item| CatalogItem {
                key: path_key(&format!("/{}", item.url.trim_start_matches('/'))),
                title: item.title.split("- Episodios").next().unwrap_or(&item.title).trim().to_string(),
                cover: (!item.thumbnail.is_empty()).then(|| absolute_url(&item.thumbnail)),
                url: Some(absolute_url(&format!("/{}", item.url.trim_start_matches('/')))),
                language: Some(LANG.to_string()),
                content_rating: Some("adult".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
            .collect(),
        has_next_page: entries.len() > start + 30,
    }
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    let root = doc.root_element();
    let info = root.select(&selector("div#anime")).next().unwrap_or(root);
    CatalogItem {
        key: path_key(path),
        title: info_value(info, "Hentai:").unwrap_or_else(|| pt_video_common::title_from_path::<HentaisTubeConfig>(path)),
        cover: info
            .select(&selector("img"))
            .next()
            .map(|img| attr(&img, "src"))
            .filter(|src| !src.is_empty())
            .map(|src| absolute_url(&src)),
        description: info.select(&selector("div#sinopse2")).next().map(text),
        tags: info_value(info, "Tags")
            .map(|tags| tags.split(',').map(|tag| tag.trim().to_string()).filter(|tag| !tag.is_empty()).collect())
            .unwrap_or_default(),
        artists: info_value(info, "Estudio").into_iter().collect(),
        url: Some(absolute_url(path)),
        language: Some(LANG.to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn episode_from_anchor(anchor: ElementRef<'_>) -> Option<VideoEpisode> {
    let href = attr(&anchor, "href");
    if href.is_empty() {
        return None;
    }
    let title = text(anchor);
    let number = title.split_whitespace().find_map(|part| part.parse::<f32>().ok()).unwrap_or(1.0);
    let key = path_key(&href);
    Some(VideoEpisode {
        key: key.clone(),
        title: Some(title),
        episode_number: Some(number),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        ..VideoEpisode::default()
    })
}

fn resolve_player_iframe(url: &str, request: &Value) -> Vec<VideoStream> {
    let body = fetch(url, "", BASE_URL);
    let doc = Html::parse_document(&body);
    if url.contains("/hd.php") {
        return doc
            .select(&selector("video > source[src], source[src]"))
            .map(|source| {
                let src = absolute_remote(&attr(&source, "src"), url);
                let label = attr(&source, "label").if_empty("Principal");
                stream_for_url::<HentaisTubeConfig>(&src, &label, url, request)
            })
            .collect();
    }
    if url.contains("/index.php") {
        if let Some(blogger) = doc.select(&selector("iframe[src]")).next().map(|iframe| absolute_remote(&attr(&iframe, "src"), url)) {
            return vec![external_stream(&blogger, "Blogger", url)];
        }
    }
    if url.contains("/player.php") {
        if let Some(link) = doc.select(&selector("a[href]")).next().map(|a| absolute_remote(&attr(&a, "href"), url)) {
            let body = fetch(&link, "", url);
            let doc = Html::parse_document(&body);
            return doc
                .select(&selector("video > source[src], source[src]"))
                .map(|source| {
                    let src = absolute_remote(&attr(&source, "src"), &link);
                    stream_for_url::<HentaisTubeConfig>(&src, "Alternativo", &link, request)
                })
                .collect();
        }
    }
    Vec::new()
}

fn info_value(info: ElementRef<'_>, key: &str) -> Option<String> {
    let wanted = normalize(key);
    let mut values = Vec::new();
    for line in info.select(&selector("div.boxAnimeSobreLinha")) {
        if normalize(&text(line)).contains(&wanted) {
            values.extend(line.select(&selector("a")).map(text).filter(|value| !value.is_empty()));
        }
    }
    (!values.is_empty()).then(|| values.join(", "))
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

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn split_filter(request: &Value, key: &str) -> Vec<String> {
    filter(request, key)
        .unwrap_or_default()
        .split(',')
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

fn contains_all(haystack: &str, needles: &[String]) -> bool {
    let haystack = haystack.to_lowercase();
    needles.iter().all(|needle| haystack.contains(needle))
}

fn contains_none(haystack: &str, needles: &[String]) -> bool {
    let haystack = haystack.to_lowercase();
    needles.iter().all(|needle| !haystack.contains(needle))
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

fn with_listing(request: &Value, list: &str) -> Value {
    let mut cloned = request.clone();
    if let Value::Object(ref mut map) = cloned {
        map.insert("listing".to_string(), Value::String(list.to_string()));
    }
    cloned
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.trim().is_empty() { fallback.to_string() } else { self }
    }
}

#[derive(Default, Deserialize)]
struct ItemsListDto {
    #[serde(rename = "encontrado")]
    items: Vec<SearchItemDto>,
}

#[derive(Deserialize)]
struct SearchItemDto {
    #[serde(rename = "titulo")]
    title: String,
    #[serde(rename = "imagem")]
    thumbnail: String,
    #[serde(rename = "estudio")]
    studios: String,
    url: String,
    tags: String,
}

const LIST_FIXTURE: &str =
    r#"<ul class="ul_sidebar"><li><img src="/poster.jpg"><div class="rt"><a class="series" href="/sample">Sample - Episodios</a></div></li></ul>"#;
const SEARCH_FIXTURE: &str =
    r#"{"encontrado":[{"titulo":"Sample - Episodios","imagem":"/poster.jpg","estudio":"Studio","url":"sample","tags":"Action"}]}"#;
const DETAILS_FIXTURE: &str =
    r#"<div id="anime"><img src="/poster.jpg"><div class="boxAnimeSobreLinha"><b>Hentai:</b><a>Sample</a></div><div class="boxAnimeSobreLinha"><b>Tags</b><a>Action</a></div><div id="sinopse2">Sample.</div><ul class="pagAniListaContainer"><li><a href="/sample-1">Episodio 1</a></li></ul></div>"#;
const PLAYER_FIXTURE: &str = r#"<iframe class="meu-player" src="/hd.php?id=1"></iframe>"#;

export_video_source!(SOURCE);
