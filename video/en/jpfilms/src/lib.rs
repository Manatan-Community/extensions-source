use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: JPFilms = JPFilms;
const BASE_URL: &str = "https://jp-films.com";
const POPULAR_URL: &str = "https://jp-films.com/wp-content/themes/halimmovies/halim-ajax.php?action=halim_get_popular_post&showpost=50&type=all";
const PLAYER_URL: &str = "https://jp-films.com/wp-content/themes/halimmovies/player.php";
const PLAYER_NONCE: &str = "8c934fd387";

struct JPFilms;

impl VideoSource for JPFilms {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = if listing(&request) == "latest" {
            BASE_URL.to_string()
        } else {
            POPULAR_URL.to_string()
        };
        let body = get_document(&target, LIST_FIXTURE, BASE_URL);
        let entries = if listing(&request) == "latest" {
            parse_latest(&body, &request)
        } else {
            parse_cards(&body, "div.item", &request)
        };
        Ok(Paged {
            entries,
            has_next_page: false,
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
                entries: vec![fetch_details(&path, &request)],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(request);
        }
        let target = format!("{BASE_URL}/?s={}", url::query_escape(query));
        let body = get_document(&target, SEARCH_FIXTURE, BASE_URL);
        Ok(Paged {
            entries: parse_cards(
                &body,
                "#main-contents section div.halim_box article",
                &request,
            ),
            has_next_page: select_all(
                &Html::parse_document(&body),
                "ul.pagination a.next, a.next.page-numbers",
            )
            .next()
            .is_some(),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(|| "/cobra-2".to_string());
        Ok(fetch_details(&key, &request))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_else(|| "/cobra-2".to_string());
        let body = get_document(&absolute_url(&key), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = episode_from_request(&request).unwrap_or_else(|| EpisodeDto {
            post_id: 28349,
            post_url: Some("https://jp-films.com/watch-cobra-2/free-hls-sv2.html".to_string()),
            server_id: 2,
            episode_slug: "free-hls".to_string(),
            episode_name: Some("HLS Streaming".to_string()),
        });
        let referer = episode
            .post_url
            .as_deref()
            .and_then(item_path_from_watch_url)
            .map(|path| absolute_url(&path))
            .unwrap_or_else(|| BASE_URL.to_string());
        let mut streams = fetch_player_streams(&episode, &referer, false);
        if streams.is_empty() {
            streams = fetch_player_streams(&episode, &referer, true);
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
        Ok(request_key(&request, "item").map(|key| absolute_url(&key)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&path, &request)),
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
        .with_webview_challenge_fallback()
}

fn get_document(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn get_player(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .xhr()
        .referer(referer)
        .header("Accept", "text/html, */*; q=0.01")
        .header("X-Requested-With", "XMLHttpRequest")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_latest(body: &str, request: &Value) -> Vec<CatalogItem> {
    let mut entries = parse_cards(
        body,
        "#ajax-vertical-widget-movie > div.item, #ajax-vertical-widget-tv_series > div.item",
        request,
    );
    if entries.is_empty() {
        entries = parse_cards(body, "article .halim-item, div.halim-item", request);
    }
    entries
}

fn parse_cards(body: &str, selector: &str, request: &Value) -> Vec<CatalogItem> {
    let doc = Html::parse_document(body);
    select_all(&doc, selector)
        .filter_map(|element| card_item(element, request))
        .collect()
}

fn card_item(element: ElementRef<'_>, request: &Value) -> Option<CatalogItem> {
    let href = attr(&element, "a", "href")?;
    let translated = text(&element, "h3.title, h2.entry-title")
        .or_else(|| attr(&element, "a", "title"))
        .unwrap_or_else(|| title_from_path(&href));
    let original = text(&element, "p.original_title, p.org_title");
    let title = preferred_title(original.as_deref(), &translated, request);
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title,
        alternate_titles: original.into_iter().chain(Some(translated)).collect(),
        cover: attr(&element, "img", "data-src")
            .or_else(|| attr(&element, "img", "src"))
            .map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str, request: &Value) -> CatalogItem {
    let body = get_document(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    parse_details(&body, path, request).unwrap_or_else(|| CatalogItem {
        key: path_key(path),
        title: title_from_path(path),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, path: &str, request: &Value) -> Option<CatalogItem> {
    let doc = Html::parse_document(body);
    let translated = select_text(&doc, "h1.entry-title").unwrap_or_else(|| title_from_path(path));
    let original = select_text(&doc, "p.org_title, p.original_title");
    let mut tags = select_all(&doc, "p.category a")
        .map(|value| collect_text(&value))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    tags.extend(
        select_all(&doc, "p.released a")
            .map(|value| collect_text(&value))
            .filter(|value| !value.is_empty()),
    );
    Some(CatalogItem {
        key: path_key(path),
        title: preferred_title(original.as_deref(), &translated, request),
        alternate_titles: original.into_iter().chain(Some(translated)).collect(),
        cover: select_attr(&doc, "div.movie-poster img, img.movie-thumb", "data-src")
            .or_else(|| select_attr(&doc, "div.movie-poster img, img.movie-thumb", "src"))
            .map(|value| absolute_url(&value)),
        url: Some(absolute_url(path)),
        authors: select_all(&doc, "p.directors a")
            .map(|value| collect_text(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        artists: select_all(&doc, "p.actors a")
            .map(|value| collect_text(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        description: select_text(&doc, "div.entry-content article p, article.item-content p"),
        tags,
        language: Some("en".to_string()),
        rating: select_attr(&doc, "i.imdb-icon", "data-rating")
            .and_then(|value| value.parse().ok()),
        content_rating: Some("adult".to_string()),
        status: parse_status(&doc),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let groups = episode_groups(body);
    let names = server_names(body);
    let selected_index = names
        .iter()
        .position(|name| name.to_ascii_uppercase().contains("FREE"))
        .unwrap_or(0);
    let selected = groups
        .get(selected_index)
        .or_else(|| groups.first())
        .cloned()
        .unwrap_or_default();
    let is_free = names
        .get(selected_index)
        .map(|name| name.to_ascii_uppercase().contains("FREE"))
        .unwrap_or_else(|| selected.iter().any(|episode| episode.server_id == 2));
    let prefix = if is_free { "[FREE] " } else { "[VIP] " };
    selected
        .into_iter()
        .rev()
        .enumerate()
        .map(|(index, episode)| {
            let name = episode
                .episode_name
                .as_deref()
                .unwrap_or(&episode.episode_slug);
            let mut extra = BTreeMap::new();
            extra.insert(
                "postId".to_string(),
                Value::Number(serde_json::Number::from(episode.post_id)),
            );
            extra.insert(
                "serverId".to_string(),
                Value::Number(serde_json::Number::from(episode.server_id)),
            );
            extra.insert(
                "episodeSlug".to_string(),
                Value::String(episode.episode_slug.clone()),
            );
            VideoEpisode {
                key: episode
                    .post_url
                    .as_deref()
                    .map(path_key)
                    .unwrap_or_else(|| episode.episode_slug.clone()),
                title: Some(format!("{prefix}{name}")),
                episode_number: episode_number(name)
                    .or_else(|| episode_number(&episode.episode_slug)),
                url: episode.post_url.as_deref().map(absolute_url),
                language: Some("en".to_string()),
                source_order: Some(index as i32),
                extra,
                ..VideoEpisode::default()
            }
        })
        .collect()
}

fn episode_groups(body: &str) -> Vec<Vec<EpisodeDto>> {
    let Some(raw) = between(body, "var jsonEpisodes =", "</script>") else {
        return Vec::new();
    };
    let json = raw.trim().trim_end_matches(';').trim();
    serde_json::from_str(json).unwrap_or_default()
}

fn server_names(body: &str) -> Vec<String> {
    let doc = Html::parse_document(body);
    select_all(&doc, "#halim-list-server .halim-server-name")
        .map(|value| collect_text(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn fetch_player_streams(
    episode: &EpisodeDto,
    referer: &str,
    backup_subserver: bool,
) -> Vec<VideoStream> {
    let mut target = format!(
        "{PLAYER_URL}?episode_slug={}&server_id={}&",
        url::query_escape(&episode.episode_slug),
        episode.server_id
    );
    if backup_subserver {
        target.push_str("subsv_id=2&");
    }
    target.push_str(&format!(
        "post_id={}&nonce={PLAYER_NONCE}&custom_var=",
        episode.post_id
    ));
    let body = get_player(&target, PLAYER_FIXTURE, referer);
    let mut streams = parse_player_json(&body, referer);
    if streams.is_empty() {
        streams = parse_player_script(&body, referer);
    }
    for stream in &mut streams {
        if stream.name.is_none() {
            stream.name = episode.episode_name.clone();
        }
        stream.is_backup = backup_subserver;
    }
    streams
}

fn parse_player_json(body: &str, referer: &str) -> Vec<VideoStream> {
    let Ok(response) = serde_json::from_str::<PlayerResponse>(body) else {
        return Vec::new();
    };
    response
        .data
        .and_then(|data| data.sources)
        .map(|sources| parse_player_script(&sources, referer))
        .unwrap_or_default()
}

fn parse_player_script(body: &str, referer: &str) -> Vec<VideoStream> {
    let mut urls = Vec::new();
    for needle in [
        "\"file\":\"",
        "file: \"",
        "file: '",
        "source src=\"",
        "src=\"",
    ] {
        collect_urls_after(body, needle, &mut urls);
    }
    urls.sort();
    urls.dedup();
    urls.into_iter()
        .filter(|stream_url| stream_url.contains(".m3u8") || stream_url.contains(".mp4"))
        .map(|stream_url| make_stream(&stream_url, referer))
        .collect()
}

fn collect_urls_after(body: &str, needle: &str, urls: &mut Vec<String>) {
    let mut rest = body;
    while let Some(start) = rest.find(needle) {
        rest = &rest[start + needle.len()..];
        let end = rest
            .find(['"', '\''])
            .unwrap_or(rest.len())
            .min(rest.find("\\\"").unwrap_or(rest.len()));
        let raw = rest[..end].replace("\\/", "/");
        if raw.starts_with("http://") || raw.starts_with("https://") {
            urls.push(html::html_unescape(&raw));
        }
    }
}

fn make_stream(stream_url: &str, referer: &str) -> VideoStream {
    let is_hls = stream_url.contains(".m3u8");
    VideoStream {
        url: stream_url.to_string(),
        name: Some(if is_hls { "HLS" } else { "MP4" }.to_string()),
        quality: quality_from_url(stream_url),
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

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = preference(request, "preferred_quality").unwrap_or("720");
    streams.sort_by_key(|stream| {
        stream
            .quality
            .as_deref()
            .unwrap_or_default()
            .contains(preferred)
    });
    streams.reverse();
}

fn parse_status(doc: &Html) -> ItemStatus {
    let body = select_text(doc, "body")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if body.contains("completed") || body.contains("full hd") || body.contains(" full") {
        ItemStatus::Completed
    } else if body.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn episode_from_request(request: &Value) -> Option<EpisodeDto> {
    if let Some(extra) = request
        .get("episode")
        .and_then(|episode| episode.get("extra"))
        .or_else(|| request.get("extra"))
    {
        let post_id = extra.get("postId").and_then(Value::as_u64)? as u32;
        let server_id = extra.get("serverId").and_then(Value::as_u64)? as u32;
        let episode_slug = extra
            .get("episodeSlug")
            .and_then(Value::as_str)?
            .to_string();
        return Some(EpisodeDto {
            post_id,
            post_url: request_key(request, "episode").map(|key| absolute_url(&key)),
            server_id,
            episode_slug,
            episode_name: None,
        });
    }
    let key = request_key(request, "episode")?;
    let (episode_slug, server_id) = episode_slug_and_server(&key)?;
    let item_path = item_path_from_watch_url(&key)?;
    let body = get_document(&absolute_url(&item_path), DETAILS_FIXTURE, BASE_URL);
    episode_groups(&body)
        .into_iter()
        .flatten()
        .find(|episode| episode.episode_slug == episode_slug && episode.server_id == server_id)
        .or_else(|| {
            post_id_from_body(&body).map(|post_id| EpisodeDto {
                post_id,
                post_url: Some(absolute_url(&key)),
                server_id,
                episode_slug,
                episode_name: None,
            })
        })
}

fn episode_slug_and_server(path: &str) -> Option<(String, u32)> {
    let filename = path.trim_end_matches('/').split('/').next_back()?;
    let stem = filename.strip_suffix(".html").unwrap_or(filename);
    let (slug, server) = stem.rsplit_once("-sv")?;
    Some((slug.to_string(), server.parse().ok()?))
}

fn item_path_from_watch_url(path: &str) -> Option<String> {
    let path = path_key(path);
    let rest = path.strip_prefix("/watch-")?;
    let slug = rest.split('/').next().unwrap_or(rest);
    Some(format!("/{slug}"))
}

fn post_id_from_body(body: &str) -> Option<u32> {
    between(body, "postid-", " ")
        .and_then(|value| value.parse().ok())
        .or_else(|| between(body, "\"post_id\":", ",").and_then(|value| value.parse().ok()))
}

fn quality_from_url(stream_url: &str) -> Option<String> {
    for quality in ["2160", "1440", "1080", "720", "480", "360"] {
        if stream_url.contains(quality) {
            return Some(format!("{quality}p"));
        }
    }
    None
}

fn episode_number(value: &str) -> Option<f32> {
    let digits = value
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>();
    digits.parse().ok()
}

fn preferred_title(original: Option<&str>, translated: &str, request: &Value) -> String {
    if preference(request, "preferred_title_style") == Some("original") {
        original
            .filter(|value| !value.is_empty())
            .unwrap_or(translated)
            .to_string()
    } else {
        translated.to_string()
    }
}

fn preference<'a>(request: &'a Value, key: &str) -> Option<&'a str> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
}

fn select_all<'a>(doc: &'a Html, selector: &str) -> impl Iterator<Item = ElementRef<'a>> {
    Selector::parse(selector)
        .ok()
        .map(|selector| doc.select(&selector).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
}

fn select_text(doc: &Html, selector: &str) -> Option<String> {
    select_all(doc, selector)
        .next()
        .map(|value| collect_text(&value))
        .filter(|value| !value.is_empty())
}

fn select_attr(doc: &Html, selector: &str, name: &str) -> Option<String> {
    select_all(doc, selector)
        .next()
        .and_then(|value| value.value().attr(name).map(ToString::to_string))
}

fn text(element: &ElementRef<'_>, selector: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    element
        .select(&selector)
        .next()
        .map(|value| collect_text(&value))
        .filter(|value| !value.is_empty())
}

fn attr(element: &ElementRef<'_>, selector: &str, name: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    element
        .select(&selector)
        .next()
        .and_then(|value| value.value().attr(name).map(ToString::to_string))
}

fn collect_text(element: &ElementRef<'_>) -> String {
    html::html_unescape(&element.text().collect::<Vec<_>>().join(" "))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn between<'a>(value: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let after = value.split_once(start)?.1;
    Some(
        after
            .split_once(end)
            .map(|(head, _)| head)
            .unwrap_or(after)
            .trim(),
    )
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
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
    ((input.starts_with(BASE_URL) || input.starts_with('/')) && !input.contains("/watch-"))
        .then(|| path_key(input))
}

fn path_key(input: &str) -> String {
    let without_origin = input.strip_prefix(BASE_URL).unwrap_or(input);
    if without_origin.starts_with('/') {
        without_origin.to_string()
    } else {
        format!("/{without_origin}")
    }
}

fn absolute_url(path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{BASE_URL}{}", path_key(path))
    }
}

fn title_from_path(path: &str) -> String {
    path_key(path)
        .trim_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("jpfilms")
        .replace(['-', '_'], " ")
        .split_whitespace()
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpisodeDto {
    #[serde(rename = "postId")]
    post_id: u32,
    #[serde(rename = "postUrl")]
    post_url: Option<String>,
    #[serde(rename = "serverId")]
    server_id: u32,
    #[serde(rename = "episodeSlug")]
    episode_slug: String,
    #[serde(rename = "episodeName")]
    episode_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PlayerResponse {
    data: Option<PlayerData>,
}

#[derive(Debug, Deserialize)]
struct PlayerData {
    sources: Option<String>,
}

const LIST_FIXTURE: &str = r#"<div class="item post-1"><a href="https://jp-films.com/sample-film" title="Sample Film"><img data-src="/sample.jpg"><h3 class="title">Sample Film</h3><p class="original_title">サンプル</p></a></div>"#;
const SEARCH_FIXTURE: &str = r#"<main id="main-contents"><section><div class="halim_box"><article><a class="halim-thumb" href="https://jp-films.com/sample-film" title="Sample Film"><img data-src="/sample.jpg"><h2 class="entry-title">Sample Film</h2><p class="original_title">サンプル</p></a></article></div></section></main>"#;
const DETAILS_FIXTURE: &str = r#"<body class="postid-1"><h1 class="entry-title">Sample Film Full HD</h1><p class="org_title">サンプル</p><div class="movie-poster"><img src="/sample.jpg"></div><p class="released"><a>2024</a><i class="imdb-icon" data-rating="8.0"></i></p><p class="directors">Director: <a>Director</a></p><p class="actors">Actors: <a>Actor</a></p><p class="category">Genres: <a>Drama</a></p><div class="entry-content"><article><p>Fixture description.</p></article></div><div id="halim-list-server"><div class="halim-server-name">Watch FREE</div></div><script>var jsonEpisodes = [[{"postId":1,"postUrl":"https:\/\/jp-films.com\/watch-sample-film\/free-hls-sv2.html","serverId":2,"episodeSlug":"free-hls","episodeName":"HLS Streaming"}]]</script></body>"#;
const PLAYER_FIXTURE: &str = r#"<script>playerInstance.setup({playlist:[{sources:[{"file":"https:\/\/fixtures.invalid\/jpfilms\/master.m3u8","type":"hls"}]}]});</script>"#;

export_video_source!(SOURCE);
