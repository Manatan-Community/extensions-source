use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: MhdFlix = MhdFlix;
const BASE_URL: &str = "https://ww1.mhdflix.com";
const API_URL: &str = "https://core.mhdflix.com";
const TMDB_IMAGE_URL: &str = "https://image.tmdb.org/t/p/w500";

struct MhdFlix;

impl VideoSource for MhdFlix {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if listing(&request) == "latest" {
            return Ok(parse_latest(&api_get(
                &format!("{API_URL}/api/serie/episode/last?page={}", page(&request)),
                LATEST_FIXTURE,
            )));
        }
        Ok(parse_seo_listing(&api_get(
            &format!("{API_URL}/api/seo/medias?page={}", page(&request)),
            SEO_FIXTURE,
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
        let page = page(&request);
        let type_filter = filter(&request, "type").unwrap_or_default();
        if !query.is_empty() {
            return Ok(parse_media_page(&api_post(
                &format!("{API_URL}/api/search/query"),
                json!({ "query": query, "page": page, "type": type_filter }),
                SEARCH_FIXTURE,
            )));
        }
        if let Some(genre) = filter(&request, "genre").filter(|v| !v.is_empty()) {
            return Ok(parse_media_page(&api_post(
                &format!("{API_URL}/api/search/genres"),
                json!({ "genre": genre, "page": page, "type": type_filter }),
                SEARCH_FIXTURE,
            )));
        }
        if let Some(year) = filter(&request, "year").filter(|v| !v.is_empty()) {
            return Ok(parse_media_page(&api_post(
                &format!("{API_URL}/api/search/year"),
                json!({ "year": year, "page": page, "type": type_filter }),
                SEARCH_FIXTURE,
            )));
        }
        if !type_filter.is_empty() {
            let mut page = parse_seo_listing(&api_get(
                &format!("{API_URL}/api/seo/medias?typeFilter={type_filter}"),
                SEO_FIXTURE,
            ));
            page.entries.retain(|item| item.key.starts_with(&format!("/{type_filter}/")));
            return Ok(page);
        }
        self.list(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/tv/1".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/tv/1".to_string());
        let (kind, id) = decode_path(&path);
        let details = media_detail(id);
        if kind == "movie" || details.kind.as_deref() == Some("movie") {
            return Ok(vec![VideoEpisode {
                key: format!("/movie/{id}"),
                title: Some(format!(
                    "Pelicula{}",
                    details
                        .title
                        .as_deref()
                        .filter(|v| !v.is_empty())
                        .map(|v| format!(" - {v}"))
                        .unwrap_or_default()
                )),
                episode_number: Some(1.0),
                url: Some(format!("{BASE_URL}/movie/{id}")),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let seasons = api_get(&format!("{API_URL}/api/serie/{id}/seasons"), SEASONS_FIXTURE);
        let mut out = Vec::new();
        for season in seasons
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let sid = season
                .get("idSeasson")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let snum = season.get("num").and_then(Value::as_i64).unwrap_or(1);
            let mut ep_page = 1;
            loop {
                let payload = api_get(
                    &format!("{API_URL}/api/serie/episodes/{sid}/{ep_page}"),
                    EPISODES_FIXTURE,
                );
                for ep in payload
                    .get("data")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    let eid = ep
                        .get("idEpisodios")
                        .and_then(Value::as_i64)
                        .unwrap_or_default();
                    let num = ep
                        .get("numEpisode")
                        .and_then(Value::as_f64)
                        .unwrap_or(out.len() as f64 + 1.0) as f32;
                    let title = ep.get("title").and_then(Value::as_str).unwrap_or_default();
                    out.push(VideoEpisode {
                        key: format!("/episode/{eid}"),
                        title: Some(
                            [format!("T{snum}x{}", format_episode_number(num)), title.to_string()]
                                .into_iter()
                                .filter(|v| !v.is_empty())
                                .collect::<Vec<_>>()
                                .join(" - "),
                        ),
                        episode_number: Some(((snum.max(1) - 1) as f32 * 100.0) + num),
                        url: Some(format!("{BASE_URL}/episode/{eid}")),
                        language: Some("es".to_string()),
                        ..VideoEpisode::default()
                    });
                }
                let total = payload
                    .get("totalPage")
                    .and_then(Value::as_u64)
                    .unwrap_or(ep_page);
                if ep_page >= total {
                    break;
                }
                ep_page += 1;
            }
        }
        out.sort_by(|a, b| {
            b.episode_number
                .partial_cmp(&a.episode_number)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(out)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/episode/1".to_string());
        let (kind, id) = decode_path(&path);
        let endpoint = if kind == "movie" {
            format!("{API_URL}/api/links/movie/{id}")
        } else {
            format!("{API_URL}/api/links/episode/{id}")
        };
        let payload = api_get(&endpoint, LINKS_FIXTURE);
        let referer = format!("{BASE_URL}/");
        let mut streams = Vec::new();
        for link in payload
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let Some(embed) = link.get("link").and_then(Value::as_str) else {
                continue;
            };
            let lang = link
                .pointer("/language/name")
                .and_then(Value::as_str)
                .and_then(language_tag)
                .unwrap_or_default();
            let quality = link
                .pointer("/quality/name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let server = link
                .pointer("/server/name")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| matched_server(embed).unwrap_or_else(|| host_name(embed)));
            let label = [lang, quality.to_string(), server.clone()]
                .into_iter()
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
                .join(" - ");
            streams.extend(resolve_embed(embed, &label, &referer, &request));
        }
        streams.sort_by(|a, b| a.url.cmp(&b.url));
        streams.dedup_by(|a, b| a.url == b.url);
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
                title: "Ultimos episodios".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|p| format!("{BASE_URL}{p}")))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|p| format!("{BASE_URL}{p}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input) {
            if path.starts_with("/episode/") || path.starts_with("/movie/") {
                return Ok(Some(UrlResolveResult {
                    episode: Some(json!({ "key": path, "url": input, "language": "es" })),
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

#[derive(Clone, Debug, Default, Deserialize)]
struct MediaDto {
    #[serde(rename = "idMedia")]
    id_media: Option<i64>,
    title: Option<String>,
    slug: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "poster_path")]
    poster_path: Option<String>,
    content: Option<String>,
    status: Option<String>,
    genders: Option<Vec<String>>,
    genre: Option<Vec<String>>,
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(BASE_URL)
        .with_header("Origin", BASE_URL)
        .with_header("Accept", "application/json, text/plain, */*")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_get(target: &str, fixture: &str) -> Value {
    let text = client()
        .get(target)
        .referer(BASE_URL)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    serde_json::from_str(&text).unwrap_or_else(|_| serde_json::from_str(fixture).unwrap())
}

fn api_post(target: &str, body: Value, fixture: &str) -> Value {
    let text = client()
        .post(target)
        .header("Content-Type", "application/json")
        .json(body.to_string())
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    serde_json::from_str(&text).unwrap_or_else(|_| serde_json::from_str(fixture).unwrap())
}

fn parse_seo_listing(payload: &Value) -> Paged<CatalogItem> {
    let mut entries = Vec::new();
    for item in payload
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        if let Some(id) = item.get("idMedia").and_then(Value::as_i64) {
            entries.push(item_from_media(&media_detail(id)));
        }
    }
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_latest(payload: &Value) -> Paged<CatalogItem> {
    let entries = payload
        .get("data")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|ep| {
            let id = ep
                .get("serieId")
                .or_else(|| ep.get("idSerie"))
                .or_else(|| ep.get("idMedia"))
                .and_then(Value::as_i64)
                .unwrap_or_default();
            CatalogItem {
                key: format!("/tv/{id}"),
                title: ep
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("MhdFlix")
                    .to_string(),
                cover: ep
                    .get("poster_path")
                    .and_then(Value::as_str)
                    .map(image_url),
                url: Some(format!("{BASE_URL}/tv/{id}")),
                language: Some("es".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            }
        })
        .collect();
    Paged {
        entries,
        has_next_page: payload
            .get("totalPage")
            .and_then(Value::as_u64)
            .map(|v| v > 1)
            .unwrap_or(false),
    }
}

fn parse_media_page(payload: &Value) -> Paged<CatalogItem> {
    let data = payload.get("data").unwrap_or(&Value::Null);
    let values = if let Some(items) = data.as_array() {
        items.clone()
    } else if let Some(items) = data.get("data").and_then(Value::as_array) {
        items.clone()
    } else {
        Vec::new()
    };
    let entries = values
        .into_iter()
        .filter_map(|v| serde_json::from_value::<MediaDto>(v).ok())
        .map(|m| item_from_media(&m))
        .collect();
    let current = payload
        .get("currentPage")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let total = payload
        .get("totalPage")
        .and_then(Value::as_u64)
        .unwrap_or(current);
    Paged {
        entries,
        has_next_page: current < total,
    }
}

fn media_detail(id: i64) -> MediaDto {
    let payload = api_get(&format!("{API_URL}/api/media/{id}"), DETAIL_FIXTURE);
    serde_json::from_value(payload.get("data").cloned().unwrap_or(Value::Null)).unwrap_or_default()
}

fn fetch_details(path: &str) -> CatalogItem {
    let (_, id) = decode_path(path);
    let media = media_detail(id);
    let mut item = item_from_media(&media);
    item.description = media.content.clone();
    item.tags = media
        .genders
        .unwrap_or_default()
        .into_iter()
        .chain(media.genre.unwrap_or_default())
        .filter(|v| !v.is_empty())
        .collect();
    item.status = match media.status.unwrap_or_default().to_ascii_lowercase().as_str() {
        "ended" | "finalizado" => ItemStatus::Completed,
        "ongoing" | "en emision" | "en emisión" => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    };
    item.initialized = true;
    item
}

fn item_from_media(media: &MediaDto) -> CatalogItem {
    let id = media.id_media.unwrap_or_default();
    let kind = media.kind.as_deref().unwrap_or("tv");
    let title = media
        .title
        .clone()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| media.slug.as_ref().map(|s| s.replace('-', " ")))
        .unwrap_or_else(|| "MhdFlix".to_string());
    CatalogItem {
        key: format!("/{kind}/{id}"),
        title,
        cover: media.poster_path.as_deref().map(image_url),
        url: Some(format!("{BASE_URL}/{kind}/{id}")),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_remote(embed, referer);
    if embed.contains(".m3u8") {
        return parse_hls(&embed, name, referer, request);
    }
    let body = client()
        .get(&embed)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    if let Some(src) = first_media_url(&body).map(|v| absolute_remote(&v, &embed)) {
        if src.contains(".m3u8") {
            parse_hls(&src, name, &embed, request)
        } else {
            vec![stream(&src, name, "direct", &embed, false)]
        }
    } else {
        vec![external_stream(&embed, name, referer)]
    }
}

fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)["']"#,
        r#"src\s*:\s*["']([^"']+)["']"#,
        r#"<source[^>]+src=["']([^"']+)["']"#,
        r#"https?://[^\s'"\\]+\.m3u8[^\s'"\\]*"#,
    ]
    .into_iter()
    .find_map(|p| {
        let re = Regex::new(p).ok()?;
        if p.starts_with("http") {
            re.find(body).map(|m| m.as_str().replace("\\/", "/"))
        } else {
            re.captures(body)?
                .get(1)
                .map(|m| m.as_str().replace("\\/", "/"))
        }
    })
}

fn parse_hls(master: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = client()
        .get(master)
        .referer(referer)
        .send_text()
        .unwrap_or_default();
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
                name,
                &quality,
                referer,
                true,
            ));
        }
    }
    if out.is_empty() {
        out.push(stream(master, name, "auto", referer, true));
    }
    sort_streams(&mut out, request);
    out
}

fn stream(target: &str, name: &str, quality: &str, referer: &str, hls: bool) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if hls { "hls" } else { "mp4" }.to_string()),
        is_hls: hls,
        stream_kind: Some(if hls {
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
    let lang = pref(request, "preferred_language", "[LAT]").to_ascii_lowercase();
    let server = pref(request, "preferred_server", "StreamWish").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|s| {
        let name = s.name.clone().unwrap_or_default().to_ascii_lowercase();
        let q = s.quality.clone().unwrap_or_default();
        (
            name.contains(&lang),
            name.contains(&server),
            q.contains(&quality) || name.contains(&quality),
            quality_rank(&q).max(quality_rank(&name)),
        )
    });
    streams.reverse();
}

fn language_tag(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    Some(if lower.contains("lat") {
        "[LAT]"
    } else if lower.contains("cast") || lower.contains("esp") {
        "[CAST]"
    } else if lower.contains("vose") {
        "[VOSE]"
    } else if lower.contains("sub") {
        "[SUB]"
    } else {
        return Some(format!("[{}]", value.trim()));
    }
    .to_string())
}

fn format_episode_number(number: f32) -> String {
    if number.fract() == 0.0 {
        (number as i32).to_string()
    } else {
        number.to_string()
    }
}

fn matched_server(input: &str) -> Option<String> {
    let lower = input.to_ascii_lowercase();
    [
        ("StreamWish", ["streamwish", "strwish", "wish"].as_slice()),
        ("Filemoon", ["filemoon", "moonplayer"].as_slice()),
        ("VidHide", ["vidhide", "streamhide", "streamvid"].as_slice()),
        ("Voe", ["voe"].as_slice()),
        ("Uqload", ["uqload"].as_slice()),
        ("Lulu", ["lulu", "luluvdo"].as_slice()),
        ("StreamTape", ["streamtape", "stape"].as_slice()),
        ("Doodstream", ["dood", "d000d"].as_slice()),
        ("MixDrop", ["mixdrop", "mix"].as_slice()),
        ("Filelions", ["filelions", "lion"].as_slice()),
    ]
    .into_iter()
    .find(|(_, keys)| keys.iter().any(|key| lower.contains(key)))
    .map(|(name, _)| name.to_string())
}

fn decode_path(path: &str) -> (String, i64) {
    let mut parts = path.trim_matches('/').split('/');
    let kind = parts.next().unwrap_or("tv").to_string();
    let id = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (kind, id)
}

fn image_url(path: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else {
        format!("{TMDB_IMAGE_URL}{path}")
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

fn absolute_remote(input: &str, base: &str) -> String {
    let t = input.trim().replace("\\/", "/");
    if t.starts_with("http") {
        t
    } else if let Some(rest) = t.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url::join_url(base, &t)
    }
}

fn host_name(input: &str) -> String {
    input
        .split("://")
        .nth(1)
        .unwrap_or(input)
        .split('/')
        .next()
        .unwrap_or("External")
        .replace("www.", "")
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

fn pref(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn quality_rank(q: &str) -> i32 {
    Regex::new(r#"(\d+)"#)
        .unwrap()
        .captures(q)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0)
}

fn referer_headers(referer: &str) -> Context {
    let mut h = Context::new();
    h.insert("Referer".to_string(), referer.to_string());
    h
}

const SEO_FIXTURE: &str = r#"{"data":[{"idMedia":1,"type":"tv"}]}"#;
const LATEST_FIXTURE: &str =
    r#"{"data":[{"idSerie":1,"title":"Sample","poster_path":"/sample.jpg"}],"totalPage":1}"#;
const DETAIL_FIXTURE: &str = r#"{"data":{"idMedia":1,"title":"Sample","type":"tv","poster_path":"/sample.jpg","content":"Sample description.","status":"ongoing","genre":["Anime"]}}"#;
const SEASONS_FIXTURE: &str = r#"{"data":[{"idSeasson":1,"num":1}]}"#;
const EPISODES_FIXTURE: &str =
    r#"{"data":[{"idEpisodios":1,"title":"Sample","numEpisode":1}],"totalPage":1}"#;
const LINKS_FIXTURE: &str = r#"{"data":[{"link":"https://example.invalid/embed","server":{"name":"External"},"language":{"name":"LAT"},"quality":{"name":"1080"}}]}"#;
const SEARCH_FIXTURE: &str = r#"{"data":[{"idMedia":1,"title":"Sample","type":"tv","poster_path":"/sample.jpg"}],"currentPage":1,"totalPage":1}"#;

export_video_source!(SOURCE);
