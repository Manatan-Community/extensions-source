use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{SearchRequest, http::HttpClient},
    video::referer_headers,
};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: FrAnime = FrAnime;
const BASE_URL: &str = "https://franime.fr";
const API_URL: &str = "https://api.franime.fr/api";

struct FrAnime;

impl VideoSource for FrAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let mut database = fetch_database();
        if listing(&request) == "popular" {
            database.sort_by(|a, b| b.note.partial_cmp(&a.note).unwrap_or(std::cmp::Ordering::Equal));
        } else {
            database.reverse();
        }
        Ok(page_items(expand_items(&database), page(&request), 50))
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
        let needle = query.to_ascii_lowercase();
        let database = fetch_database()
            .into_iter()
            .filter(|anime| {
                anime.title.to_ascii_lowercase().contains(&needle)
                    || anime.original_title.to_ascii_lowercase().contains(&needle)
                    || anime
                        .titles
                        .as_ref()
                        .map(|titles| {
                            titles.en.as_deref().unwrap_or_default().to_ascii_lowercase().contains(&needle)
                                || titles.en_jp.as_deref().unwrap_or_default().to_ascii_lowercase().contains(&needle)
                                || titles.ja_jp.as_deref().unwrap_or_default().to_ascii_lowercase().contains(&needle)
                        })
                        .unwrap_or(false)
                    || slugify(&anime.original_title).contains(&needle)
            })
            .collect::<Vec<_>>();
        Ok(page_items(expand_items(&database), page(&request), 50))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item")
            .unwrap_or_else(|| "/anime/sample?lang=vo&s=1".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item")
            .unwrap_or_else(|| "/anime/sample?lang=vo&s=1".to_string());
        let parts = item_parts(&path);
        let Some(anime) = find_anime(&parts.slug) else {
            return Ok(Vec::new());
        };
        let Some(season) = anime.seasons.get(parts.season.saturating_sub(1)) else {
            return Ok(Vec::new());
        };
        let mut episodes = season
            .episodes
            .iter()
            .enumerate()
            .filter_map(|(index, episode)| {
                let players = if parts.lang == "vf" {
                    &episode.lang.vf.players
                } else {
                    &episode.lang.vo.players
                };
                if players.is_empty() {
                    return None;
                }
                let number = (index + 1) as f32;
                Some(VideoEpisode {
                    key: format!("{path}&ep={}", index + 1),
                    title: episode
                        .title
                        .clone()
                        .or_else(|| Some(format!("Episode {}", index + 1))),
                    episode_number: Some(number),
                    language: Some("fr".to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect::<Vec<_>>();
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = request_key(&request, "episode")
            .unwrap_or_else(|| "/anime/sample?lang=vo&s=1&ep=1".to_string());
        let parts = item_parts(&path);
        let episode_index = query_param(&path, "ep")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .saturating_sub(1);
        let season_index = parts.season.saturating_sub(1);
        let Some(anime) = find_anime(&parts.slug) else {
            return Ok(Vec::new());
        };
        let Some(episode) = anime
            .seasons
            .get(season_index)
            .and_then(|season| season.episodes.get(episode_index))
        else {
            return Ok(Vec::new());
        };
        let players = if parts.lang == "vf" {
            &episode.lang.vf.players
        } else {
            &episode.lang.vo.players
        };
        let mut streams = Vec::new();
        for (index, player) in players.iter().enumerate() {
            let raw = client()
                .get(format!(
                    "{API_URL}/anime/{}/{season_index}/{episode_index}/{}/{index}",
                    anime.id_string(),
                    parts.lang
                ))
                .referer(BASE_URL)
                .send_text()
                .unwrap_or_default();
            let link = raw.trim().trim_matches('"').replace("\\/", "/");
            if !link.is_empty() {
                streams.extend(resolve_link(&link, player, BASE_URL, &request));
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
                title: "Latest".to_string(),
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

#[derive(Clone, Deserialize)]
struct Anime {
    #[serde(default)]
    themes: Vec<String>,
    #[serde(default, rename = "saisons")]
    seasons: Vec<Season>,
    id: Value,
    #[serde(default, rename = "affiche")]
    poster: String,
    #[serde(default, rename = "titleO")]
    original_title: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    titles: Option<Titles>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    note: f32,
    #[serde(default)]
    status: String,
    #[serde(default)]
    nsfw: bool,
}

impl Anime {
    fn id_string(&self) -> String {
        self.id
            .as_str()
            .map(ToString::to_string)
            .unwrap_or_else(|| self.id.to_string())
            .trim_matches('"')
            .to_string()
    }
}

#[derive(Clone, Deserialize)]
struct Titles {
    #[serde(default)]
    en: Option<String>,
    #[serde(default, rename = "en_jp")]
    en_jp: Option<String>,
    #[serde(default, rename = "ja_jp")]
    ja_jp: Option<String>,
}

#[derive(Clone, Deserialize)]
struct Season {
    #[serde(default)]
    episodes: Vec<Episode>,
}

#[derive(Clone, Deserialize)]
struct Episode {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    lang: EpisodeLanguages,
}

#[derive(Clone, Default, Deserialize)]
struct EpisodeLanguages {
    #[serde(default)]
    vf: EpisodeLanguage,
    #[serde(default)]
    vo: EpisodeLanguage,
}

#[derive(Clone, Default, Deserialize)]
struct EpisodeLanguage {
    #[serde(default, rename = "lecteurs")]
    players: Vec<String>,
}

struct ItemParts {
    slug: String,
    lang: String,
    season: usize,
}

fn fetch_database() -> Vec<Anime> {
    let body = client()
        .get(format!("{API_URL}/animes/"))
        .referer(BASE_URL)
        .send_text()
        .unwrap_or_else(|_| DATABASE_FIXTURE.to_string());
    serde_json::from_str(&body).unwrap_or_default()
}

fn find_anime(slug: &str) -> Option<Anime> {
    fetch_database()
        .into_iter()
        .find(|anime| slugify(&anime.original_title) == slug || slugify(&anime.title) == slug)
}

fn expand_items(database: &[Anime]) -> Vec<CatalogItem> {
    database
        .iter()
        .flat_map(|anime| {
            anime.seasons
                .iter()
                .enumerate()
                .flat_map(move |(season_index, season)| {
                    let has_vo = season.episodes.iter().any(|ep| !ep.lang.vo.players.is_empty());
                    let has_vf = season.episodes.iter().any(|ep| !ep.lang.vf.players.is_empty());
                    [("VOSTFR", "vo", has_vo, has_vf), ("VF", "vf", has_vf, has_vo)]
                        .into_iter()
                        .filter(|(_, _, exists, _)| *exists)
                        .map(move |(label, lang, _, show_label)| catalog_item(anime, season_index, label, lang, show_label))
                })
        })
        .collect()
}

fn catalog_item(anime: &Anime, season_index: usize, label: &str, lang: &str, show_label: bool) -> CatalogItem {
    let slug = slugify(&anime.original_title);
    let season_suffix = if anime.seasons.len() > 1 {
        format!(" S{}", season_index + 1)
    } else {
        String::new()
    };
    let lang_suffix = if show_label { format!(" ({label})") } else { String::new() };
    let key = format!("/anime/{slug}?lang={lang}&s={}", season_index + 1);
    CatalogItem {
        key: key.clone(),
        title: format!("{}{}{}", anime.title, season_suffix, lang_suffix),
        cover: (!anime.poster.is_empty()).then_some(anime.poster.clone()),
        description: (!anime.description.is_empty()).then_some(anime.description.clone()),
        tags: anime.themes.clone(),
        url: Some(absolute_url(&key)),
        language: Some("fr".to_string()),
        content_rating: Some(if anime.nsfw { "adult" } else { "safe" }.to_string()),
        status: parse_status(&anime.status, anime.seasons.len(), season_index + 1),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_details(path: &str) -> CatalogItem {
    let parts = item_parts(path);
    let Some(anime) = find_anime(&parts.slug) else {
        return CatalogItem {
            key: path.to_string(),
            title: title_from_path(path),
            url: Some(absolute_url(path)),
            language: Some("fr".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            initialized: true,
            ..CatalogItem::default()
        };
    };
    let label = if parts.lang == "vf" { "VF" } else { "VOSTFR" };
    catalog_item(&anime, parts.season.saturating_sub(1), label, &parts.lang, true)
}

fn page_items(items: Vec<CatalogItem>, page: u32, per_page: usize) -> Paged<CatalogItem> {
    let start = page.saturating_sub(1) as usize * per_page;
    let end = (start + per_page).min(items.len());
    Paged {
        entries: items.get(start..end).unwrap_or(&[]).to_vec(),
        has_next_page: end < items.len(),
    }
}

fn resolve_link(link: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    if link.contains(".m3u8") {
        return parse_hls(link, name, referer, request);
    }
    if link.contains(".mp4") || link.contains(".webm") {
        return vec![stream(
            link,
            name,
            &preference(request, "preferred_quality", "auto"),
            referer,
        )];
    }
    vec![external(link, name, referer)]
}

fn parse_hls(url: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = client()
        .get(url)
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    if !body.contains("#EXT-X-STREAM-INF") {
        return vec![stream(
            url,
            name,
            &preference(request, "preferred_quality", "auto"),
            referer,
        )];
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
            let line = block
                .lines()
                .find(|line| !line.trim().is_empty() && !line.starts_with('#'))?;
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
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
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

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let quality = preference(request, "preferred_quality", "1080p");
    streams.sort_by_key(|stream| stream.quality.as_deref().unwrap_or_default().contains(&quality));
    streams.reverse();
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(BASE_URL)
        .with_header("Origin", BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn page(request: &Value) -> u32 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1) as u32
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .as_str()
                .or_else(|| value.get("key").and_then(Value::as_str))
        })
        .map(ToString::to_string)
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(map) = next.as_object_mut() {
        map.insert("listing".to_string(), json!(listing));
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

fn item_parts(path: &str) -> ItemParts {
    let slug = path
        .split('?')
        .next()
        .unwrap_or(path)
        .trim_matches('/')
        .split('/')
        .next_back()
        .unwrap_or_default()
        .to_string();
    ItemParts {
        slug,
        lang: query_param(path, "lang").unwrap_or_else(|| "vo".to_string()),
        season: query_param(path, "s")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1),
    }
}

fn query_param(path: &str, key: &str) -> Option<String> {
    path.split('?')
        .nth(1)?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_string())
}

fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .map(|value| format!("/{}", value.trim_start_matches('/')))
        .filter(|path| path.starts_with("/anime/"))
}

fn absolute_url(path: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else {
        format!("{BASE_URL}/{}", path.trim_start_matches('/'))
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

fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn parse_status(status: &str, season_count: usize, season: usize) -> ItemStatus {
    if season < season_count {
        return ItemStatus::Completed;
    }
    match status.trim() {
        "EN COURS" => ItemStatus::Ongoing,
        "TERMINE" | "TERMINÉ" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn title_from_path(path: &str) -> String {
    path.split('?')
        .next()
        .unwrap_or(path)
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .replace(['-', '_'], " ")
}

const DATABASE_FIXTURE: &str = r#"
[{"themes":["Action"],"saisons":[{"title":"Saison 1","episodes":[{"title":"Episode 1","lang":{"vf":{"lecteurs":[]},"vo":{"lecteurs":["external"]}}}]}],"id":1,"source_url":"","affiche":"https://franime.fr/sample.jpg","titleO":"Sample","title":"Sample","titles":{"en":null,"en_jp":null,"ja_jp":null},"description":"Synopsis","note":1.0,"status":"EN COURS","nsfw":false}]
"#;

export_video_source!(SOURCE);
