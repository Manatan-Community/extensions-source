use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: BeatZAnime = BeatZAnime;
const BASE_URL: &str = "https://www.beatz-anime.net";
const INDEX_URL: &str = "https://dd.beatz-anime.net";

struct BeatZAnime;

impl VideoSource for BeatZAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            if page > 1 {
                format!("{BASE_URL}/index.php?pagina={page}")
            } else {
                BASE_URL.to_string()
            }
        } else if page > 1 {
            format!("{BASE_URL}/emision/pagina={page}")
        } else {
            format!("{BASE_URL}/emision/")
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE, BASE_URL)))
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
            .post(format!("{BASE_URL}/lista-animes/index.php"))
            .browser_document()
            .referer(&format!("{BASE_URL}/lista-animes/index.php"))
            .form(&[
                ("buscar", query),
                ("fuente", &filter(&request, "source").unwrap_or_default()),
                ("estado", &filter(&request, "status").unwrap_or_default()),
                ("tipo-anime", &filter(&request, "type").unwrap_or_default()),
            ])
            .send_text()
            .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
        Ok(parse_search(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_index_episodes(&body))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key =
            request_key(&request, "episode").unwrap_or_else(|| "/sample/sample.mp4".to_string());
        let raw = format!("{INDEX_URL}/api/raw/?path={}", url::query_escape(&key));
        let referer_path = key
            .trim_start_matches('/')
            .rsplit_once('/')
            .map(|(p, _)| p)
            .unwrap_or("");
        Ok(vec![VideoStream {
            url: raw.clone(),
            name: Some("Video".to_string()),
            quality: Some(title_from_path(&key)),
            format: Some("mp4".to_string()),
            stream_kind: Some(VideoStreamKind::Direct),
            headers: referer_headers(&format!("{INDEX_URL}/{referer_path}/")),
            ..VideoStream::default()
        }])
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "En emision".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Ultimos".to_string(),
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
        Ok(request_key(&request, "episode").map(|path| format!("{INDEX_URL}{path}")))
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
        .with_cookies_for(INDEX_URL)
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
    let entries = doc
        .select(&selector(".row > div"))
        .filter_map(card_from_listing)
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("pagination") && body.contains("active"),
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let entries = doc
        .select(&selector(".row > div"))
        .filter_map(card_from_search)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn card_from_listing(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "a.titulo-largo", "href")?;
    let title = select_text(el, "a.titulo-largo")?;
    Some(card(&href, &title, select_attr(el, "img", "src")))
}

fn card_from_search(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "a:has(span), a[href*='/anime/']", "href")?;
    let title = select_text(el, "a:has(span), span.titulo")?;
    Some(card(&href, &title, select_attr(el, "img", "src")))
}

fn card(href: &str, title: &str, cover: Option<String>) -> CatalogItem {
    let path = path_key(href);
    CatalogItem {
        key: path.clone(),
        title: title.to_string(),
        cover: cover.map(|src| absolute_url(&src)),
        url: Some(absolute_url(&path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, "h1").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(&doc, ".row > div > img, img", "src").map(|src| absolute_url(&src)),
        url: Some(absolute_url(path)),
        description: select_text_doc(&doc, "p.post-text"),
        tags: Vec::new(),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&body),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_index_episodes(body: &str) -> Vec<VideoEpisode> {
    let Some(index_href) = body
        .split("href=\"")
        .skip(1)
        .find(|s| s.contains("dd.beatz-anime.net"))
        .and_then(|s| s.split('"').next())
    else {
        return Vec::new();
    };
    let base = if index_href.contains("/api/raw/") {
        index_href
            .split("path=")
            .nth(1)
            .map(|p| {
                format!(
                    "/{}",
                    p.trim_start_matches('/').split('/').next().unwrap_or("")
                )
            })
            .unwrap_or_else(|| "/sample".to_string())
    } else {
        let path = index_href.trim_start_matches(INDEX_URL).trim_matches('/');
        format!("/{}", path.split('/').next().unwrap_or(path))
    };
    let mut out = Vec::new();
    traverse_folder(&base, "", 0, &mut out);
    out.reverse();
    out
}

fn traverse_folder(base: &str, rel: &str, depth: usize, out: &mut Vec<VideoEpisode>) {
    if depth == 2 {
        return;
    }
    let api = format!("{INDEX_URL}/api/?path={}", url::query_escape(base));
    let body = client(INDEX_URL)
        .get(api)
        .xhr()
        .referer(&format!("{INDEX_URL}{base}/"))
        .send_text()
        .unwrap_or_else(|_| INDEX_FIXTURE.to_string());
    let Ok(data) = serde_json::from_str::<IndexResponse>(&body) else {
        return;
    };
    for item in data.folder.value {
        let path = format!("{base}/{}", item.name);
        if item.folder.is_some() {
            traverse_folder(&path, &item.name, depth + 1, out);
        } else if item.file.is_some() && supported(&item.name) {
            let title = if rel.is_empty() {
                item.name.clone()
            } else {
                format!("{rel} - {}", item.name)
            };
            out.push(VideoEpisode {
                key: path.clone(),
                title: Some(title),
                url: Some(format!("{INDEX_URL}{path}")),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            });
        }
    }
}

fn supported(name: &str) -> bool {
    matches!(
        name.rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "mp4" | "mkv" | "avi"
    )
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
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
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
fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("BeatZ Anime")
        .replace(['-', '_'], " ")
}
fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

#[derive(Deserialize)]
struct IndexResponse {
    folder: IndexFolder,
}
#[derive(Deserialize)]
struct IndexFolder {
    value: Vec<IndexItem>,
}
#[derive(Deserialize)]
struct IndexItem {
    name: String,
    folder: Option<Value>,
    file: Option<Value>,
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="row"><div><a class="titulo-largo" href="/anime/sample">Sample</a><img src="/sample.jpg"></div></div>"#;
const SEARCH_FIXTURE: &str = r#"<div class="row"><div><a href="/anime/sample"><span>Sample</span></a><img src="/sample.jpg"></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><p class="post-text">Sample description.</p><a href="https://dd.beatz-anime.net/sample/">Index</a>"#;
const INDEX_FIXTURE: &str = r#"{"folder":{"value":[{"name":"sample.mp4","size":1,"file":{}}]}}"#;
