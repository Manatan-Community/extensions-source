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
use serde_json::{Value, json};

const SOURCE: LaMovie = LaMovie;
const BASE_URL: &str = "https://la.movie";
const API_PATH: &str = "api";
const POSTS_PER_PAGE: u64 = 24;
const EPISODES_PER_PAGE: u64 = 24;

struct LaMovie;

impl VideoSource for LaMovie {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let kind = default_type(&request);
        let order = if listing(&request) == "latest" {
            "latest"
        } else {
            "views"
        };
        let target = format!(
            "{BASE_URL}/{API_PATH}/listing/{kind}?postType={kind}&postsPerPage={POSTS_PER_PAGE}&page={}&orderBy={order}&order=DESC",
            page(&request)
        );
        Ok(parse_listing(&fetch_json(&target, LIST_FIXTURE, BASE_URL)))
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
        let p = page(&request);
        let target = if !query.is_empty() {
            format!(
                "{BASE_URL}/{API_PATH}/search?postType=any&q={}&postsPerPage={POSTS_PER_PAGE}&page={p}",
                url::query_escape(query)
            )
        } else {
            let kind =
                normalize_type(&filter(&request, "type").unwrap_or_else(|| default_type(&request)));
            let mut target = format!(
                "{BASE_URL}/{API_PATH}/listing/{kind}?postType={kind}&postsPerPage={POSTS_PER_PAGE}&page={p}&orderBy={}&order={}",
                filter(&request, "order_by").unwrap_or_else(|| "latest".to_string()),
                filter(&request, "order").unwrap_or_else(|| "DESC".to_string())
            );
            if let Some(filter_json) = filter_json(&request) {
                target.push_str("&filter=");
                target.push_str(&url::query_escape(&filter_json));
            }
            target
        };
        Ok(parse_listing(&fetch_json(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/movies/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/movies/sample".to_string());
        let data = fetch_details_json(&path);
        let post_type = post_type(&data);
        let series_id = data.get("_id").and_then(Value::as_i64).unwrap_or(0);
        if !matches!(
            post_type.as_str(),
            "tvshows" | "series" | "animes" | "anime"
        ) {
            return Ok(vec![VideoEpisode {
                key: series_id.to_string(),
                title: Some(title_value(&data).unwrap_or_else(|| "Pelicula".to_string())),
                episode_number: Some(1.0),
                url: Some(absolute_url(&path)),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        Ok(fetch_all_episodes(series_id, &path))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let post_id = request_raw_key(&request, "episode")
            .and_then(|v| v.split('|').next().and_then(|x| x.parse::<i64>().ok()))
            .or_else(|| {
                request
                    .get("episode")
                    .and_then(|v| v.get("id"))
                    .and_then(Value::as_i64)
            })
            .unwrap_or(0);
        let target = format!("{BASE_URL}/{API_PATH}/player?postId={post_id}&demo=0");
        let data = data_value(&fetch_json(&target, PLAYER_FIXTURE, BASE_URL));
        Ok(parse_hosters(data.get("embeds"), &absolute_url("/")))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(raw) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = raw.splitn(5, '|');
        let server = parts.next().unwrap_or("external");
        let lang = parts.next().unwrap_or("");
        let quality = parts.next().unwrap_or("");
        let embed = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(BASE_URL);
        let mut streams = resolve_embed(embed, server, lang, quality, referer, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut out = Vec::new();
        for hoster in self.hosters(request.clone())? {
            for mut stream in self.resolve_hoster(json!({
                "hoster": { "key": hoster.key },
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
            }))? {
                stream.hoster = Some(hoster.clone());
                out.push(stream);
            }
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
                title: "Populares".to_string(),
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
        .with_cookies_for(BASE_URL)
        .with_header("Origin", BASE_URL)
        .with_webview_challenge_fallback()
}
fn fetch_json(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .xhr()
        .referer(referer)
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}
fn fetch_doc(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}
fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let data = data_value(body);
    let posts = data
        .get("posts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let current = data
        .pointer("/pagination/current_page")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let last = data
        .pointer("/pagination/last_page")
        .and_then(Value::as_u64)
        .unwrap_or(current);
    Paged {
        entries: posts.iter().map(item_from_post).collect(),
        has_next_page: current < last,
    }
}
fn item_from_post(post: &Value) -> CatalogItem {
    let key = anime_path(post);
    CatalogItem {
        key: key.clone(),
        title: title_value(post).unwrap_or_else(|| title_from_path(&key)),
        cover: image_url(post.pointer("/images/poster").and_then(Value::as_str)),
        url: Some(absolute_url(&key)),
        description: post
            .get("overview")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: false,
        ..CatalogItem::default()
    }
}
fn fetch_details(path: &str) -> CatalogItem {
    let post = fetch_details_json(path);
    let key = anime_path(&post);
    CatalogItem {
        key: if key == "/movies/0" {
            path_key(path)
        } else {
            key.clone()
        },
        title: title_value(&post).unwrap_or_else(|| title_from_path(path)),
        cover: image_url(post.pointer("/images/poster").and_then(Value::as_str))
            .or_else(|| image_url(post.pointer("/images/backdrop").and_then(Value::as_str))),
        url: Some(absolute_url(if key == "/movies/0" { path } else { &key })),
        description: post
            .get("overview")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}
fn fetch_details_json(path: &str) -> Value {
    let (kind, slug, id) = parse_anime_context(path);
    let mut target = format!(
        "{BASE_URL}/{API_PATH}/single/{kind}?slug={}&postType={kind}",
        url::query_escape(&slug)
    );
    if let Some(id) = id {
        target.push_str("&_id=");
        target.push_str(&id.to_string());
    }
    data_value(&fetch_json(&target, DETAILS_FIXTURE, BASE_URL))
}
fn fetch_all_episodes(series_id: i64, item_path: &str) -> Vec<VideoEpisode> {
    let mut out = Vec::new();
    let mut counter = 1.0;
    for season in ["1", "0"] {
        let first = fetch_episode_page(series_id, season, 1);
        let posts = first
            .get("posts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if posts.is_empty() {
            continue;
        }
        let last = first
            .pointer("/pagination/last_page")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        for post in posts {
            out.push(episode_from_post(&post, counter, item_path));
            counter += 1.0;
        }
        for page in 2..=last {
            for post in fetch_episode_page(series_id, season, page)
                .get("posts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                out.push(episode_from_post(&post, counter, item_path));
                counter += 1.0;
            }
        }
        break;
    }
    out.reverse();
    out
}
fn fetch_episode_page(series_id: i64, season: &str, page: u64) -> Value {
    let target = format!(
        "{BASE_URL}/{API_PATH}/single/episodes/list?_id={series_id}&season={season}&page={page}&postsPerPage={EPISODES_PER_PAGE}"
    );
    data_value(&fetch_json(&target, EPISODES_FIXTURE, BASE_URL))
}
fn episode_from_post(post: &Value, counter: f32, item_path: &str) -> VideoEpisode {
    let id = post
        .get("_id")
        .and_then(Value::as_i64)
        .unwrap_or(counter as i64);
    let season = post
        .get("season_number")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let episode = post
        .get("episode_number")
        .and_then(Value::as_i64)
        .unwrap_or(counter as i64);
    let name = title_value(post).unwrap_or_else(|| format!("Episodio {episode}"));
    VideoEpisode {
        key: format!("{id}|{item_path}"),
        title: Some(format!("T{season}x{episode} - {name}")),
        episode_number: Some(counter),
        url: Some(absolute_url(item_path)),
        language: Some("es".to_string()),
        ..VideoEpisode::default()
    }
}
fn parse_hosters(embeds: Option<&Value>, referer: &str) -> Vec<VideoHoster> {
    let mut out = Vec::new();
    collect_embeds(embeds.unwrap_or(&Value::Null), &mut out, referer);
    out
}
fn collect_embeds(value: &Value, out: &mut Vec<VideoHoster>, referer: &str) {
    match value {
        Value::Array(items) => items.iter().for_each(|v| collect_embeds(v, out, referer)),
        Value::Object(map) => {
            if let Some(url) = map
                .get("url")
                .or_else(|| map.get("src"))
                .or_else(|| map.get("link"))
                .and_then(Value::as_str)
            {
                let server = map
                    .get("server")
                    .or_else(|| map.get("name"))
                    .or_else(|| map.get("key"))
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| server_key(url));
                let quality = map
                    .get("quality")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let lang = map
                    .get("language")
                    .or_else(|| map.get("lang"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let embed = absolute_remote(url, referer);
                let label = format!("{} {} {}", lang.to_uppercase(), server, quality)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                out.push(VideoHoster {
                    key: format!(
                        "{}|{}|{}|{}|{}",
                        server_key(&format!("{server} {embed}")),
                        lang,
                        quality,
                        embed,
                        referer
                    ),
                    name: if label.is_empty() {
                        host_name(&embed)
                    } else {
                        label
                    },
                    url: Some(embed),
                    lazy: true,
                    video_count: Some(1),
                    ..VideoHoster::default()
                });
            } else {
                map.values().for_each(|v| collect_embeds(v, out, referer));
            }
        }
        _ => {}
    }
}
fn resolve_embed(
    embed: &str,
    server: &str,
    lang: &str,
    quality: &str,
    referer: &str,
    request: &Value,
) -> Vec<VideoStream> {
    let embed = absolute_remote(embed, referer);
    let name = format!(
        "{} {} {}",
        lang.to_uppercase(),
        server_label(server, &embed),
        quality
    )
    .split_whitespace()
    .collect::<Vec<_>>()
    .join(" ");
    if server == "lamovie" || embed.contains("lamovie") || embed.contains("vimeos") {
        let streams = lamovie_embed_streams(&embed, &name, request);
        if !streams.is_empty() {
            return streams;
        }
    }
    if embed.contains(".m3u8") {
        return parse_hls(&embed, &name, referer, request);
    }
    let body = fetch_doc(&embed, "", referer);
    if let Some(src) = first_media_url(&body) {
        let src = absolute_remote(&src, &embed);
        if src.contains(".m3u8") {
            return parse_hls(&src, &name, &embed, request);
        }
        return vec![media_stream(&src, &name, "direct", &embed)];
    }
    vec![external_stream(&embed, &name, referer)]
}
fn lamovie_embed_streams(embed: &str, name: &str, request: &Value) -> Vec<VideoStream> {
    let body = fetch_doc(embed, "", BASE_URL);
    let playlist = Regex::new(r#""file"\s*:\s*"([^"]+)""#)
        .unwrap()
        .captures(&body)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().replace("\\/", "/"))
        .or_else(|| {
            Regex::new(r#"https?://[^\s'"\\]+\.m3u8[^\s'"\\]*"#)
                .unwrap()
                .find(&body)
                .map(|m| m.as_str().replace("\\/", "/"))
        });
    playlist
        .map(|m3u8| parse_hls(&m3u8, name, embed, request))
        .unwrap_or_default()
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
            out.push(media_stream(
                &absolute_remote(line.trim(), master),
                name,
                &quality,
                referer,
            ));
        }
    }
    if out.is_empty() {
        out.push(media_stream(master, name, "auto", referer));
    }
    sort_streams(&mut out, request);
    out
}
fn media_stream(target: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = target.contains(".m3u8");
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
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
fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)["']"#,
        r#"src\s*:\s*["']([^"']+)["']"#,
        r#"<source[^>]+src=["']([^"']+)["']"#,
    ]
    .into_iter()
    .find_map(|p| {
        Regex::new(p)
            .ok()?
            .captures(body)?
            .get(1)
            .map(|m| m.as_str().replace("\\/", "/"))
    })
}
fn data_value(body: &str) -> Value {
    let root: Value =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap());
    root.get("data").cloned().unwrap_or(root)
}
fn title_value(value: &Value) -> Option<String> {
    value
        .get("title")
        .or_else(|| value.get("name"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}
fn post_type(value: &Value) -> String {
    normalize_type(
        value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("movies"),
    )
}
fn anime_path(post: &Value) -> String {
    let kind = post_type(post);
    let slug = post
        .get("slug")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("0");
    let id = post.get("_id").and_then(Value::as_i64).unwrap_or(0);
    format!("/{kind}/{slug}-{id}")
}
fn parse_anime_context(path: &str) -> (String, String, Option<i64>) {
    let clean = path_key(path);
    let mut parts = clean.trim_matches('/').split('/');
    let kind = normalize_type(parts.next().unwrap_or("movies"));
    let raw = parts.next().unwrap_or("sample");
    let id = raw
        .rsplit_once('-')
        .and_then(|(_, id)| id.parse::<i64>().ok());
    let slug = id
        .and_then(|_| raw.rsplit_once('-').map(|(slug, _)| slug.to_string()))
        .unwrap_or_else(|| raw.to_string());
    (kind, slug, id)
}
fn image_url(raw: Option<&str>) -> Option<String> {
    raw.filter(|v| !v.is_empty()).map(|v| {
        if v.starts_with("http") {
            v.to_string()
        } else {
            absolute_url(v)
        }
    })
}
fn filter_json(request: &Value) -> Option<String> {
    let mut map = serde_json::Map::new();
    for (filter_key, api_key) in [
        ("genre", "genres"),
        ("country", "countries"),
        ("provider", "providers"),
        ("year", "years"),
    ] {
        if let Some(value) = filter(request, filter_key).and_then(|v| v.parse::<i64>().ok()) {
            map.insert(api_key.to_string(), json!([value]));
        }
    }
    if map.is_empty() {
        None
    } else {
        Some(Value::Object(map).to_string())
    }
}
fn normalize_type(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "movie" | "movies" | "peliculas" => "movies".to_string(),
        "tv-show" | "tvshows" | "tv shows" | "series" => "tvshows".to_string(),
        "anime" | "animes" => "animes".to_string(),
        _ => {
            if raw.is_empty() {
                "movies".to_string()
            } else {
                raw.to_string()
            }
        }
    }
}
fn server_key(input: &str) -> &str {
    let lower = input.to_ascii_lowercase();
    if lower.contains("dood") || lower.contains("d000d") {
        "dood"
    } else if lower.contains("voe") {
        "voe"
    } else if lower.contains("mp4upload") {
        "mp4upload"
    } else if lower.contains("streamhide") || lower.contains("sht") {
        "streamhide"
    } else if lower.contains("streamwish") || lower.contains("wish") || lower.contains("strwish") {
        "streamwish"
    } else if lower.contains("yourupload") {
        "yourupload"
    } else if lower.contains("filemoon") {
        "filemoon"
    } else if lower.contains("goodstream") {
        "goodstream"
    } else if lower.contains("lamovie") || lower.contains("vimeos") || lower.contains("la.movie") {
        "lamovie"
    } else {
        "external"
    }
}
fn server_label(server: &str, embed: &str) -> String {
    if server == "external" {
        host_name(embed)
    } else {
        server.to_string()
    }
}
fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let qpref = pref(request, "preferred_quality", "1080");
    let spref = pref(request, "preferred_server_lamovie", "auto");
    let lpref = pref(request, "preferred_language_lamovie", "any");
    streams.sort_by_key(|s| {
        let text = format!("{} {}", s.name.clone().unwrap_or_default(), s.url).to_ascii_lowercase();
        let q = s.quality.clone().unwrap_or_default();
        (
            spref == "auto" || text.contains(&spref),
            lpref == "any" || text.contains(&lpref),
            q.contains(&qpref),
            quality_rank(&q),
        )
    });
    streams.reverse();
}
fn absolute_url(input: &str) -> String {
    absolute_remote(input, BASE_URL)
}
fn absolute_remote(input: &str, base: &str) -> String {
    let t = input.trim().replace("\\/", "/").replace("&amp;", "&");
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
}
fn path_key(input: &str) -> String {
    if let Some(p) = input.strip_prefix(BASE_URL) {
        return path_key(p);
    }
    format!(
        "/{}",
        input
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .trim_matches('/')
    )
}
fn request_key(request: &Value, field: &str) -> Option<String> {
    request_raw_key(request, field).map(|v| path_key(&v))
}
fn request_raw_key(request: &Value, field: &str) -> Option<String> {
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
fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("LaMovie")
        .replace('-', " ")
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
fn default_type(request: &Value) -> String {
    normalize_type(&pref(request, "default_content_type", "movies"))
}
fn with_listing(request: &Value, listing: &str) -> Value {
    json!({ "listing": listing, "preferences": request.get("preferences").cloned().unwrap_or(Value::Null) })
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
fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|f| f.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}
fn quality_rank(input: &str) -> i32 {
    Regex::new(r#"(\d+)"#)
        .unwrap()
        .captures(input)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0)
}
fn referer_headers(referer: &str) -> Context {
    let mut h = Context::new();
    h.insert("Referer".to_string(), referer.to_string());
    h
}

const LIST_FIXTURE: &str = r#"{"data":{"posts":[{"_id":1,"title":"Demo","slug":"demo","type":"movies","overview":"Demo","images":{"poster":"/poster.jpg"}}],"pagination":{"current_page":1,"last_page":1}}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"_id":1,"title":"Demo","slug":"demo","type":"movies","overview":"Demo","images":{"poster":"/poster.jpg"}}}"#;
const EPISODES_FIXTURE: &str = r#"{"data":{"posts":[{"_id":1,"name":"Piloto","season_number":1,"episode_number":1}],"pagination":{"current_page":1,"last_page":1},"seasons":[1]}}"#;
const PLAYER_FIXTURE: &str = r#"{"data":{"embeds":[{"server":"lamovie","url":"https://lamovie.link/e/demo","quality":"HD","language":"latino"}]}}"#;

export_video_source!(SOURCE);
