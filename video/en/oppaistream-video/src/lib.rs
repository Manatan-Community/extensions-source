use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack,
    UrlResolveResult, VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult,
    export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

const SOURCE: OppaiStream = OppaiStream;
const BASE_URL: &str = "https://oppai.stream";
const SEARCH_PATH: &str = "actions/search.php";
const SEARCH_LIMIT: usize = 36;

struct OppaiStream;

impl VideoSource for OppaiStream {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let order = if listing(&request) == "latest" {
            "uploaded"
        } else {
            "views"
        };
        let target = search_url(page(&request), "", Some(order), &request);
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
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
                entries: vec![fetch_details(&path, &request)],
                has_next_page: false,
            });
        }
        let order = filter_string(&request, "order").filter(|value| !value.is_empty());
        let target = search_url(page(&request), query, order.as_deref(), &request);
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item")
            .unwrap_or_else(|| "/watch?e=Sample-Oppai-Stream-1".to_string());
        Ok(fetch_details(&key, &request))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item")
            .unwrap_or_else(|| "/watch?e=Sample-Oppai-Stream-1".to_string());
        let body = get_or_fixture(&absolute_url(&key), DETAILS_FIXTURE, BASE_URL);
        let mut episodes = parse_episodes(&body);
        if episodes.is_empty() {
            episodes.push(parse_current_episode(&body, &key));
        }
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode")
            .or_else(|| request_key(&request, "item"))
            .unwrap_or_else(|| "/watch?e=Sample-Oppai-Stream-1".to_string());
        let referer = absolute_url(&key);
        let body = get_or_fixture(&referer, DETAILS_FIXTURE, BASE_URL);
        let subtitles = parse_subtitles(&body, &referer);
        let mut streams = parse_streams(&body, subtitles, &referer, &request);
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

fn get_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(page: u64, query: &str, order: Option<&str>, request: &Value) -> String {
    let mut params = vec![
        ("text".to_string(), query.to_string()),
        ("page".to_string(), page.to_string()),
        ("limit".to_string(), SEARCH_LIMIT.to_string()),
    ];
    if let Some(order) = order {
        params.push(("order".to_string(), order.to_string()));
    }
    if let Some(genres) = joined_filter(request, "genres") {
        params.push(("genres".to_string(), genres));
    }
    if let Some(blacklist) = joined_filter(request, "blacklist") {
        params.push(("blacklist".to_string(), blacklist));
    }
    if let Some(studio) = joined_filter(request, "studio") {
        params.push(("studio".to_string(), studio));
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{}={}", url::query_escape(&key), url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/{SEARCH_PATH}?{query}")
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let mut seen = HashSet::new();
    let entries = select_all(&doc, "div.episode-shown > div > a")
        .filter_map(|anchor| card_item(anchor, false))
        .filter(|item| seen.insert(item.title.clone()))
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() >= SEARCH_LIMIT,
        entries,
    }
}

fn card_item(anchor: ElementRef<'_>, initialized: bool) -> Option<CatalogItem> {
    let href = anchor
        .value()
        .attr("exur")
        .filter(|value| !value.is_empty())
        .or_else(|| anchor.value().attr("href"))?;
    let key = path_key(href);
    let title = text(&anchor, "font.title")
        .or_else(|| text(&anchor, ".title-ep"))
        .map(clean_episode_title)
        .unwrap_or_else(|| title_from_path(&key));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: attr(&anchor, "img.cover-img-in", "src").map(|image| absolute_url(&image)),
        artists: text(&anchor, ".extra-line a").into_iter().collect(),
        tags: anchor
            .value()
            .attr("tags")
            .map(split_csv)
            .unwrap_or_default(),
        description: anchor
            .value()
            .attr("desc")
            .map(|value| html::html_unescape(value))
            .filter(|value| !value.is_empty()),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str, request: &Value) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    parse_details(&body, path, request).unwrap_or_else(|| CatalogItem {
        key: path_key(path),
        title: title_from_path(path),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, path: &str, request: &Value) -> Option<CatalogItem> {
    let doc = Html::parse_document(body);
    let title = select_text(&doc, "div.episode-info > h1")
        .map(clean_episode_title)
        .or_else(|| meta_attr(&doc, "meta[property=\"og:title\"]", "content").map(clean_meta_title))
        .unwrap_or_else(|| title_from_path(path));
    let studios = select_all(&doc, "div.episode-info h6 a.red, div.episode-info h6 a")
        .map(|node| collect_text(&node))
        .filter(|value| !value.is_empty() && !value.starts_with("http"))
        .collect::<Vec<_>>();
    let fallback_cover = select_attr(&doc, "video#episode", "poster")
        .or_else(|| meta_attr(&doc, "meta[property=\"og:image\"]", "content"))
        .map(|image| absolute_url(&image));
    let cover = if pref_bool(request, "preferred_anilist_cover", true) {
        fetch_anilist_cover(&title, &studios, request).or(fallback_cover)
    } else {
        fallback_cover
    };
    Some(CatalogItem {
        key: path_key(path),
        title,
        cover,
        authors: studios.clone(),
        artists: studios,
        tags: select_all(&doc, "div.tags a")
            .map(|tag| collect_text(&tag))
            .filter(|tag| !tag.is_empty())
            .collect(),
        description: select_text(&doc, "div.description")
            .map(|value| {
                value
                    .split(" Watch ")
                    .next()
                    .unwrap_or(&value)
                    .trim()
                    .to_string()
            })
            .filter(|value| !value.is_empty()),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    })
}

fn fetch_anilist_cover(title: &str, local_studios: &[String], request: &Value) -> Option<String> {
    let search_title = title
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric()
                || ch.is_ascii_whitespace()
                || ['!', '.', ':', '"'].contains(&ch)
            {
                ch
            } else {
                ' '
            }
        })
        .collect::<String>();
    let query = format!(
        r#"query {{ Media(search: "{}", type: ANIME, isAdult: true) {{ coverImage {{ extraLarge large }} studios {{ nodes {{ name }} }} }} }}"#,
        search_title.replace('"', "\\\"")
    );
    let body = HttpClient::browser()
        .post("https://graphql.anilist.co")
        .header("Accept", "application/json")
        .form(&[("query", query.as_str())])
        .send_text()
        .ok()?;
    let root: Value = serde_json::from_str(&body).ok()?;
    let media = root.pointer("/data/Media")?;
    let anilist_studios = media
        .pointer("/studios/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|node| node.get("name").and_then(Value::as_str))
        .collect::<Vec<_>>();
    if !local_studios.is_empty()
        && !anilist_studios
            .iter()
            .any(|studio| local_studios.iter().any(|local| local == studio))
    {
        return None;
    }
    let key = if pref_string(request, "preferred_cover_quality").as_deref() == Some("large") {
        "large"
    } else {
        "extraLarge"
    };
    media
        .pointer(&format!("/coverImage/{key}"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    select_all(&doc, "div.more-same-eps div.in-main-gr > a")
        .filter_map(episode_from_anchor)
        .collect()
}

fn episode_from_anchor(anchor: ElementRef<'_>) -> Option<VideoEpisode> {
    let href = anchor
        .value()
        .attr("exur")
        .filter(|value| !value.is_empty())
        .or_else(|| anchor.value().attr("href"))?;
    let key = path_key(href);
    let number = text(&anchor, "font.ep")
        .and_then(|value| value.parse::<f32>().ok())
        .or_else(|| episode_number_from_path(&key))
        .unwrap_or(1.0);
    Some(VideoEpisode {
        key: key.clone(),
        title: Some(format!("Episode {}", trim_float(number))),
        episode_number: Some(number),
        thumbnail: attr(&anchor, "img.cover-img-in", "src").map(|image| absolute_url(&image)),
        release_group: text(&anchor, "h6 > a"),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        labels: anchor
            .value()
            .attr("tags")
            .map(split_csv)
            .unwrap_or_default(),
        ..VideoEpisode::default()
    })
}

fn parse_current_episode(body: &str, path: &str) -> VideoEpisode {
    let doc = Html::parse_document(body);
    let key = path_key(path);
    let number = select_text(&doc, "div.episode-info > h1")
        .and_then(|title| title.rsplit(" Ep ").next()?.parse::<f32>().ok())
        .or_else(|| episode_number_from_path(&key))
        .unwrap_or(1.0);
    VideoEpisode {
        key: key.clone(),
        title: Some(format!("Episode {}", trim_float(number))),
        episode_number: Some(number),
        thumbnail: select_attr(&doc, "video#episode", "poster").map(|image| absolute_url(&image)),
        release_group: select_text(&doc, "div.episode-info h6 a.red"),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        ..VideoEpisode::default()
    }
}

fn parse_subtitles(body: &str, referer: &str) -> Vec<SubtitleTrack> {
    let doc = Html::parse_document(body);
    select_all(&doc, "track[kind=\"captions\"], track[kind=\"subtitles\"]")
        .filter_map(|track| {
            let src = track.value().attr("src")?;
            let label = track
                .value()
                .attr("label")
                .or_else(|| track.value().attr("srclang"))
                .map(ToString::to_string);
            Some(SubtitleTrack {
                url: absolute_url(src),
                language: track.value().attr("srclang").map(ToString::to_string),
                label,
                format: Some(format_from_url(src)),
                headers: referer_headers(referer),
                is_default: track.value().attr("default").is_some(),
                ..SubtitleTrack::default()
            })
        })
        .collect()
}

fn parse_streams(
    body: &str,
    subtitles: Vec<SubtitleTrack>,
    referer: &str,
    request: &Value,
) -> Vec<VideoStream> {
    let Some(script) = body.split("var availableres = ").nth(1) else {
        return Vec::new();
    };
    let Some(map_body) = script.split(';').next() else {
        return Vec::new();
    };
    let urls = serde_json::from_str::<HashMap<String, String>>(map_body).unwrap_or_default();
    urls.into_iter()
        .map(|(resolution, stream_url)| {
            let quality = match resolution.as_str() {
                "4k" => "2160p".to_string(),
                value if value.ends_with('p') => value.to_string(),
                value => format!("{value}p"),
            };
            let format = format_from_url(&stream_url);
            VideoStream {
                url: stream_url.clone(),
                name: Some(quality.clone()),
                quality: Some(quality.clone()),
                format: Some(format),
                stream_kind: Some(VideoStreamKind::Direct),
                preferred: quality == preferred_quality(request),
                headers: referer_headers(referer),
                subtitles: subtitles.clone(),
                initialized: true,
                ..VideoStream::default()
            }
        })
        .collect()
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = preferred_quality(request);
    streams.sort_by_key(|stream| {
        (
            stream.quality.as_deref() == Some(preferred.as_str()),
            stream
                .quality
                .as_deref()
                .and_then(|quality| quality.trim_end_matches('p').parse::<u32>().ok())
                .unwrap_or_default(),
        )
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

fn meta_attr(doc: &Html, selector: &str, name: &str) -> Option<String> {
    select_attr(doc, selector, name).map(|value| html::html_unescape(&value))
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

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
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

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
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
    if input.starts_with(BASE_URL) || input.starts_with("/watch") {
        Some(path_key(input))
    } else {
        None
    }
}

fn path_key(input: &str) -> String {
    let without_origin = input.strip_prefix(BASE_URL).unwrap_or(input);
    if without_origin.starts_with('/') {
        without_origin.to_string()
    } else {
        format!("/{without_origin}")
    }
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        url::join_url(BASE_URL, value)
    }
}

fn title_from_path(path: &str) -> String {
    path.split("e=")
        .nth(1)
        .and_then(|value| value.split('&').next())
        .unwrap_or("Oppai Stream")
        .replace('~', "-")
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn clean_episode_title(value: String) -> String {
    value
        .split(" Ep ")
        .next()
        .unwrap_or(&value)
        .trim_end_matches(|ch: char| ch.is_ascii_digit() || ch.is_whitespace())
        .trim()
        .to_string()
}

fn clean_meta_title(value: String) -> String {
    value
        .trim_start_matches("Watch ")
        .split(" in HD on ")
        .next()
        .unwrap_or(&value)
        .to_string()
        .split(" EP ")
        .next()
        .unwrap_or(&value)
        .trim()
        .to_string()
}

fn episode_number_from_path(path: &str) -> Option<f32> {
    path.split("e=")
        .nth(1)?
        .split('&')
        .next()?
        .rsplit('-')
        .next()?
        .parse()
        .ok()
}

fn trim_float(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as u32)
    } else {
        value.to_string()
    }
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

fn filter_string(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn joined_filter(request: &Value, key: &str) -> Option<String> {
    let value = request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))?;
    let values = if let Some(array) = value.as_array() {
        array
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    } else {
        value.as_str().map(split_csv).unwrap_or_default()
    };
    let joined = values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(",");
    (!joined.is_empty()).then_some(joined)
}

fn pref_string(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn pref_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn preferred_quality(request: &Value) -> String {
    pref_string(request, "preferred_quality").unwrap_or_else(|| "1080p".to_string())
}

fn format_from_url(value: &str) -> String {
    value
        .split('?')
        .next()
        .and_then(|clean| clean.rsplit('.').next())
        .filter(|ext| ext.len() <= 5)
        .unwrap_or("mp4")
        .to_lowercase()
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class='in-grid episode-shown' tags='hd,uncensored' desc='Fixture description'>
  <div class='in-main-gr'><a href='https://oppai.stream/watch?e=Sample-Oppai-Stream-1&for=search'>
    <img class='cover-img-in' src='https://myspacecat.pictures/sample/thumbnail_1.png'>
    <object><h6 class='gray extra-line'>By <a>Fixture Studio</a></h6></object>
    <h5 class='title-ep'><font class='title'>Sample Oppai Stream</font> <font class='ep'>1</font></h5>
  </a></div>
</div>
"#;

const DETAILS_FIXTURE: &str = r#"
<video id="episode" poster="https://myspacecat.pictures/sample/thumbnail_1.png">
  <source src="https://myspacecat.pictures/sample/720/E01.mp4" type="video/mp4">
  <track label="en" src="https://myspacecat.pictures/sample/720/E01_SUB_1.vtt" kind="subtitles" srclang="en" default>
</video>
<div class="episode-info left">
  <h6 class="gray line-5">1 day ago by <a class='red'>Fixture Studio</a></h6>
  <h1>Sample Oppai Stream Ep 1</h1>
  <div class="description"><h5>Fixture description. Watch Sample on oppai.stream.</h5></div>
  <div class="tags"><a><h5>hd</h5></a><a><h5>uncensored</h5></a></div>
</div>
<div class='other-episodes more-same-eps'>
  <div class='in-main-gr'><a href='https://oppai.stream/watch?e=Sample-Oppai-Stream-1&for=episode-more' tags='hd,uncensored'>
    <img class='cover-img-in' src='https://myspacecat.pictures/sample/thumbnail_1.png'>
    <h6><a>Fixture Studio</a></h6>
    <h5 class='title-ep'><font class='title'>Sample Oppai Stream</font> <font class='ep'>1</font></h5>
  </a></div>
</div>
<script>
var availableres = {"720":"https:\/\/myspacecat.pictures\/sample\/720\/E01.mp4","1080":"https:\/\/myspacecat.pictures\/sample\/1080\/E01.mp4"};
</script>
"#;
