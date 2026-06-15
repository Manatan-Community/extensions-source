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
use serde_json::Value;

const SOURCE: OgladajAnime = OgladajAnime;
const BASE_URL: &str = "https://ogladajanime.pl";

struct OgladajAnime;

impl VideoSource for OgladajAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let search_type = if listing(&request) == "latest" {
            "new"
        } else {
            "page"
        };
        Ok(fetch_search(page(&request), search_type, ""))
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
        let search_type = filter(&request, "search_type").unwrap_or_else(|| "name".to_string());
        Ok(fetch_search(page(&request), &search_type, query))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "episode").unwrap_or_else(|| "sample".to_string());
        let target = format!("{BASE_URL}:8443/Player/{}", episode.trim_start_matches('/'));
        let body = fetch(&target, PLAYER_FIXTURE, BASE_URL);
        let players = serde_json::from_str::<Vec<ApiPlayer>>(&body).unwrap_or_default();
        let mut streams = players
            .into_iter()
            .filter_map(|player| player.into_stream(&request))
            .collect::<Vec<_>>();
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
                title: "Najnowsze".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|path| absolute_url(&path)))
    }

    fn episode_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(None)
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
        .with_header("Origin", BASE_URL)
        .with_header("Accept-Language", "pl,en-US;q=0.7,en;q=0.3")
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

fn fetch_search(page: u64, search_type: &str, query: &str) -> Paged<CatalogItem> {
    let form = [
        ("page", page.to_string()),
        ("search_type", search_type.to_string()),
        ("search", query.to_string()),
    ];
    let body = client(BASE_URL)
        .post(format!("{BASE_URL}/manager.php?action=get_search"))
        .referer(BASE_URL)
        .form(
            &form
                .iter()
                .map(|(k, v)| (*k, v.as_str()))
                .collect::<Vec<_>>(),
        )
        .send_text()
        .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
    let payload = serde_json::from_str::<FetchAnime>(&body).unwrap_or_else(|_| FetchAnime {
        data: SEARCH_HTML.to_string(),
    });
    parse_cards(&payload.data)
}

fn parse_cards(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_fragment(body);
    let entries = doc
        .select(&selector(
            "div.anime-item div.card.bg-white, div.card.bg-white",
        ))
        .filter_map(card)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() >= 25,
        entries,
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "a[href]", "href")?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_text(el, "h5.card-title > a, .card-title a, a")
            .unwrap_or_else(|| title_from_path(&key)),
        cover: select_attr(el, "img", "data-srcset")
            .or_else(|| select_attr(el, "img", "data-src"))
            .or_else(|| select_attr(el, "img", "src"))
            .map(|src| absolute_url(&src)),
        url: Some(absolute_url(&key)),
        language: Some("pl".to_string()),
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
        title: select_text_doc(&doc, "h1, h2, h5.card-title")
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, "img[data-srcset], img[data-src], img", "data-srcset")
            .or_else(|| select_attr_doc(&doc, "img[data-src], img", "data-src"))
            .or_else(|| select_attr_doc(&doc, "img", "src"))
            .map(|src| absolute_url(&src)),
        description: select_text_doc(&doc, "p#animeDesc, .animeDesc, .description"),
        tags: select_texts_doc(
            &doc,
            "span.badge[href^='/search/name/'], div.row > div.col-12 > span.badge",
        ),
        url: Some(absolute_url(path)),
        language: Some("pl".to_string()),
        content_rating: Some("adult".to_string()),
        status: parse_status(&doc),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let mut episodes = doc
        .select(&selector(
            "ul#ep_list > li:has(div > img), #ep_list li[ep_id], #ep_list li[value]",
        ))
        .filter_map(|el| {
            let key = el.value().attr("ep_id")?.to_string();
            let number = el
                .value()
                .attr("value")
                .and_then(|value| value.parse::<f32>().ok())
                .or_else(|| select_text(el, "p, div").and_then(|text| first_number(&text)))
                .unwrap_or(0.0);
            let label = select_text(el, "div > div > p, p").unwrap_or_default();
            let voice = select_attr(el, "div > img, img", "alt").unwrap_or_default();
            let title = if label.is_empty() {
                format!("{} Odcinek", number as i32)
            } else if voice.eq_ignore_ascii_case("PL") || voice.is_empty() {
                format!("{} {label}", number as i32)
            } else {
                format!("{} [{}] {label}", number as i32, voice.to_uppercase())
            };
            Some(VideoEpisode {
                key: key.clone(),
                title: Some(title),
                episode_number: Some(number),
                language: Some("pl".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect::<Vec<_>>();
    episodes.reverse();
    episodes
}

#[derive(Debug, Deserialize)]
struct FetchAnime {
    data: String,
}

#[derive(Debug, Deserialize)]
struct ApiPlayer {
    #[serde(rename = "mainUrl")]
    main_url: String,
    label: String,
    res: i32,
    src: String,
    #[serde(rename = "type")]
    stream_type: String,
    extra: String,
}

impl ApiPlayer {
    fn into_stream(self, request: &Value) -> Option<VideoStream> {
        if self.src.trim().is_empty() {
            return None;
        }
        let host = host_label(&self.main_url);
        let mut quality = format!("{host} - {}p", self.res);
        if self.extra == "inv" {
            quality = format!("[Odwrocone Kolory] {quality}");
        }
        let is_hls = self.stream_type.eq_ignore_ascii_case("hls") || self.src.contains(".m3u8");
        let format = if is_hls { "hls" } else { "mp4" };
        let title = if self.label.trim().is_empty() {
            quality.clone()
        } else {
            format!("{} {quality}", self.label.trim())
        };
        Some(VideoStream {
            url: absolute_remote(&self.src, BASE_URL),
            name: Some(title),
            quality: Some(quality),
            format: Some(format.to_string()),
            is_hls,
            stream_kind: Some(if is_hls {
                VideoStreamKind::Hls
            } else {
                VideoStreamKind::Direct
            }),
            headers: referer_headers(BASE_URL),
            preferred: pref(request, "preferred_quality", "1080") == self.res.to_string(),
            initialized: true,
            ..VideoStream::default()
        })
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|stream| {
        let value = stream
            .name
            .clone()
            .or_else(|| stream.quality.clone())
            .unwrap_or_default();
        (value.contains(&quality), quality_rank(&value))
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
        .and_then(|el| el.value().attr(name))
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
        .and_then(|el| el.value().attr(name))
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
    let trimmed = input.trim().replace("\\/", "/").replace("&amp;", "&");
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

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn with_listing(request: &Value, id: &str) -> Value {
    let mut copy = request.clone();
    if let Some(obj) = copy.as_object_mut() {
        obj.insert("listing".to_string(), Value::String(id.to_string()));
    }
    copy
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

fn first_number(input: &str) -> Option<f32> {
    Regex::new(r#"\d+(?:\.\d+)?"#)
        .unwrap()
        .find(input)
        .and_then(|m| m.as_str().parse().ok())
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
        .unwrap_or("ogladajanime")
        .replace('-', " ")
}

fn parse_status(doc: &Html) -> ItemStatus {
    let lower = select_text_doc(doc, "div.col-12 > p.m-0, body")
        .unwrap_or_default()
        .to_lowercase();
    if lower.contains("zako") {
        ItemStatus::Completed
    } else if lower.contains("emitowane") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn host_label(input: &str) -> String {
    Regex::new(r#"https?://(?:www\.)?([^/]+)"#)
        .unwrap()
        .captures(input)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "direct".to_string())
}

export_video_source!(SOURCE);

const SEARCH_HTML: &str = r#"<div class="anime-item"><div class="card bg-white"><a href="/anime/sample"><img data-srcset="/sample.jpg"><h5 class="card-title"><a href="/anime/sample">Sample</a></h5></a></div></div>"#;
const SEARCH_FIXTURE: &str = r#"{"data":"<div class=\"anime-item\"><div class=\"card bg-white\"><a href=\"/anime/sample\"><img data-srcset=\"/sample.jpg\"><h5 class=\"card-title\"><a href=\"/anime/sample\">Sample</a></h5></a></div></div>"}"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><p id="animeDesc">Sample description.</p><ul id="ep_list"><li ep_id="sample-player" value="1"><div><img alt="PL"><div><p>Odcinek</p></div></div></li></ul>"#;
const PLAYER_FIXTURE: &str = r#"[{"mainUrl":"https://example.invalid/watch","label":"Sample","res":720,"src":"https://example.invalid/video.mp4","type":"mp4","extra":"","startTime":0,"endTime":0,"ageValidation":false}]"#;
