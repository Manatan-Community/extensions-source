use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    dates, html,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;

const SOURCE: HahoMoe = HahoMoe;
const BASE_URL: &str = "https://haho.moe";

struct HahoMoe;

impl VideoSource for HahoMoe {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let sort = if listing(&request) == "latest" { "rel-d" } else { "vdy-d" };
        let target = format!("{BASE_URL}/anime?s={sort}&page={}", page(&request));
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&path)],
                has_next_page: false,
            });
        }
        let include = array_filter(&request, "include_tags");
        let exclude = array_filter(&request, "exclude_tags");
        let mut search = String::new();
        if !query.is_empty() {
            search.push_str(query);
        }
        for tag in include {
            search.push_str(" genre:");
            search.push_str(&tag);
        }
        for tag in exclude {
            search.push_str(" -genre:");
            search.push_str(&tag);
        }
        let sort = filter(&request, "sort", "az-");
        let order = filter(&request, "order", "a");
        let target = format!(
            "{BASE_URL}/anime?page={}&s={sort}{order}&q={}",
            page(&request),
            url::query_escape(&search)
        );
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(|| "/anime/sample?s=srt-d".to_string());
        Ok(fetch_details(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_else(|| "/anime/sample?s=srt-d".to_string());
        Ok(fetch_episodes(&key))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "episode").unwrap_or_else(|| "/anime/sample/episode-1".to_string());
        let episode_url = absolute_url(&episode);
        let body = get_or_fixture(&episode_url, EPISODE_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let Some(iframe) = select_attr(&doc, "iframe", "src") else {
            return Ok(Vec::new());
        };
        let iframe_url = absolute_url(&iframe);
        let iframe_body = get_or_fixture(&iframe_url, IFRAME_FIXTURE, &episode_url);
        let mut streams = parse_sources(&iframe_body, &iframe_url, &request);
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
        .with_header("Cookie", "loop-view=thumb")
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: select_all(&doc, "ul.anime-loop.loop > li > a")
            .filter_map(card_item)
            .collect(),
        has_next_page: select_all(&doc, "ul.pagination li.page-item a[rel=next]")
            .next()
            .is_some(),
    }
}

fn card_item(anchor: ElementRef<'_>) -> Option<CatalogItem> {
    let href = anchor.value().attr("href")?;
    let title = text(&anchor, "div.label > span")
        .or_else(|| text(&anchor, "div span.thumb-title"))
        .or_else(|| Some(title_from_path(href)))?;
    let key = path_key(&format!("{href}?s=srt-d"));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: attr(&anchor, "img", "src").map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
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
    Some(CatalogItem {
        key: path_key(path),
        title: select_text(&doc, "li.breadcrumb-item.active").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr(&doc, "img.cover-image.img-thumbnail", "src").map(|value| absolute_url(&value)),
        url: Some(absolute_url(path)),
        tags: select_all(&doc, "li.genre span.value, div.genre-tree ul > li > a")
            .map(|tag| collect_text(&tag))
            .filter(|tag| !tag.is_empty())
            .collect(),
        description: select_text(&doc, "div.card-body"),
        authors: select_all(&doc, "li.production span.value")
            .map(|value| collect_text(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        artists: select_text(&doc, "li.group span.value").into_iter().collect(),
        status: parse_status(select_text(&doc, "li.status span.value").as_deref()),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn fetch_episodes(path: &str) -> Vec<VideoEpisode> {
    let mut next = Some(absolute_url(path));
    let mut episodes = Vec::new();
    while let Some(target) = next.take() {
        let body = get_or_fixture(&target, DETAILS_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        episodes.extend(parse_episode_page(&doc));
        next = select_attr(&doc, "ul.pagination li.page-item a[rel=next]", "href").map(|href| absolute_url(&href));
        if episodes.len() > 500 {
            break;
        }
    }
    episodes.sort_by(|a, b| {
        b.episode_number
            .partial_cmp(&a.episode_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    episodes
}

fn parse_episode_page(doc: &Html) -> Vec<VideoEpisode> {
    select_all(doc, "ul.episode-loop > li > a")
        .filter_map(|anchor| {
            let href = anchor.value().attr("href")?;
            let number_text = text(&anchor, "div.episode-number, div.episode-slug")
                .unwrap_or_else(|| "Episode".to_string());
            let number = number_text.trim_start_matches("Episode ").parse::<f32>().unwrap_or(1.0);
            let extra = text(&anchor, "div.episode-label, div.episode-title")
                .filter(|title| !title.eq_ignore_ascii_case("No Title"))
                .map(|title| format!(": {title}"))
                .unwrap_or_default();
            let date = text(&anchor, "div.date").and_then(|value| parse_date(&value));
            Some(VideoEpisode {
                key: path_key(href),
                title: Some(format!("{number_text}{extra}")),
                episode_number: Some(number),
                date_uploaded: date,
                url: Some(absolute_url(href)),
                language: Some("en".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_sources(body: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let doc = Html::parse_document(body);
    select_all(&doc, "source")
        .filter_map(|source| {
            let stream_url = source.value().attr("src")?;
            let title = source.value().attr("title").unwrap_or("stream");
            let is_hls = stream_url.contains(".m3u8");
            Some(VideoStream {
                url: absolute_url(stream_url),
                name: Some(title.to_string()),
                quality: Some(title.to_string()),
                format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
                is_hls,
                stream_kind: Some(if is_hls { VideoStreamKind::Hls } else { VideoStreamKind::Direct }),
                preferred: title.contains(&preferred_quality(request)),
                headers: referer_headers(referer),
                initialized: true,
                ..VideoStream::default()
            })
        })
        .collect()
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value {
        Some("Ongoing") => ItemStatus::Ongoing,
        Some("Completed") => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: &str) -> Option<i64> {
    let cleaned = value
        .replace("st", "")
        .replace("nd", "")
        .replace("rd", "")
        .replace("th", "")
        .replace(" of ", " ")
        .replace(',', "");
    let mut parts = cleaned.split_whitespace();
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year = parts.next()?.parse::<u32>().ok()?;
    dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = preferred_quality(request);
    streams.sort_by_key(|stream| stream.quality.as_deref().unwrap_or_default().contains(&preferred));
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
    (input.starts_with(BASE_URL) || input.starts_with("/anime/")).then(|| path_key(input))
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
        .unwrap_or("haho.moe")
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

fn filter<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .or_else(|| request.get(key).and_then(Value::as_str))
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
        .and_then(|preferences| preferences.get("pref_quality_key"))
        .or_else(|| request.get("pref_quality_key"))
        .and_then(Value::as_str)
        .unwrap_or("720p")
        .to_string()
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

const LIST_FIXTURE: &str = r#"<ul class="anime-loop loop"><li><a href="/anime/sample"><div class="label"><span>Sample haho.moe</span></div><img src="/cover.jpg"></a></li></ul>"#;
const DETAILS_FIXTURE: &str = r#"<li class="breadcrumb-item active">Sample haho.moe</li><img class="cover-image img-thumbnail" src="/cover.jpg"><li class="genre"><span class="value">Action</span></li><div class="card-body">Fixture description.</div><li class="status"><span class="value">Completed</span></li><ul class="episode-loop"><li><a href="/anime/sample/episode-1"><div class="episode-number">Episode 1</div><div class="episode-label">Pilot</div><div class="date">1st of Jan, 2024</div></a></li></ul>"#;
const EPISODE_FIXTURE: &str = r#"<iframe src="https://haho.moe/embed/sample"></iframe>"#;
const IFRAME_FIXTURE: &str = r#"<video><source src="https://fixtures.invalid/video-720.mp4" title="720p"></video>"#;

export_video_source!(SOURCE);
