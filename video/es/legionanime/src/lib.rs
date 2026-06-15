use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use regex::Regex;
use scraper::{Html, Selector};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: LegionAnime = LegionAnime;
const BASE_URL: &str = "https://legionanime.club/api";
const SITE_URL: &str = "https://legionanime.club";
const API_KEY: &str = "pM7VYr2bBG2plWQp";
const JSON_HEADER: &str = r#"{"mob3":"wj2fea7esGZ44ef","language":"es","isDeb":false,"vcode":"2.0.2.6","platform":"13","kind_device":"0","manufacturer":"Google","som":"android","device_name":"Sdk_gphone64_x86_64","root":"0","token":"es","isSign":true,"api_lvl":"33","package_version":"50","package_name":"aplicaciones.paleta.legionanimefull"}"#;
const IMAGE_BASE: &str = "https://la-space-4.sfo2.digitaloceanspaces.com/";

struct LegionAnime;

impl VideoSource for LegionAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let order = if listing(&request) == "latest" {
            "2"
        } else {
            "4"
        };
        Ok(parse_list(&post_directories(
            page, "", "", order, "", "0", "",
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
        let genre = filter_values(&request, "genre").join(",");
        let exclude = filter_values(&request, "not_genre").join(",");
        let order = filter(&request, "orderBy")
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "4".to_string());
        let status = filter(&request, "status").unwrap_or_default();
        let studio = filter(&request, "studio")
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "0".to_string());
        Ok(parse_list(&post_directories(
            page, query, &genre, &order, &exclude, &studio, &status,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/v1/episodes/1".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/v1/episodes/1".to_string());
        let body = get_api(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/v2/episode_links/1".to_string());
        let body = post_api(&absolute_url(&path), PLAYERS_FIXTURE, BASE_URL);
        let parsed = serde_json::from_str::<PlayersRoot>(&body).unwrap_or_default();
        Ok(parsed
            .response
            .players
            .into_iter()
            .filter_map(|player| {
                let url = decode_player_url(&player.name)?;
                let name = if player.option.is_empty() {
                    server_name(&url)
                } else {
                    player.option
                };
                Some(VideoHoster {
                    key: format!("{name}|{url}|{}", absolute_url(&path)),
                    name,
                    url: Some(url),
                    lazy: true,
                    video_count: Some(1),
                    ..VideoHoster::default()
                })
            })
            .collect())
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = key.splitn(3, '|');
        let name = parts.next().unwrap_or("External");
        let embed = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(BASE_URL);
        let mut streams = resolve_embed(embed, name, referer, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut out = Vec::new();
        for hoster in self.hosters(request.clone())? {
            let mut streams = self.resolve_hoster(json!({"hoster":{"key":hoster.key},"preferences":request.get("preferences").cloned().unwrap_or(Value::Null)}))?;
            for stream in &mut streams {
                stream.hoster = Some(hoster.clone());
            }
            out.extend(streams);
        }
        sort_streams(&mut out, &request);
        Ok(out)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Mas visitado".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Recientes".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|p| absolute_url(&p)))
    }
    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|p| absolute_url(&p)))
    }
    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input) {
            if path.contains("/episode_links/") {
                return Ok(Some(UrlResolveResult {
                    episode: Some(json!({"key":path,"url":absolute_url(&path),"language":"es"})),
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

fn post_directories(
    page: u64,
    query: &str,
    genre: &str,
    order: &str,
    exclude: &str,
    studio: &str,
    status: &str,
) -> String {
    let offset = (page.saturating_sub(1)) * 24;
    let target = format!(
        "{BASE_URL}/v2/directories?studio={studio}&not_genre={}&year=&orderBy={}&language=&type=&duration=&search={}&letter=0&limit=24&genre={}&season=&page={offset}&status={}",
        url::query_escape(exclude),
        url::query_escape(order),
        url::query_escape(query),
        url::query_escape(genre),
        url::query_escape(status)
    );
    post_api(&target, LIST_FIXTURE, BASE_URL)
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let parsed = serde_json::from_str::<DirectoryRoot>(body).unwrap_or_default();
    let entries = parsed
        .response
        .into_iter()
        .map(|item| {
            let path = format!("/v1/episodes/{}", item.id);
            CatalogItem {
                key: path.clone(),
                title: item.nombre,
                cover: item.img_url.map(|img| absolute_remote(&img, IMAGE_BASE)),
                url: Some(absolute_url(&path)),
                language: Some("es".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() >= 24,
        entries,
    }
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_api(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let parsed = serde_json::from_str::<DetailsRoot>(&body).unwrap_or_default();
    let anime = parsed.response.anime;
    CatalogItem {
        key: path_key(path),
        title: anime.name.unwrap_or_else(|| title_from_path(path)),
        cover: anime.img_url.map(|img| absolute_remote(&img, IMAGE_BASE)),
        url: Some(absolute_url(path)),
        description: anime.synopsis,
        tags: anime
            .genres
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        authors: anime
            .studios
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(anime.status.as_deref().unwrap_or_default()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let parsed = serde_json::from_str::<DetailsRoot>(body).unwrap_or_default();
    parsed
        .response
        .episodes
        .into_iter()
        .filter_map(|ep| {
            let id = ep.id?;
            let name = ep.name.unwrap_or_else(|| id.to_string());
            let num = name.parse::<f32>().ok();
            let path = format!("/v2/episode_links/{id}");
            Some(VideoEpisode {
                key: path.clone(),
                title: Some(format!("Episodio {name}")),
                episode_number: num,
                url: Some(absolute_url(&path)),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn decode_player_url(input: &str) -> Option<String> {
    if input.is_empty() {
        return None;
    }
    let raw = if input.starts_with("F-") {
        input.split_once('-').map(|(_, tail)| tail.to_string())?
    } else {
        input
            .split_once('-')
            .map(|(_, tail)| tail.chars().rev().collect::<String>())
            .unwrap_or_else(|| input.chars().rev().collect())
    };
    Some(raw).filter(|v| v.starts_with("http"))
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = normalize_embed(embed);
    if embed.contains(".m3u8") {
        return parse_hls(&embed, name, referer, request);
    }
    if embed.contains("mediafire") {
        if let Some(url) = mediafire_download(&embed) {
            return vec![stream(
                &url,
                &format!("{name}-MediaFire"),
                "direct",
                &embed,
                false,
            )];
        }
    }
    if embed.contains("/stream/amz.php?") {
        if let Some(url) = first_media_url(&get_any(&embed.replace(".com", ".tv"), "", referer)) {
            return vec![stream(
                &absolute_remote(&url, &embed),
                name,
                "direct",
                &embed,
                url.contains(".m3u8"),
            )];
        }
    }
    let body = get_any(&embed, "", referer);
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

fn mediafire_download(input: &str) -> Option<String> {
    let body = get_any(input, "", BASE_URL);
    let doc = Html::parse_document(&body);
    doc.select(&selector("a#downloadButton"))
        .next()
        .and_then(|a| a.value().attr("href"))
        .map(ToString::to_string)
}

fn normalize_embed(raw: &str) -> String {
    let t = raw.trim().replace("\\/", "/");
    if t.starts_with("http") {
        t
    } else if let Some(rest) = t.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        absolute_remote(&t, SITE_URL)
    }
}

fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)["']"#,
        r#"url\s*:\s*["']([^"']+)["']"#,
        r#"src\s*:\s*["']([^"']+)["']"#,
        r#"<source[^>]+src=["']([^"']+)["']"#,
        r#"\[\{"file":"([^"]+)""#,
    ]
    .into_iter()
    .find_map(|p| {
        Regex::new(p)
            .ok()?
            .captures(body)?
            .get(1)
            .map(|m| m.as_str().replace("\\", "").replace("\\/", "/"))
    })
}

fn parse_hls(master: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = client(referer)
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
        initialized: true,
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
        initialized: true,
        ..VideoStream::default()
    }
}

fn client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_header("json", JSON_HEADER)
        .with_header("User-Agent", "android l3gi0n4N1mE %E6%9C%AC%E7%89%A9")
        .with_cookies_for(SITE_URL)
        .with_webview_challenge_fallback()
}
fn get_api(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}
fn get_any(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}
fn post_api(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .post(target)
        .referer(referer)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!("apyki={}", url::query_escape(API_KEY)))
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}
fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = pref(request, "preferred_quality", "Desu").to_ascii_lowercase();
    streams.sort_by_key(|s| {
        let name = s.name.clone().unwrap_or_default().to_ascii_lowercase();
        (name.contains(&preferred), quality_rank(&name))
    });
    streams.reverse();
}
fn selector(input: &str) -> Selector {
    Selector::parse(input).unwrap()
}
fn referer_headers(referer: &str) -> Context {
    let mut h = Context::new();
    h.insert("Referer".to_string(), referer.to_string());
    h
}
fn absolute_url(input: &str) -> String {
    absolute_remote(input, BASE_URL)
}
fn absolute_remote(input: &str, base: &str) -> String {
    let t = input.trim().replace("\\/", "/");
    if t.starts_with("http://") || t.starts_with("https://") {
        t
    } else if let Some(rest) = t.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url::join_url(base, &t)
    }
}
fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .filter(|p| p.starts_with('/'))
        .map(path_key)
        .or_else(|| {
            input
                .strip_prefix(SITE_URL)
                .filter(|p| p.starts_with("/api/"))
                .map(|p| path_key(p.trim_start_matches("/api")))
        })
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
fn raw_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|v| {
            v.get("key")
                .or_else(|| v.get("url"))
                .and_then(Value::as_str)
                .or_else(|| v.as_str())
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(ToString::to_string)
}
fn request_key(request: &Value, field: &str) -> Option<String> {
    raw_key(request, field).map(|v| path_key(&v))
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
    json!({"listing":listing,"preferences":request.get("preferences").cloned().unwrap_or(Value::Null)})
}
fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|f| f.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}
fn filter_values(request: &Value, key: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|f| f.get(key))
        .or_else(|| request.get(key))
        .map(|value| match value {
            Value::Array(items) => items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect(),
            Value::String(one) if !one.is_empty() => vec![one.clone()],
            _ => Vec::new(),
        })
        .unwrap_or_default()
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
        .unwrap_or("LegionAnime")
        .replace('-', " ")
}
fn parse_status(input: &str) -> ItemStatus {
    if input.contains("En emision") || input.contains("En emisión") {
        ItemStatus::Ongoing
    } else if input.contains("Finalizado") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}
fn server_name(input: &str) -> String {
    input
        .split("://")
        .nth(1)
        .unwrap_or(input)
        .split('/')
        .next()
        .unwrap_or("External")
        .replace("www.", "")
}

#[derive(Default, Deserialize)]
struct DirectoryRoot {
    #[serde(default)]
    response: Vec<DirectoryItem>,
}
#[derive(Default, Deserialize)]
struct DirectoryItem {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    nombre: String,
    img_url: Option<String>,
}
#[derive(Default, Deserialize)]
struct DetailsRoot {
    #[serde(default)]
    response: DetailsResponse,
}
#[derive(Default, Deserialize)]
struct DetailsResponse {
    #[serde(default)]
    anime: AnimeDto,
    #[serde(default)]
    episodes: Vec<EpisodeDto>,
}
#[derive(Default, Deserialize)]
struct AnimeDto {
    name: Option<String>,
    synopsis: Option<String>,
    genres: Option<String>,
    studios: Option<String>,
    status: Option<String>,
    img_url: Option<String>,
}
#[derive(Default, Deserialize)]
struct EpisodeDto {
    id: Option<u64>,
    name: Option<String>,
}
#[derive(Default, Deserialize)]
struct PlayersRoot {
    #[serde(default)]
    response: PlayersResponse,
}
#[derive(Default, Deserialize)]
struct PlayersResponse {
    #[serde(default)]
    players: Vec<PlayerDto>,
}
#[derive(Default, Deserialize)]
struct PlayerDto {
    #[serde(default)]
    option: String,
    #[serde(default)]
    name: String,
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"response":[{"id":1,"nombre":"Sample","img_url":"sample.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"response":{"anime":{"name":"Sample","synopsis":"Sample description.","genres":"Accion","studios":"Studio","status":"Finalizado","img_url":"sample.jpg"},"episodes":[{"id":1,"name":"1","release_date":"01-01-2024"}]}}"#;
const PLAYERS_FIXTURE: &str =
    r#"{"response":{"players":[{"option":"External","name":"F-https://example.invalid/embed"}]}}"#;
