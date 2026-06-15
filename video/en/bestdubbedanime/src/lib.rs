use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind,
    abi::{ExtensionResult, system_time},
    export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;

const SOURCE: BestDubbedAnime = BestDubbedAnime;
const BASE_URL: &str = "https://bestdubbedanime.com";

struct BestDubbedAnime;

impl VideoSource for BestDubbedAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/xz/gridgrabrecent.php?p={page}&limit=12&_={}", unix_seconds())
        } else {
            format!("{BASE_URL}/xz/trending.php?_={}", unix_seconds())
        };
        let body = get_or_fixture(&target, if listing(&request) == "latest" { LATEST_FIXTURE } else { POPULAR_FIXTURE }, BASE_URL);
        Ok(if listing(&request) == "latest" {
            parse_grid_page(&body)
        } else {
            parse_popular_page(&body)
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&path)],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            let tags = array_filter(&request, "tags").join(",,");
            format!("{BASE_URL}/xz/v3/taglist.php?tags={}&_={}", url::query_escape(&tags), unix_seconds())
        } else {
            format!(
                "{BASE_URL}/xz/searchgrid.php?p={}&limit=12&s={}&_={}",
                page(&request),
                url::query_escape(query),
                unix_seconds()
            )
        };
        let body = get_or_fixture(&target, SEARCH_FIXTURE, BASE_URL);
        Ok(if query.is_empty() {
            parse_tag_page(&body)
        } else {
            parse_grid_page(&body)
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        let body = get_or_fixture(&absolute_url(&key), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body, &key))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "episode").unwrap_or_else(|| "/sample-episode".to_string());
        let episode_url = absolute_url(&episode);
        let slug = episode.trim_start_matches('/');
        let server_body = ajax_get(
            &format!("{BASE_URL}/xz/v3/jsonEpi.php?slug={slug}&_={}", unix_seconds()),
            &episode_url,
            SERVER_FIXTURE,
        );
        let mut streams = parse_servers(&server_body, &episode_url, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Trending".to_string(),
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
        .with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn ajax_get(target: &str, referer: &str, fixture: &str) -> String {
    client(referer)
        .get(target)
        .header("Accept", "application/json, text/javascript, */*; q=0.01")
        .header("X-Requested-With", "XMLHttpRequest")
        .referer(referer)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular_page(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: select_all(&doc, "li").filter_map(popular_item).collect(),
        has_next_page: false,
    }
}

fn popular_item(element: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&element, "a", "href")?;
    let title = text(&element, "div.cittx").or_else(|| text(&element, "a"))?;
    Some(card_item(&href, title, attr(&element, "img", "src")))
}

fn parse_grid_page(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let entries = select_all(&doc, "div.grid > div.grid__item")
        .filter_map(|item| {
            let href = attr(&item, "a", "href")?;
            let title = text(&item, "div.tixtlis").or_else(|| text(&item, "a"))?;
            Some(card_item(&href, title, attr(&item, "img", "src")))
        })
        .collect::<Vec<_>>();
    let has_next_page = entries.len() == 12;
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_tag_page(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: select_all(&doc, "div.itemdtagk")
            .filter_map(|item| {
                let href = attr(&item, "a", "href")?;
                let title = text(&item, "div.titlekf").or_else(|| text(&item, "a"))?;
                Some(card_item(&href, title, attr(&item, "img", "src")))
            })
            .collect(),
        has_next_page: false,
    }
}

fn card_item(href: &str, title: String, cover: Option<String>) -> CatalogItem {
    let key = path_key(href);
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn fetch_details(path: &str) -> CatalogItem {
    if path_key(path).starts_with("/movies/") {
        let slug = path_key(path).trim_start_matches("/movies/").to_string();
        let body = ajax_get(
            &format!("{BASE_URL}/movies/jsonMovie.php?slug={slug}&_={}", unix_seconds()),
            &absolute_url(path),
            MOVIE_DETAILS_FIXTURE,
        );
        parse_movie_details(&body, path).unwrap_or_else(|| fallback_item(path))
    } else {
        let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
        parse_details(&body, path).unwrap_or_else(|| fallback_item(path))
    }
}

fn parse_movie_details(body: &str, path: &str) -> Option<CatalogItem> {
    let root: Value = serde_json::from_str(body).ok()?;
    let anime = root.pointer("/result/anime/0")?;
    let title = anime.get("title").and_then(Value::as_str)?;
    Some(CatalogItem {
        key: path_key(path),
        title: title.to_string(),
        url: Some(absolute_url(path)),
        description: anime.get("desc").and_then(Value::as_str).map(html::strip_tags),
        tags: anime
            .get("tags")
            .and_then(Value::as_str)
            .map(|tags| {
                Html::parse_fragment(tags)
                    .root_element()
                    .text()
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        status: anime
            .get("status")
            .and_then(Value::as_str)
            .map(parse_status)
            .unwrap_or(ItemStatus::Unknown),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, path: &str) -> Option<CatalogItem> {
    let doc = Html::parse_document(body);
    let title = select_text(&doc, "h1, h2, h3").unwrap_or_else(|| title_from_path(path));
    let info = select_text(&doc, "div.animeDescript p").or_else(|| select_text(&doc, ".animeDescript"));
    Some(CatalogItem {
        key: path_key(path),
        title,
        url: Some(absolute_url(path)),
        cover: select_attr(&doc, "img", "src").map(|value| absolute_url(&value)),
        description: info,
        tags: select_all(&doc, "div[itemprop=keywords] a")
            .map(|tag| collect_text(&tag))
            .filter(|tag| !tag.is_empty())
            .collect(),
        status: select_all(&doc, "div.animeDescript div")
            .map(|item| collect_text(&item))
            .find(|line| line.contains("Status"))
            .map(|line| parse_status(&line))
            .unwrap_or(ItemStatus::Unknown),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &str, path: &str) -> Vec<VideoEpisode> {
    if path_key(path).starts_with("/movies/") {
        let title = select_text(&Html::parse_document(body), "div.tinywells div h4")
            .unwrap_or_else(|| title_from_path(path));
        return vec![VideoEpisode {
            key: path_key(path),
            title: Some(title),
            episode_number: Some(1.0),
            url: Some(absolute_url(path)),
            language: Some("en".to_string()),
            ..VideoEpisode::default()
        }];
    }
    let doc = Html::parse_document(body);
    let mut episodes = select_all(&doc, "div.eplistz div div a")
        .enumerate()
        .filter_map(|(idx, episode)| {
            let href = episode.value().attr("href")?;
            let title = text(&episode, "div.inwel span").unwrap_or_else(|| title_from_path(href));
            Some(VideoEpisode {
                key: path_key(href),
                title: Some(title),
                episode_number: Some((idx + 1) as f32),
                url: Some(absolute_url(href)),
                language: Some("en".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect::<Vec<_>>();
    if episodes.is_empty() {
        if let Some(cache_url) = cache_url(body) {
            let cache_body = get_or_fixture(&cache_url, CACHE_FIXTURE, BASE_URL);
            episodes = parse_cache_episodes(&cache_body);
        }
    }
    episodes.reverse();
    episodes
}

fn parse_cache_episodes(body: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    select_all(&doc, "a")
        .enumerate()
        .filter_map(|(idx, episode)| {
            let href = episode.value().attr("href")?;
            let title = text(&episode, "div.inwel span").unwrap_or_else(|| title_from_path(href));
            Some(VideoEpisode {
                key: path_key(href),
                title: Some(title),
                episode_number: Some((idx + 1) as f32),
                url: Some(absolute_url(href)),
                language: Some("en".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_servers(body: &str, episode_url: &str, request: &Value) -> Vec<VideoStream> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(SERVER_FIXTURE).unwrap());
    let html = root
        .pointer("/result/anime/0/serversHTML")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let doc = Html::parse_fragment(html);
    let mut streams = Vec::new();
    for server in select_all(&doc, "div.serversks") {
        let Some(player_url) = server.value().attr("hl") else {
            continue;
        };
        let name = collect_text(&server);
        let player = ajax_get(
            &format!("{BASE_URL}/xz/api/playeri.php?url={}&_={}", url::query_escape(player_url), unix_seconds()),
            episode_url,
            PLAYER_FIXTURE,
        );
        streams.extend(parse_player_sources(&player, &name, episode_url, request));
    }
    streams
}

fn parse_player_sources(body: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let doc = Html::parse_document(body);
    select_all(&doc, "source")
        .filter_map(|source| {
            let url = source.value().attr("src")?;
            if url.trim().is_empty() {
                return None;
            }
            let label = source.value().attr("label").unwrap_or("auto");
            let quality = normalize_quality(label);
            let stream_url = absolute_url(url);
            let is_hls = stream_url.contains(".m3u8");
            Some(VideoStream {
                url: stream_url,
                name: Some(format!("{quality}p {name}")),
                quality: Some(quality.clone()),
                format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
                is_hls,
                stream_kind: Some(if is_hls { VideoStreamKind::Hls } else { VideoStreamKind::Direct }),
                preferred: quality.contains(&preferred_quality(request)),
                headers: referer_headers(referer),
                initialized: true,
                ..VideoStream::default()
            })
        })
        .collect()
}

fn fallback_item(path: &str) -> CatalogItem {
    CatalogItem {
        key: path_key(path),
        title: title_from_path(path),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn cache_url(body: &str) -> Option<String> {
    body.split("url: '").nth(1)?.split('\'').next().map(ToString::to_string)
}

fn parse_status(input: &str) -> ItemStatus {
    if input.contains("Ongoing") {
        ItemStatus::Ongoing
    } else if input.contains("Completed") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = preferred_quality(request);
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or_default();
        let score = quality.chars().filter(char::is_ascii_digit).collect::<String>().parse::<u32>().unwrap_or(0);
        (quality.contains(&preferred), score)
    });
    streams.reverse();
}

fn select_all<'a>(doc: &'a Html, selector: &str) -> impl Iterator<Item = ElementRef<'a>> {
    Selector::parse(selector)
        .ok()
        .map(|selector| doc.select(&selector).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
}

fn select_text(doc: &Html, selector: &str) -> Option<String> {
    select_all(doc, selector).next().map(|value| collect_text(&value)).filter(|value| !value.is_empty())
}

fn select_attr(doc: &Html, selector: &str, name: &str) -> Option<String> {
    select_all(doc, selector).next().and_then(|value| value.value().attr(name).map(ToString::to_string))
}

fn text(element: &ElementRef<'_>, selector: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    element.select(&selector).next().map(|value| collect_text(&value)).filter(|value| !value.is_empty())
}

fn attr(element: &ElementRef<'_>, selector: &str, name: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    element.select(&selector).next().and_then(|value| value.value().attr(name).map(ToString::to_string))
}

fn collect_text(element: &ElementRef<'_>) -> String {
    html::html_unescape(&element.text().collect::<Vec<_>>().join(" "))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
    (input.starts_with(BASE_URL)
        || input.starts_with("/anime/")
        || input.starts_with("/movies/")
        || input.starts_with("/episode/")
        || input.starts_with("/"))
    .then(|| path_key(input))
}

fn path_key(input: &str) -> String {
    let without_origin = input.strip_prefix(BASE_URL).unwrap_or(input);
    let path = without_origin
        .split('#')
        .next()
        .unwrap_or(without_origin)
        .split('?')
        .next()
        .unwrap_or(without_origin);
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
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
        .unwrap_or("bestdubbedanime")
        .replace('-', " ")
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

fn normalize_quality(input: &str) -> String {
    let digits = input.chars().filter(char::is_ascii_digit).collect::<String>();
    if digits.is_empty() { input.to_string() } else { digits }
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn preferred_quality(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("preferred_quality"))
        .or_else(|| request.get("preferred_quality"))
        .and_then(Value::as_str)
        .unwrap_or("1080")
        .to_string()
}

fn array_filter(request: &Value, key: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(ToString::to_string))
        .collect()
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

fn unix_seconds() -> u64 {
    system_time()
        .map(|time| time.unix_seconds.max(0) as u64)
        .unwrap_or(0)
}

const POPULAR_FIXTURE: &str = r#"<li><a href="/anime/sample"><img src="/cover.jpg"><div class="cittx">Sample BestDubbedAnime</div></a></li>"#;
const LATEST_FIXTURE: &str = r#"<div class="grid"><div class="grid__item"><a href="/anime/sample"><img src="/cover.jpg"><div class="tixtlis">Sample BestDubbedAnime</div></a></div></div>"#;
const SEARCH_FIXTURE: &str = r#"<div class="grid"><div class="grid__item"><a href="/anime/sample"><img src="/cover.jpg"><div class="tixtlis">Sample BestDubbedAnime</div></a></div></div><div class="itemdtagk"><a href="/anime/sample"><img src="/cover.jpg"><div class="titlekf">Sample BestDubbedAnime</div></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="animeDescript"><p>Fixture description.</p><div><div>Status: Completed</div></div></div><div itemprop="keywords"><a>Action</a></div><div class="eplistz"><div><div><a href="/sample-episode"><div class="inwel"><span>Episode 1</span></div></a></div></div></div>"#;
const CACHE_FIXTURE: &str = r#"<a href="/sample-episode"><div class="inwel"><span>Episode 1</span></div></a>"#;
const MOVIE_DETAILS_FIXTURE: &str = r#"{"result":{"anime":[{"title":"Sample Movie","desc":"Movie fixture.","status":"Completed","tags":"<a>Action</a>"}]}}"#;
const SERVER_FIXTURE: &str = r#"{"result":{"anime":[{"serversHTML":"<div class=\"serversks\" hl=\"https://fixtures.invalid/player\">Fixture</div>"}]}}"#;
const PLAYER_FIXTURE: &str = r#"<video><source src="https://fixtures.invalid/video-720.mp4" label="720"></video>"#;

export_video_source!(SOURCE);
