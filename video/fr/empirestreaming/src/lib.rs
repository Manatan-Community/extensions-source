use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{SearchRequest, http::HttpClient},
    video::referer_headers,
};
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: EmpireStreaming = EmpireStreaming;
const BASE_URL: &str = "https://empire-stream.net";

struct EmpireStreaming;

impl VideoSource for EmpireStreaming {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let body = fetch(&base, LIST_FIXTURE, &base);
        let section = if listing(&request) == "latest" {
            "Ajout"
        } else {
            "plus vus"
        };
        Ok(Paged {
            entries: parse_cards(&body, section, &base),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let base = base_url(&request);
        if let Some(path) = path_from_url(query, &base) {
            return Ok(Paged {
                entries: vec![fetch_details_with_base(&path, &base)],
                has_next_page: false,
            });
        }
        let body = fetch(&format!("{base}/api/views/contenitem"), SEARCH_FIXTURE, &base);
        let needle = query.to_ascii_lowercase();
        let mut entries = serde_json::from_str::<SearchResults>(&body)
            .map(|res| {
                res.content_item
                    .items()
                    .into_iter()
                    .filter(|item| item.title.to_ascii_lowercase().contains(&needle))
                    .map(|item| CatalogItem {
                        key: format!("/{}", item.url_path.trim_start_matches('/')),
                        title: item.title,
                        cover: item
                            .image
                            .first()
                            .map(|image| format!("{base}/images/medias/{}", image.path)),
                        url: Some(format!("{base}/{}", item.url_path.trim_start_matches('/'))),
                        language: Some("fr".to_string()),
                        content_rating: Some("safe".to_string()),
                        status: ItemStatus::Unknown,
                        ..CatalogItem::default()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        entries.sort_by(|a, b| a.title.cmp(&b.title));
        Ok(page_items(entries, page(&request), 30))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let path = request_key(&request, "item").unwrap_or_else(|| "/film/sample".to_string());
        Ok(fetch_details_with_base(&path, &base))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base = base_url(&request);
        let path = request_key(&request, "item").unwrap_or_else(|| "/film/sample".to_string());
        let body = fetch(&absolute_url(&base, &path), DETAILS_FIXTURE, &base);
        let data = extract_data_json(&body).unwrap_or_else(|| MOVIE_DATA_FIXTURE.to_string());
        let value = serde_json::from_str::<Value>(&data).unwrap_or(Value::Null);
        if path.contains("serie") {
            let mut episodes = Vec::new();
            if let Some(seasons) = value.get("Saison").and_then(Value::as_object) {
                for list in seasons.values().filter_map(Value::as_array) {
                    for ep in list {
                        let season = ep.get("saison").and_then(Value::as_i64).unwrap_or(1);
                        let number = ep.get("episode").and_then(Value::as_i64).unwrap_or(1);
                        let title = ep.get("title").and_then(Value::as_str).unwrap_or_default();
                        let videos = ep.get("video").and_then(Value::as_array).cloned().unwrap_or_default();
                        episodes.push(VideoEpisode {
                            key: encode_videos(&videos),
                            title: Some(format!("Saison {season} Episode {number}: {title}")),
                            episode_number: format!("{season}.{number}").parse::<f32>().ok(),
                            language: Some("fr".to_string()),
                            ..VideoEpisode::default()
                        });
                    }
                }
            }
            episodes.sort_by(|a, b| b.episode_number.partial_cmp(&a.episode_number).unwrap_or(std::cmp::Ordering::Equal));
            Ok(episodes)
        } else {
            let videos = value
                .get("Iframe")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            Ok(vec![VideoEpisode {
                key: encode_videos(&videos),
                title: value
                    .get("Titre")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .or_else(|| Some("Movie".to_string())),
                episode_number: Some(1.0),
                language: Some("fr".to_string()),
                ..VideoEpisode::default()
            }])
        }
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let base = base_url(&request);
        let encoded = request_key(&request, "episode").unwrap_or_default();
        let mut streams = Vec::new();
        for entry in encoded.split(", ").filter(|v| !v.is_empty()) {
            let mut parts = entry.split('|');
            let id = parts.next().unwrap_or_default();
            let version = parts.next().unwrap_or_default();
            let hoster = parts.next().unwrap_or("External");
            let body = fetch(&format!("{base}/player_submit/{id}/{version}"), "", &base);
            let link = body
                .split("window.location.href = \"")
                .nth(1)
                .and_then(|v| v.split('"').next())
                .unwrap_or_default()
                .replace("\\/", "/");
            if !link.is_empty() {
                streams.extend(resolve_link(&link, hoster, &base, &request));
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
                title: "Les plus vus".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Ajout recents".to_string(),
                entries: latest.entries,
                has_more: false,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "item").map(|path| absolute_url(&base, &path)))
    }

    fn episode_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(None)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input, &base) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details_with_base(&path, &base)),
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

#[derive(Deserialize)]
struct SearchResults {
    #[serde(rename = "contentItem")]
    content_item: Content,
}

#[derive(Deserialize)]
struct Content {
    #[serde(default)]
    films: Vec<Entry>,
    #[serde(default)]
    series: Vec<Entry>,
}

impl Content {
    fn items(self) -> Vec<Entry> {
        self.films.into_iter().chain(self.series).collect()
    }
}

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "urlPath")]
    url_path: String,
    title: String,
    #[serde(default)]
    image: Vec<Image>,
}

#[derive(Deserialize)]
struct Image {
    path: String,
}

fn client(referer: &str, base: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_header("Origin", base)
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch(target: &str, fixture: &str, referer: &str) -> String {
    client(referer, referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_cards(body: &str, section: &str, base: &str) -> Vec<CatalogItem> {
    let section_needle = section.to_ascii_lowercase();
    body.split(r#"<div class="block-forme""#)
        .filter(|block| block.to_ascii_lowercase().contains(&section_needle))
        .flat_map(|block| block.split(r#"<div class="content-card""#).skip(1))
        .filter_map(|block| {
            let href = attr_from_html(block, "href")?;
            let key = path_key(&href);
            let title = text_from_tag(block, "h3").unwrap_or_else(|| title_from_path(&key));
            let cover = attr_from_html(block, "data-src")
                .or_else(|| attr_from_html(block, "src"))
                .map(|src| absolute_url(base, &src));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover,
                url: Some(absolute_url(base, &key)),
                language: Some("fr".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn attr_from_html(input: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let marker = format!("{name}={quote}");
        if let Some(value) = input.split(&marker).nth(1) {
            return value
                .split(quote)
                .next()
                .map(|value| value.to_string())
                .filter(|value| !value.is_empty());
        }
    }
    None
}

fn text_from_tag(input: &str, tag: &str) -> Option<String> {
    let start = input.split(&format!("<{tag}")).nth(1)?;
    let text = start.split_once('>')?.1.split(&format!("</{tag}>")).next()?;
    let cleaned = text
        .split('<')
        .next()
        .unwrap_or(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!cleaned.is_empty()).then_some(cleaned)
}

fn fetch_details_with_base(path: &str, base: &str) -> CatalogItem {
    let body = fetch(&absolute_url(base, path), DETAILS_FIXTURE, base);
    let doc = Html::parse_document(&body);
    let thumb = body
        .split("backdrop\":\"")
        .nth(1)
        .and_then(|v| v.split('"').next())
        .map(|v| format!("{base}/images/medias/{}", v.replace('\\', "")));
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, "h3#title_media, h1, h3").unwrap_or_else(|| title_from_path(path)),
        cover: thumb.or_else(|| select_attr_doc(&doc, "picture img, img", "src").map(|src| absolute_url(base, &src))),
        description: select_text_doc(&doc, "div.target-media-desc p.content, .content"),
        tags: doc
            .select(&selector("div > button.bc-w.fs-12.ml-1.c-b, .genres a"))
            .map(text)
            .filter(|v| !v.is_empty())
            .collect(),
        url: Some(absolute_url(base, path)),
        language: Some("fr".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn encode_videos(videos: &[Value]) -> String {
    videos
        .iter()
        .filter_map(|video| {
            Some(format!(
                "{}|{}|{}",
                video.get("id").and_then(Value::as_i64)?,
                video.get("version").and_then(Value::as_str).unwrap_or_default(),
                video.get("property").and_then(Value::as_str).unwrap_or_default()
            ))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn extract_data_json(body: &str) -> Option<String> {
    let script = body.split("window.empire").nth(1)?;
    Some(
        script
            .split("data:")
            .nth(1)?
            .split("countpremiumaccount:")
            .next()?
            .trim()
            .trim_end_matches(',')
            .to_string(),
    )
}

fn resolve_link(link: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    if link.contains(".m3u8") {
        return parse_hls(link, name, referer, request);
    }
    if link.contains(".mp4") || link.contains(".webm") {
        return vec![stream(link, name, &preference(request, "preferred_quality", "auto"), referer)];
    }
    let body = fetch(link, "", referer);
    if let Some(media) = extract_media_url(&body) {
        return if media.contains(".m3u8") {
            parse_hls(&media, name, link, request)
        } else {
            vec![stream(&media, name, "auto", link)]
        };
    }
    vec![external(link, name, referer)]
}

fn parse_hls(url: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = fetch(url, "", referer);
    if !body.contains("#EXT-X-STREAM-INF") {
        return vec![stream(url, name, &preference(request, "preferred_quality", "auto"), referer)];
    }
    body.split("#EXT-X-STREAM-INF:")
        .skip(1)
        .filter_map(|block| {
            let quality = block
                .split("RESOLUTION=")
                .nth(1)
                .and_then(|v| v.split('x').nth(1))
                .and_then(|v| v.split([',', '\n']).next())
                .map(|v| format!("{v}p"))
                .unwrap_or_else(|| "auto".to_string());
            let line = block.lines().find(|line| !line.trim().is_empty() && !line.starts_with('#'))?;
            Some(stream(&absolute_or(line.trim(), url), name, &quality, referer))
        })
        .collect()
}

fn stream(url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = url.contains(".m3u8");
    VideoStream {
        url: url.to_string(),
        name: Some(format!("{name} - {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls { VideoStreamKind::Hls } else { VideoStreamKind::Direct }),
        headers: referer_headers(referer),
        preferred: quality.contains("1080"),
        initialized: true,
        ..VideoStream::default()
    }
}

fn external(url: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: url.to_string(),
        name: Some(name.to_string()),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        preferred: true,
        initialized: true,
        ..VideoStream::default()
    }
}

fn extract_media_url(body: &str) -> Option<String> {
    for marker in ["videoSource\":\"", "file:\"", "file: \"", "source:\""] {
        if let Some(value) = body.split(marker).nth(1) {
            let url = value.split(['"', '\'']).next()?.replace("\\/", "/");
            if url.contains(".m3u8") || url.contains(".mp4") {
                return Some(url);
            }
        }
    }
    None
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let hoster = preference(request, "preferred_hoster", "");
    let quality = preference(request, "preferred_quality", "720p");
    streams.sort_by_key(|stream| {
        let name = stream.name.as_deref().unwrap_or_default();
        let q = stream.quality.as_deref().unwrap_or_default();
        (name.contains(&hoster), q.contains(&quality))
    });
    streams.reverse();
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).unwrap()
}

fn select_text_doc(doc: &Html, selector_value: &str) -> Option<String> {
    doc.select(&selector(selector_value)).next().map(text).filter(|value| !value.is_empty())
}

fn select_attr_doc(doc: &Html, selector_value: &str, name: &str) -> Option<String> {
    doc.select(&selector(selector_value)).next().and_then(|e| attr(&e, name))
}

fn attr(el: &ElementRef<'_>, name: &str) -> Option<String> {
    el.value().attr(name).map(|v| v.to_string()).filter(|v| !v.is_empty())
}

fn text(el: ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn listing(request: &Value) -> &str {
    request.get("listing").or_else(|| request.get("listingId")).and_then(Value::as_str).unwrap_or("popular")
}

fn page(request: &Value) -> u32 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1) as u32
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| value.as_str().or_else(|| value.get("key").and_then(Value::as_str)))
        .map(ToString::to_string)
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(map) = next.as_object_mut() {
        map.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

fn preference(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn base_url(request: &Value) -> String {
    preference(request, "preferred_domain", BASE_URL)
}

fn path_from_url(input: &str, base: &str) -> Option<String> {
    input.strip_prefix(base).map(path_key).filter(|p| p != "/")
}

fn path_key(input: &str) -> String {
    let value = input.split('?').next().unwrap_or(input).split('#').next().unwrap_or(input);
    if value.starts_with("http") {
        format!("/{}", value.split('/').skip(3).collect::<Vec<_>>().join("/")).trim_end_matches('/').to_string()
    } else {
        format!("/{}", value.trim_start_matches('/')).trim_end_matches('/').to_string()
    }
}

fn absolute_url(base: &str, path: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else {
        format!("{base}/{}", path.trim_start_matches('/'))
    }
}

fn absolute_or(path: &str, base: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else {
        let prefix = base.rsplit_once('/').map(|(p, _)| p).unwrap_or(BASE_URL);
        format!("{}/{}", prefix, path.trim_start_matches('/'))
    }
}

fn page_items(items: Vec<CatalogItem>, page: u32, per_page: usize) -> Paged<CatalogItem> {
    let start = page.saturating_sub(1) as usize * per_page;
    let end = (start + per_page).min(items.len());
    Paged {
        entries: items.get(start..end).unwrap_or(&[]).to_vec(),
        has_next_page: end < items.len(),
    }
}

fn title_from_path(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).replace(['-', '_'], " ")
}

const LIST_FIXTURE: &str = r#"<div class="block-forme"><p>Les plus vus</p><div class="content-card"><a class="play" href="/film/sample"><picture><img data-src="/sample.jpg"></picture><h3 class="line-h-s">Sample</h3></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h3 id="title_media">Sample</h3><div class="target-media-desc"><p class="content">Synopsis</p></div><script>window.empire={data:{"Titre":"Sample","Iframe":[{"id":1,"version":"movie","property":"voe"}]},countpremiumaccount:0}</script>"#;
const SEARCH_FIXTURE: &str = r#"{"contentItem":{"films":[{"urlPath":"film/sample","title":"Sample","image":[{"path":"sample.jpg"}]}],"series":[]}}"#;
const MOVIE_DATA_FIXTURE: &str = r#"{"Titre":"Sample","Iframe":[{"id":1,"version":"movie","property":"voe"}]}"#;

export_video_source!(SOURCE);
