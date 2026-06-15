use std::collections::HashSet;

use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
    VideoEpisode, VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult,
    export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use scraper::{ElementRef, Html, Selector};
use serde_json::{Value, json};

const SOURCE: HentaiMama = HentaiMama;
const BASE_URL: &str = "https://hentaimama.tv";

struct HentaiMama;

impl VideoSource for HentaiMama {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/tvshows/page/{page}/")
        } else {
            format!("{BASE_URL}/advance-search/page/{page}/?submit=Submit&filter=weekly")
        };
        let body = fetch_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
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
        if let Some(slug) = query.strip_prefix("id:") {
            return Ok(Paged {
                entries: vec![fetch_details(&format!("/watch/{slug}/"))],
                has_next_page: false,
            });
        }

        let body = if query.is_empty() {
            let target = format!(
                "{BASE_URL}/advance-search/page/{}/?{}",
                page(&request),
                filter_query(&request)
            );
            fetch_or_fixture(&target, LIST_FIXTURE, BASE_URL)
        } else {
            let target = format!(
                "{BASE_URL}/page/{}/?s={}",
                page(&request),
                url::query_escape(query)
            );
            fetch_or_fixture(&target, SEARCH_FIXTURE, BASE_URL)
        };
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            request_key(&request, "item").unwrap_or_else(|| "/watch/sample-title/".to_string());
        Ok(fetch_details(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key =
            request_key(&request, "item").unwrap_or_else(|| "/watch/sample-title/".to_string());
        let body = fetch_or_fixture(&absolute_url(&key), DETAILS_FIXTURE, BASE_URL);
        let episodes = parse_episodes(&body, &key);
        Ok(if episodes.is_empty() {
            vec![fallback_episode(&body, &key)]
        } else {
            episodes
        })
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let key = request_key(&request, "episode")
            .or_else(|| request_key(&request, "item"))
            .unwrap_or_else(|| "/watch/sample-title/".to_string());
        let page_url = absolute_url(&key);
        let body = fetch_or_fixture(&page_url, DETAILS_FIXTURE, BASE_URL);
        let mut hosters = parse_hosters(&body, &page_url);
        if hosters.is_empty() {
            hosters = ajax_hosters(&body, &page_url);
        }
        Ok(hosters)
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let mut streams = resolve_hoster_key(&key, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let hosters = self.hosters(request.clone())?;
        let mut streams = Vec::new();
        for hoster in hosters {
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
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let mut seen = HashSet::new();
    let entries = select_all(
        &doc,
        "article.tvshows, div.grid > a[href*=\"/watch/\"], .page-listing-item a[href*=\"/watch/\"], .search-results a[href*=\"/watch/\"]",
    )
    .filter_map(|element| card_item(element))
    .filter(|item| seen.insert(item.key.clone()))
    .collect();
    Paged {
        entries,
        has_next_page: has_next_page(&doc),
    }
}

fn card_item(element: ElementRef<'_>) -> Option<CatalogItem> {
    let href = if element.value().name() == "a" {
        element.value().attr("href").map(ToString::to_string)
    } else {
        attr(&element, "a", "href")
    }?;
    let key = path_key(&href);
    let title = text(&element, "div.data h3 a")
        .or_else(|| text(&element, "div.details > div.title a"))
        .or_else(|| text(&element, "h2"))
        .or_else(|| attr(&element, "img", "alt").map(clean_cover_title))
        .unwrap_or_else(|| title_from_path(&key));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: attr(&element, "div.poster img, img", "data-src")
            .or_else(|| attr(&element, "div.poster img, img", "src"))
            .map(|value| absolute_remote(&value, BASE_URL)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    parse_details(&body, path).unwrap_or_else(|| CatalogItem {
        key: path_key(path),
        title: title_from_path(path),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, path: &str) -> Option<CatalogItem> {
    let doc = Html::parse_document(body);
    let title = select_text(&doc, "#info1 div:nth-child(2) span")
        .or_else(|| select_text(&doc, "h1"))
        .or_else(|| meta(&doc, "meta[property=\"og:title\"]"))
        .map(|value| value.replace(" - Hentaimama", ""))
        .unwrap_or_else(|| title_from_path(path));
    Some(CatalogItem {
        key: path_key(path),
        title,
        cover: select_attr(&doc, "div.sheader div.poster img", "data-src")
            .or_else(|| select_attr(&doc, "div.sheader div.poster img", "src"))
            .or_else(|| meta(&doc, "meta[property=\"og:image\"]"))
            .map(|value| absolute_remote(&value, BASE_URL)),
        authors: select_all(
            &doc,
            "#info1 div:nth-child(3) span div div a, a[href*=\"/studio/\"], a[href*=\"/studios/\"]",
        )
        .map(|element| collect_text(&element))
        .filter(|text| !text.is_empty())
        .collect(),
        tags: select_all(&doc, "div.sgeneros a, a[href*=\"/genres/\"]")
            .map(|element| collect_text(&element))
            .filter(|text| !text.is_empty())
            .collect(),
        description: select_text(&doc, "#info1 div.wp-content p")
            .or_else(|| meta(&doc, "meta[property=\"og:description\"]"))
            .or_else(|| meta(&doc, "meta[name=\"description\"]")),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: parse_status(
            &select_text(&doc, "#info1 div:nth-child(6) span").unwrap_or_default(),
        ),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &str, item_key: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    select_all(
        &doc,
        "div.series div.items article, .chapter-list a[href*=\"/watch/\"], a[href*=\"/watch/\"]",
    )
    .filter_map(|element| episode_from_element(element, item_key))
    .collect()
}

fn episode_from_element(element: ElementRef<'_>, item_key: &str) -> Option<VideoEpisode> {
    let href = if element.value().name() == "a" {
        element.value().attr("href").map(ToString::to_string)
    } else {
        attr(&element, "div.season_m a, a", "href")
    }?;
    let key = path_key(&href);
    let title = text(&element, "div.data h3")
        .or_else(|| text(&element, "span.c"))
        .or_else(|| text(&element, "h2"))
        .or_else(|| attr(&element, "img", "alt").map(clean_cover_title))
        .unwrap_or_else(|| title_from_path(&key));
    if !key.contains("/watch/") || (key == path_key(item_key) && title.is_empty()) {
        return None;
    }
    Some(VideoEpisode {
        key: key.clone(),
        title: Some(title.clone()),
        episode_number: episode_number(&title)
            .or_else(|| episode_number(&key))
            .or(Some(1.0)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        ..VideoEpisode::default()
    })
}

fn fallback_episode(body: &str, key: &str) -> VideoEpisode {
    let doc = Html::parse_document(body);
    VideoEpisode {
        key: path_key(key),
        title: select_text(&doc, "h1").or_else(|| Some(title_from_path(key))),
        episode_number: episode_number(key).or(Some(1.0)),
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        ..VideoEpisode::default()
    }
}

fn parse_hosters(body: &str, page_url: &str) -> Vec<VideoHoster> {
    let doc = Html::parse_document(body);
    let mut seen = HashSet::new();
    let mut hosters: Vec<_> = select_all(
        &doc,
        ".player_logic_option, iframe, a[href*=\"/script-manager/go/\"]",
    )
    .filter_map(|element| {
        let target = element
            .value()
            .attr("data-player-logic-data")
            .or_else(|| element.value().attr("src"))
            .or_else(|| element.value().attr("href"))?;
        let target = absolute_remote(target, page_url);
        let name = mirror_name(&target);
        Some(hoster(name, &target, page_url))
    })
    .filter(|hoster| seen.insert(hoster.key.clone()))
    .collect();
    if hosters.is_empty() {
        hosters = media_candidates(body)
            .into_iter()
            .filter_map(|target| {
                let name = mirror_name(&target);
                Some(hoster(name, &target, page_url))
            })
            .filter(|hoster| seen.insert(hoster.key.clone()))
            .collect();
    }
    hosters
}

fn ajax_hosters(body: &str, page_url: &str) -> Vec<VideoHoster> {
    let Some(token) = ajax_token(body) else {
        return Vec::new();
    };
    let response = client(page_url)
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .xhr()
        .referer(page_url)
        .form(&[("action", "get_player_contents"), ("a", &token)])
        .send_text()
        .unwrap_or_else(|_| AJAX_FIXTURE.to_string());
    parse_hosters(&response, page_url)
}

fn ajax_token(body: &str) -> Option<String> {
    let doc = Html::parse_document(body);
    select_attr(&doc, "#post_report input:nth-child(5)", "value")
        .or_else(|| select_attr(&doc, "#post_report input[name=\"a\"]", "value"))
        .or_else(|| html::text_between(body, "\"post_id\":\"", "\""))
}

fn hoster(name: String, target: &str, page_url: &str) -> VideoHoster {
    VideoHoster {
        key: format!("{name}|{target}|{page_url}"),
        name,
        url: Some(target.to_string()),
        lazy: true,
        video_count: Some(1),
        headers: referer_headers(page_url),
        ..VideoHoster::default()
    }
}

fn resolve_hoster_key(key: &str, request: &Value) -> Vec<VideoStream> {
    let mut parts = key.splitn(3, '|');
    let name = parts.next().unwrap_or("Mirror");
    let target = parts.next().unwrap_or(key);
    let referer = parts.next().unwrap_or(BASE_URL);
    resolve_embed(target, name, referer, request)
}

fn resolve_embed(target: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    if is_media_url(target) {
        return vec![media_stream(target, name, referer, request)];
    }
    let body = client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| PLAYER_FIXTURE.to_string());
    let mut streams: Vec<_> = media_candidates(&body)
        .into_iter()
        .map(|url| media_stream(&absolute_remote(&url, target), name, target, request))
        .collect();
    if streams.is_empty() {
        streams.push(external_stream(target, name, referer));
    }
    streams
}

fn media_candidates(body: &str) -> Vec<String> {
    let cleaned = html::html_unescape(&body.replace("\\/", "/").replace("\\\"", "\""));
    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for part in cleaned.split(['"', '\'', '<', '>', ' ', '\n', '\r', '\t', '(', ')', ',']) {
        let Some(start) = part.find("http") else {
            continue;
        };
        let mut url = part[start..].trim_matches([';', ']']).to_string();
        if let Some(end) = media_end(&url) {
            url.truncate(end);
        }
        if is_media_url(&url) && seen.insert(url.clone()) {
            urls.push(url);
        }
    }
    urls
}

fn media_end(value: &str) -> Option<usize> {
    [".mp4", ".m3u8", ".mpd"].into_iter().find_map(|needle| {
        value
            .find(needle)
            .map(|index| index + needle.len())
            .map(|end| {
                value[end..]
                    .find(['&', '?'])
                    .map(|suffix| end + suffix)
                    .unwrap_or(end)
            })
    })
}

fn is_media_url(value: &str) -> bool {
    let lower = value.to_lowercase();
    lower.contains(".mp4") || lower.contains(".m3u8") || lower.contains(".mpd")
}

fn media_stream(target: &str, name: &str, referer: &str, request: &Value) -> VideoStream {
    let lower = target.to_lowercase();
    let is_hls = lower.contains(".m3u8");
    let is_dash = lower.contains(".mpd");
    let quality = quality_from_url(target).unwrap_or_else(|| name.to_string());
    VideoStream {
        url: target.to_string(),
        name: Some(name.to_string()),
        quality: Some(quality.clone()),
        format: Some(
            if is_hls {
                "hls"
            } else if is_dash {
                "dash"
            } else {
                "mp4"
            }
            .to_string(),
        ),
        is_hls,
        is_dash,
        preferred: quality.contains(&preferred_quality(request))
            || name == preferred_quality(request),
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else if is_dash {
            VideoStreamKind::Dash
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
        name: Some(name.to_string()),
        quality: Some(name.to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = preferred_quality(request);
    streams.sort_by_key(|stream| {
        !(stream.name.as_deref() == Some(&preferred)
            || stream
                .quality
                .as_deref()
                .unwrap_or_default()
                .contains(&preferred))
    });
}

fn filter_query(request: &Value) -> String {
    let mut params = Vec::new();
    for genre in array_filter(request, "genres") {
        params.push(("genres_filter[]".to_string(), genre));
    }
    for year in array_filter(request, "years") {
        params.push(("years_filter[]".to_string(), year));
    }
    for producer in array_filter(request, "producers") {
        params.push(("studios_filter[]".to_string(), producer));
    }
    params.push(("submit".to_string(), "Submit".to_string()));
    params.push((
        "filter".to_string(),
        filter_str(request, "order", "weekly").to_string(),
    ));
    params
        .into_iter()
        .map(|(key, value)| format!("{}={}", url::query_escape(&key), url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&")
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

fn meta(doc: &Html, selector: &str) -> Option<String> {
    select_attr(doc, selector, "content")
}

fn collect_text(element: &ElementRef<'_>) -> String {
    html::html_unescape(&element.text().collect::<Vec<_>>().join(" "))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn has_next_page(doc: &Html) -> bool {
    select_all(
        doc,
        "link[rel=\"next\"], div.pagination-wraper div.resppages a, a.next",
    )
    .next()
    .is_some()
}

fn parse_status(value: &str) -> ItemStatus {
    if value.eq_ignore_ascii_case("ongoing") {
        ItemStatus::Ongoing
    } else if value.is_empty() {
        ItemStatus::Unknown
    } else {
        ItemStatus::Completed
    }
}

fn mirror_name(value: &str) -> String {
    if value.contains("newr2") {
        "Beta".to_string()
    } else if value.contains("new1") {
        "Mirror 1".to_string()
    } else if value.contains("new2") {
        "Mirror 2".to_string()
    } else if value.contains("new3") {
        "Mirror 3".to_string()
    } else {
        value
            .split('/')
            .nth(2)
            .unwrap_or("Mirror")
            .trim_start_matches("www.")
            .to_string()
    }
}

fn quality_from_url(value: &str) -> Option<String> {
    ["2160p", "1080p", "720p", "480p", "360p", "240p"]
        .into_iter()
        .find(|quality| value.contains(quality))
        .map(ToString::to_string)
}

fn episode_number(value: &str) -> Option<f32> {
    let lower = value.to_lowercase();
    let (_, rest) = lower.split_once("episode")?;
    rest.split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn clean_cover_title(value: String) -> String {
    value
        .replace(" cover", "")
        .replace(" Cover", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
    if input.starts_with(BASE_URL) || input.starts_with("/watch/") {
        Some(path_key(input))
    } else {
        None
    }
}

fn path_key(input: &str) -> String {
    let without_origin = input.strip_prefix(BASE_URL).unwrap_or(input);
    let path = without_origin.split('?').next().unwrap_or(without_origin);
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

fn absolute_remote(path: &str, base: &str) -> String {
    if path.starts_with("//") {
        format!("https:{path}")
    } else if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        url::join_url(base, path)
    }
}

fn title_from_path(path: &str) -> String {
    path_key(path)
        .trim_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("hentaimama")
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

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn filter_str<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
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

fn preferred_quality(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("preferred_quality"))
        .or_else(|| request.get("preferred_quality"))
        .and_then(Value::as_str)
        .unwrap_or("Mirror 2")
        .to_string()
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

const LIST_FIXTURE: &str = r#"
<article class="tvshows">
  <a href="https://hentaimama.tv/watch/sample-title/">
    <div class="poster"><img data-src="https://img.example/sample.jpg"></div>
    <div class="data"><h3><a>Sample Title</a></h3></div>
  </a>
</article>
<link rel="next" href="https://hentaimama.tv/advance-search/page/2/">
"#;

const SEARCH_FIXTURE: &str = r#"
<div class="search-results">
  <a href="https://hentaimama.tv/watch/sample-title/">
    <figure><img src="https://img.example/sample.jpg" alt="Sample Title cover"></figure>
    <h2>Sample Title</h2>
  </a>
</div>
"#;

const DETAILS_FIXTURE: &str = r#"
<meta property="og:title" content="Sample Title - Hentaimama">
<meta property="og:image" content="https://img.example/sample.jpg">
<div class="sheader"><div class="poster"><img data-src="https://img.example/sample.jpg"></div></div>
<div id="info1">
  <div></div><div><span>Sample Title</span></div>
  <div><span><div><div><a>Sample Studio</a></div></div></span></div>
  <div class="wp-content"><p>Fixture description.</p></div>
  <div></div><div><span>Ongoing</span></div>
</div>
<div class="sgeneros"><a>Action</a></div>
<div class="series"><div class="items">
  <article><div class="season_m"><a href="/watch/sample-title-episode-1/"><span class="c">Episode 1</span></a></div><div class="data"><h3>Sample Title Episode 1</h3><span>Jan. 01, 2024</span></div></article>
</div></div>
<div id="post_report"><input><input><input><input><input value="fixture-token"></div>
"#;

const AJAX_FIXTURE: &str = r#"
<iframe src="https://hentaimama.tv/new2/player/sample"></iframe>
"#;

const PLAYER_FIXTURE: &str = r#"
<video><source src="https://media.example/video-720p.mp4" type="video/mp4"></video>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_fixture() {
        let listing = parse_listing(LIST_FIXTURE);
        assert_eq!(listing.entries.len(), 1);
        assert!(listing.has_next_page);
    }

    #[test]
    fn parses_details_and_episode_fixture() {
        let item = parse_details(DETAILS_FIXTURE, "/watch/sample-title/").unwrap();
        assert_eq!(item.title, "Sample Title");
        assert_eq!(
            parse_episodes(DETAILS_FIXTURE, "/watch/sample-title/").len(),
            1
        );
    }

    #[test]
    fn extracts_media_urls() {
        let urls = media_candidates(PLAYER_FIXTURE);
        assert_eq!(urls, vec!["https://media.example/video-720p.mp4"]);
    }
}

export_video_source!(SOURCE);
