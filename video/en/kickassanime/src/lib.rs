use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack, UrlResolveResult,
    VideoEpisode, VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult,
    export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: KickAssAnime = KickAssAnime;
const SEARCH_BASE_URL: &str = "https://kaa.lt";
const DEFAULT_BASE_URL: &str = "https://kaa.lt";
struct KickAssAnime;

impl VideoSource for KickAssAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let page = page(&request);
        let url = if listing(&request) == "latest" {
            format!("{base}/api/show/recent?type=all&page={page}")
        } else {
            format!("{base}/api/show/trending?page={page}")
        };
        let body = api_get(&base, &url, if listing(&request) == "latest" { RECENT_FIXTURE } else { POPULAR_FIXTURE });
        Ok(if listing(&request) == "latest" {
            parse_recent(&base, &body, &request)
        } else {
            parse_popular(&base, &body, page, &request)
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(slug) = slug_from_url(query) {
            return Ok(Paged { entries: vec![fetch_details(&base, &slug, &request)], has_next_page: false });
        }
        if query.is_empty() && request.get("filters").is_none() {
            return self.list(request);
        }
        let page = page(&request);
        if query.is_empty() {
            let body = api_get(SEARCH_BASE_URL, &format!("{SEARCH_BASE_URL}/api/anime?page={page}"), SEARCH_FIXTURE);
            return Ok(parse_search(SEARCH_BASE_URL, &body, page, &request));
        }
        let payload = json!({ "page": page, "query": query }).to_string();
        let body = client(SEARCH_BASE_URL)
            .post(format!("{SEARCH_BASE_URL}/api/fsearch"))
            .xhr()
            .header("Accept", "application/json, text/plain, */*")
            .header("Content-Type", "application/json")
            .referer(&format!("{SEARCH_BASE_URL}/search?q={}", url::query_escape(query)))
            .body(payload.into_bytes())
            .send_text()
            .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
        Ok(parse_search(SEARCH_BASE_URL, &body, page, &request))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = request_key(&request, "item").unwrap_or_else(|| "/sample-anime".to_string());
        Ok(fetch_details(&base, key.trim_start_matches('/'), &request))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base = base_url(&request);
        let key = request_key(&request, "item").unwrap_or_else(|| "/sample-anime".to_string());
        let slug = key.trim_start_matches('/');
        let langs_body = api_get(&base, &format!("{base}/api/show/{slug}/language"), LANGUAGES_FIXTURE);
        let mut langs = serde_json::from_str::<LanguagesDto>(&langs_body).map(|dto| dto.result).unwrap_or_default();
        sort_languages(&mut langs, &request);
        let lang = langs.first().cloned().unwrap_or_else(|| "ja-JP".to_string());
        let first = fetch_episode_page(&base, slug, 1, &lang);
        let parsed = serde_json::from_str::<EpisodeResponseDto>(&first).unwrap_or_default();
        let mut all = parsed.result;
        let page_count = parsed.pages.len().max(1);
        for page in 2..=page_count {
            let body = fetch_episode_page(&base, slug, page as u64, &lang);
            if let Ok(next) = serde_json::from_str::<EpisodeResponseDto>(&body) {
                all.extend(next.result);
            }
        }
        let mut episodes = all.into_iter().filter_map(|episode| episode.into_episode(slug, &lang)).collect::<Vec<_>>();
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let base = base_url(&request);
        let episode = request_key(&request, "episode").unwrap_or_else(|| "/sample-anime/ep-1-sample".to_string());
        let url = format!("{base}/api/show{}", episode.replace("/ep-", "/episode/ep-"));
        let body = api_get(&base, &url, SERVERS_FIXTURE);
        let excluded = excluded_hosters(&request);
        let servers = serde_json::from_str::<ServersDto>(&body).unwrap_or_default();
        Ok(servers.servers.into_iter()
            .filter(|server| !excluded.iter().any(|name| name == &server.name))
            .map(|server| VideoHoster {
                key: format!("{}|{}", server.name, server.src),
                name: server.name,
                url: Some(server.src),
                lazy: true,
                video_count: Some(1),
                ..VideoHoster::default()
            })
            .collect())
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let (name, src) = key.split_once('|').unwrap_or(("KickAssAnime", &key));
        let mut streams = resolve_player(src, name, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut streams = Vec::new();
        for hoster in self.hosters(request.clone())? {
            let mut resolved = self.resolve_hoster(json!({
                "hoster": { "key": hoster.key },
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
            }))?;
            for stream in &mut resolved {
                stream.hoster = Some(hoster.clone());
            }
            streams.extend(resolved);
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection { id: "popular".to_string(), title: "Trending".to_string(), style: Some(HomeSectionStyle::Featured), entries: popular.entries, has_more: popular.has_next_page, ..HomeSection::default() },
            HomeSection { id: "latest".to_string(), title: "Recent".to_string(), entries: latest.entries, has_more: latest.has_next_page, ..HomeSection::default() },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "item").map(|key| format!("{base}/{}", key.trim_start_matches('/'))))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "episode").map(|key| format!("{base}{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None) };
        if let Some(slug) = slug_from_url(input) {
            let base = base_url(&request);
            return Ok(Some(UrlResolveResult { item: Some(fetch_details(&base, &slug, &request)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_referer(base)
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn api_get(base: &str, target: &str, fixture: &str) -> String {
    client(base)
        .get(target)
        .xhr()
        .header("Accept", "application/json, text/plain, */*")
        .referer(base)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_episode_page(base: &str, slug: &str, page: u64, lang: &str) -> String {
    api_get(base, &format!("{base}/api/show/{slug}/episodes?page={page}&lang={lang}"), EPISODES_FIXTURE)
}

fn parse_popular(base: &str, body: &str, page: u64, request: &Value) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<PopularResponseDto>(body).unwrap_or_default();
    Paged {
        entries: payload.result.into_iter().map(|item| item.into_item(base, request)).collect(),
        has_next_page: page < payload.page_count,
    }
}

fn parse_recent(base: &str, body: &str, request: &Value) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<RecentResponseDto>(body).unwrap_or_default();
    Paged {
        entries: payload.result.into_iter().map(|item| item.into_item(base, request)).collect(),
        has_next_page: payload.had_next,
    }
}

fn parse_search(base: &str, body: &str, page: u64, request: &Value) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<SearchResponseDto>(body).unwrap_or_default();
    Paged {
        entries: payload.result.into_iter().map(|item| item.into_item(base, request)).collect(),
        has_next_page: page < payload.max_page,
    }
}

fn fetch_details(base: &str, slug: &str, request: &Value) -> CatalogItem {
    let body = api_get(base, &format!("{base}/api/show/{slug}"), DETAILS_FIXTURE);
    let anime = serde_json::from_str::<AnimeInfoDto>(&body).unwrap_or_else(|_| AnimeInfoDto::fallback(slug));
    let languages = api_get(base, &format!("{base}/api/show/{slug}/language"), LANGUAGES_FIXTURE);
    let languages = serde_json::from_str::<LanguagesDto>(&languages).map(|dto| dto.result).unwrap_or_default();
    anime.into_item(base, request, languages)
}

fn resolve_player(src: &str, name: &str, request: &Value) -> Vec<VideoStream> {
    if src.is_empty() {
        return Vec::new();
    }
    let final_url = if src.contains("/vast") { src.replace("/vast", "/cat-player/player") } else { src.to_string() };
    let body = client(DEFAULT_BASE_URL).get(&final_url).browser_document().referer(DEFAULT_BASE_URL).send_text().unwrap_or_default();
    let clean = body.replace("&quot;", "\"");
    let subtitles = parse_player_subtitles(&clean, &final_url);
    if let Some(manifest) = find_manifest(&clean, &final_url) {
        if manifest.contains(".m3u8") {
            return parse_hls(&manifest, name, &final_url, subtitles, request);
        }
        return vec![media_stream(&manifest, name, "dash", &final_url, subtitles, request)];
    }
    vec![external_stream(&final_url, name, request)]
}

fn find_manifest(body: &str, base: &str) -> Option<String> {
    for marker in ["manifest\":[0,\"", "\"hls\":\"", "\"dash\":\"", "file:\"", "file: '"] {
        if let Some(value) = body.split(marker).nth(1).and_then(|tail| tail.split(['"', '\'']).next()) {
            if value.contains("http") || value.contains("//") || value.contains(".m3u8") || value.contains(".mpd") {
                return Some(fix_url(&value.replace("\\/", "/"), base));
            }
        }
    }
    None
}

fn parse_player_subtitles(body: &str, base: &str) -> Vec<SubtitleTrack> {
    let mut out = Vec::new();
    for chunk in body.split("\"src\"").skip(1) {
        let Some(raw) = chunk.split('"').nth(2) else { continue };
        if !raw.contains(".vtt") && !raw.contains(".srt") {
            continue;
        }
        let label = chunk.split("\"name\"").nth(1).and_then(|tail| tail.split('"').nth(2)).unwrap_or("Subtitle");
        let lang = chunk.split("\"language\"").nth(1).and_then(|tail| tail.split('"').nth(2)).unwrap_or(label);
        out.push(SubtitleTrack {
            url: fix_url(&raw.replace("\\/", "/"), base),
            language: language_code(lang),
            label: Some(format!("{label} ({lang})")),
            format: Some(if raw.ends_with(".srt") { "srt" } else { "vtt" }.to_string()),
            headers: referer_headers(base),
            ..SubtitleTrack::default()
        });
    }
    out
}

fn parse_hls(target: &str, name: &str, referer: &str, subtitles: Vec<SubtitleTrack>, request: &Value) -> Vec<VideoStream> {
    let body = client(referer).get(target).referer(referer).send_text().unwrap_or_default();
    if !body.contains("#EXT-X-STREAM-INF") {
        return vec![media_stream(target, name, "auto", referer, subtitles, request)];
    }
    body.split("#EXT-X-STREAM-INF:").skip(1).filter_map(|block| {
        let quality = block.split("RESOLUTION=").nth(1)
            .and_then(|part| part.split('x').nth(1))
            .and_then(|part| part.split([',', '\n']).next())
            .map(|height| format!("{height}p"))
            .unwrap_or_else(|| "auto".to_string());
        let line = block.lines().find(|line| !line.trim().is_empty() && !line.starts_with('#'))?;
        Some(media_stream(&fix_url(line.trim(), target), name, &quality, referer, subtitles.clone(), request))
    }).collect()
}

fn media_stream(url: &str, name: &str, quality: &str, referer: &str, subtitles: Vec<SubtitleTrack>, request: &Value) -> VideoStream {
    let is_hls = url.contains(".m3u8");
    VideoStream {
        url: url.to_string(),
        name: Some(format!("{name} - {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls { VideoStreamKind::Hls } else { VideoStreamKind::Direct }),
        headers: referer_headers(referer),
        subtitles,
        preferred: is_preferred(quality, name, request),
        initialized: true,
        ..VideoStream::default()
    }
}

fn external_stream(url: &str, name: &str, request: &Value) -> VideoStream {
    VideoStream {
        url: url.to_string(),
        name: Some(name.to_string()),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(DEFAULT_BASE_URL),
        preferred: is_preferred("", name, request),
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    streams.sort_by_key(|stream| {
        let q = stream.quality.as_deref().unwrap_or_default();
        let score = q.chars().filter(char::is_ascii_digit).collect::<String>().parse::<i32>().unwrap_or(0);
        (i32::from(is_preferred(q, stream.name.as_deref().unwrap_or_default(), request)), score)
    });
    streams.reverse();
}

fn is_preferred(quality: &str, name: &str, request: &Value) -> bool {
    let pref_quality = pref(request, "preferred_quality", "1080p");
    let pref_server = pref(request, "preferred_server", "VidStreaming");
    quality.contains(&pref_quality) || name.to_lowercase().contains(&pref_server.to_lowercase())
}

fn sort_languages(langs: &mut [String], request: &Value) {
    let pref1 = pref(request, "preferred_audio_lang", "ja-JP");
    let pref2 = pref(request, "preferred_audio_lang_2nd", "en-US");
    langs.sort_by_key(|lang| {
        if lang == &pref1 { 0 } else if lang == &pref2 { 1 } else { 2 }
    });
}

fn excluded_hosters(request: &Value) -> Vec<String> {
    request.get("preferences").and_then(|p| p.get("hoster_exclusion")).and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(ToString::to_string).collect())
        .unwrap_or_default()
}

fn base_url(request: &Value) -> String {
    pref(request, "preferred_domain", DEFAULT_BASE_URL)
}

fn pref(request: &Value, key: &str, default: &str) -> String {
    request.get("preferences").and_then(|p| p.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).unwrap_or(default).to_string()
}

fn pref_bool(request: &Value, key: &str, default: bool) -> bool {
    request.get("preferences").and_then(|p| p.get(key)).or_else(|| request.get(key)).and_then(Value::as_bool).unwrap_or(default)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request.get(field).and_then(|value| {
        value.get("key").or_else(|| value.get("url")).and_then(Value::as_str).or_else(|| value.as_str())
    }).or_else(|| request.get("key").and_then(Value::as_str)).map(|value| {
        if field == "item" { format!("/{}", value.trim_start_matches('/')) } else { value.to_string() }
    })
}

fn slug_from_url(input: &str) -> Option<String> {
    let host_ok = input.starts_with("https://kaa.lt")
        || input.starts_with("https://kickass-anime.ru")
        || input.starts_with("https://kickass-anime.ro")
        || input.starts_with("https://kaa.to")
        || input.starts_with("https://kaa.rs")
        || input.starts_with('/');
    if !host_ok {
        return None;
    }
    let path = input.split("://").nth(1).and_then(|tail| tail.split_once('/').map(|(_, path)| path)).unwrap_or(input.trim_start_matches('/'));
    path.split('/').next().filter(|slug| !slug.is_empty()).map(ToString::to_string)
}

fn fix_url(input: &str, base: &str) -> String {
    let trimmed = input.trim().replace("https:////", "https://").replace("http:////", "http://");
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed
    } else if let Some(rest) = trimmed.strip_prefix("//") {
        format!("https://{rest}")
    } else if trimmed.starts_with('/') {
        let root = base.split("://").nth(1).and_then(|tail| tail.split('/').next()).unwrap_or(base);
        format!("https://{root}{trimmed}")
    } else {
        trimmed
    }
}

fn language_code(label: &str) -> Option<String> {
    let lower = label.to_lowercase();
    if lower.contains("english") || lower == "en-us" { Some("en".to_string()) } else { None }
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn listing(request: &Value) -> &str {
    request.get("listing").or_else(|| request.get("listingId")).and_then(Value::as_str).unwrap_or("popular")
}

fn with_listing(request: &Value, listing: &str) -> Value {
    json!({ "listing": listing, "preferences": request.get("preferences").cloned().unwrap_or(Value::Null) })
}

#[derive(Default, Deserialize)]
struct PopularResponseDto {
    page_count: u64,
    result: Vec<PopularItemDto>,
}

#[derive(Default, Deserialize)]
struct SearchResponseDto {
    result: Vec<PopularItemDto>,
    #[serde(default, rename = "maxPage")]
    max_page: u64,
}

#[derive(Default, Deserialize)]
struct RecentResponseDto {
    #[serde(default, rename = "hadNext")]
    had_next: bool,
    result: Vec<PopularItemDto>,
}

#[derive(Default, Clone, Deserialize)]
struct PopularItemDto {
    title: String,
    title_en: Option<String>,
    slug: String,
    poster: PosterDto,
}

impl PopularItemDto {
    fn into_item(self, base: &str, request: &Value) -> CatalogItem {
        let title = if pref_bool(request, "pref_use_english", false) {
            self.title_en.filter(|title| !title.is_empty()).unwrap_or(self.title)
        } else {
            self.title
        };
        CatalogItem {
            key: format!("/{}", self.slug),
            title,
            cover: Some(format!("{base}/{}", self.poster.url())),
            url: Some(format!("{base}/{}", self.slug)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Clone, Deserialize)]
struct PosterDto {
    #[serde(default, rename = "hq")]
    slug: String,
}

impl PosterDto {
    fn url(&self) -> String {
        format!("image/poster/{}.webp", self.slug)
    }
}

#[derive(Default, Deserialize)]
struct AnimeInfoDto {
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    poster: PosterDto,
    season: Option<String>,
    slug: String,
    status: String,
    synopsis: Option<String>,
    title: String,
    title_en: Option<String>,
    year: Option<u64>,
}

impl AnimeInfoDto {
    fn fallback(slug: &str) -> Self {
        Self { slug: slug.to_string(), title: slug.replace('-', " "), ..Self::default() }
    }

    fn into_item(self, base: &str, request: &Value, languages: Vec<String>) -> CatalogItem {
        let title = if pref_bool(request, "pref_use_english", false) {
            self.title_en.filter(|title| !title.is_empty()).unwrap_or(self.title)
        } else {
            self.title
        };
        let mut description = self.synopsis.unwrap_or_default();
        if !languages.is_empty() {
            if !description.is_empty() { description.push_str("\n\n"); }
            description.push_str("Available Dub Languages: ");
            description.push_str(&languages.join(", "));
        }
        if let Some(season) = self.season {
            if !description.is_empty() { description.push('\n'); }
            description.push_str("Season: ");
            description.push_str(&season);
        }
        if let Some(year) = self.year {
            if !description.is_empty() { description.push('\n'); }
            description.push_str(&format!("Year: {year}"));
        }
        CatalogItem {
            key: format!("/{}", self.slug),
            title,
            cover: Some(format!("{base}/{}", self.poster.url())),
            description: (!description.is_empty()).then_some(description),
            tags: self.genres,
            url: Some(format!("{base}/{}", self.slug)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            status: match self.status.as_str() {
                "finished_airing" => ItemStatus::Completed,
                "currently_airing" => ItemStatus::Ongoing,
                _ => ItemStatus::Unknown,
            },
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct EpisodeResponseDto {
    #[serde(default)]
    pages: Vec<Value>,
    #[serde(default)]
    result: Vec<EpisodeDto>,
}

#[derive(Deserialize)]
struct EpisodeDto {
    slug: String,
    title: Option<String>,
    episode_string: String,
}

impl EpisodeDto {
    fn into_episode(self, anime_slug: &str, lang: &str) -> Option<VideoEpisode> {
        let number = self.episode_string.parse::<f32>().ok();
        let title = if let Some(name) = self.title.filter(|title| !title.is_empty()) {
            format!("Ep. {} - {name}", self.episode_string)
        } else {
            format!("Ep. {}", self.episode_string)
        };
        Some(VideoEpisode {
            key: format!("/{anime_slug}/ep-{}-{}", self.episode_string, self.slug),
            title: Some(title),
            episode_number: number,
            url: Some(format!("{DEFAULT_BASE_URL}/{anime_slug}/ep-{}-{}", self.episode_string, self.slug)),
            language: Some(lang.to_string()),
            ..VideoEpisode::default()
        })
    }
}

#[derive(Default, Deserialize)]
struct LanguagesDto {
    #[serde(default)]
    result: Vec<String>,
}

#[derive(Default, Deserialize)]
struct ServersDto {
    #[serde(default)]
    servers: Vec<ServerDto>,
}

#[derive(Deserialize)]
struct ServerDto {
    name: String,
    src: String,
}

const POPULAR_FIXTURE: &str = r#"{"page_count":1,"result":[{"title":"Sample Anime","title_en":"Sample Anime","slug":"sample-anime","poster":{"hq":"sample"}}]}"#;
const RECENT_FIXTURE: &str = r#"{"hadNext":false,"result":[{"title":"Sample Anime","title_en":"Sample Anime","slug":"sample-anime","poster":{"hq":"sample"}}]}"#;
const SEARCH_FIXTURE: &str = r#"{"result":[{"title":"Sample Anime","title_en":"Sample Anime","slug":"sample-anime","poster":{"hq":"sample"}}],"maxPage":1}"#;
const DETAILS_FIXTURE: &str = r#"{"genres":["Action"],"poster":{"hq":"sample"},"season":"spring","slug":"sample-anime","status":"currently_airing","synopsis":"Sample synopsis.","title":"Sample Anime","title_en":"Sample Anime","year":2026}"#;
const LANGUAGES_FIXTURE: &str = r#"{"result":["ja-JP","en-US"]}"#;
const EPISODES_FIXTURE: &str = r#"{"pages":[{}],"result":[{"slug":"sample","title":"First Episode","episode_string":"1"}]}"#;
const SERVERS_FIXTURE: &str = r#"{"servers":[{"name":"VidStreaming","src":"https://vidstreaming.example/player?id=sample"}]}"#;

export_video_source!(SOURCE);
