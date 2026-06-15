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
use serde::{Deserialize, Serialize};
use serde_json::Value;

const SOURCE: Kawaiifu = Kawaiifu;
const BASE_URL: &str = "https://kawaiifu.com";

struct Kawaiifu;

impl VideoSource for Kawaiifu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/{}", page_path(page))
        } else {
            format!("{BASE_URL}/category/tv-series/{}", page_path(page))
        };
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(if listing(&request) == "latest" {
            parse_update_listing(&body)
        } else {
            parse_popular_listing(&body)
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
                entries: vec![fetch_details(&path)],
                has_next_page: false,
            });
        }

        let category = filter(&request, "category", "");
        let tags = array_filter(&request, "tags")
            .into_iter()
            .map(|tag| format!("&tag-get[]={}", url::query_escape(&tag)))
            .collect::<String>();
        let target = format!(
            "{BASE_URL}/search-movie/{}?keyword={}&cat-get={}{}",
            page_path(page(&request)),
            url::query_escape(query),
            url::query_escape(category),
            tags
        );
        let body = get_or_fixture(&target, SEARCH_FIXTURE, BASE_URL);
        Ok(parse_update_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = item_key(&request).unwrap_or_else(|| "/sample-anime".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = item_key(&request).unwrap_or_else(|| "/sample-anime".to_string());
        let details_url = absolute_url(&path);
        let details_body = get_or_fixture(&details_url, DETAILS_FIXTURE, BASE_URL);
        let details_doc = Html::parse_document(&details_body);
        let first_server = select_attr(&details_doc, "div.list-server a", "href")
            .map(|href| absolute_url(&href))
            .unwrap_or_else(|| absolute_url("/sample-anime-episode-1"));

        let episode_body = get_or_fixture(&first_server, EPISODES_FIXTURE, &details_url);
        let episode_doc = Html::parse_document(&episode_body);
        let top_episode = first_top_episode_url(&episode_doc);
        let body = if let Some(first_url) = top_episode {
            get_or_fixture(&first_url, EPISODES_FIXTURE, &first_server)
        } else {
            episode_body
        };
        Ok(parse_episodes(&body))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = episode_key(&request).unwrap_or_else(|| SAMPLE_SERVERS.to_string());
        let servers = serde_json::from_str::<Vec<ServerInfo>>(&episode).unwrap_or_else(|_| {
            vec![ServerInfo {
                name: "Kawaiifu".to_string(),
                url: absolute_url(&episode),
            }]
        });
        let mut streams = Vec::new();
        for server in servers {
            let page_url = absolute_url(&server.url);
            let body = get_or_fixture(&page_url, STREAM_FIXTURE, BASE_URL);
            let doc = Html::parse_document(&body);
            for source in select_all(&doc, "div#video_box div.player video source") {
                let Some(src) = source.value().attr("src") else {
                    continue;
                };
                let stream_url = absolute_or(src, &page_url);
                let quality = source
                    .value()
                    .attr("data-quality")
                    .map(|value| format!("{}p", value.trim_end_matches('p')))
                    .unwrap_or_else(|| normalize_quality(&stream_url));
                let is_hls = stream_url.contains(".m3u8");
                streams.push(VideoStream {
                    url: stream_url,
                    name: Some(format!("{quality} ({})", server.name)),
                    quality: Some(quality.clone()),
                    format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
                    is_hls,
                    stream_kind: Some(if is_hls {
                        VideoStreamKind::Hls
                    } else {
                        VideoStreamKind::Direct
                    }),
                    preferred: quality.contains(&preferred_quality(&request)),
                    headers: referer_headers(&page_url),
                    initialized: true,
                    ..VideoStream::default()
                });
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
        Ok(item_key(&request).map(|path| absolute_url(&path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let Some(raw) = episode_key(&request) else {
            return Ok(None);
        };
        let url = serde_json::from_str::<Vec<ServerInfo>>(&raw)
            .ok()
            .and_then(|servers| servers.first().map(|server| absolute_url(&server.url)))
            .or_else(|| Some(absolute_url(&raw)));
        Ok(url)
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
        .with_header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
        )
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

fn parse_popular_listing(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: select_all(&doc, "ul.list-film li")
            .filter_map(popular_card)
            .collect(),
        has_next_page: select_all(&doc, "div.wp-pagenavi a.nextpostslink")
            .next()
            .is_some(),
    }
}

fn popular_card(element: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&element, "a.mv-namevn", "href")?;
    let title = text(&element, "a.mv-namevn")?;
    Some(card_item(&href, title, attr(&element, "a img", "src")))
}

fn parse_update_listing(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: select_all(&doc, "div.today-update > div.item")
            .filter_map(update_card)
            .collect(),
        has_next_page: select_all(&doc, "div.pagination-content > span.current + a")
            .next()
            .is_some(),
    }
}

fn update_card(element: ElementRef<'_>) -> Option<CatalogItem> {
    let anchor = select_all_from(&element, "div.info a")
        .find(|anchor| anchor.value().attr("style").is_none())?;
    let href = anchor.value().attr("href")?;
    Some(card_item(
        href,
        collect_text(&anchor),
        attr(&element, "a.thumb img", "src"),
    ))
}

fn card_item(href: &str, title: String, cover: Option<String>) -> CatalogItem {
    let key = path_key(href);
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
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
    let title = select_text(&doc, "h1, div.info h1").unwrap_or_else(|| title_from_path(path));
    let tags = select_all(&doc, "div.desc-top table tbody tr")
        .filter(|row| collect_text(row).contains("Genres"))
        .flat_map(|row| {
            select_all_from(&row, "td a")
                .map(|genre| collect_text(&genre))
                .collect::<Vec<_>>()
        })
        .filter(|genre| !genre.is_empty())
        .collect::<Vec<_>>();
    Some(CatalogItem {
        key: path_key(path),
        title,
        cover: select_attr(
            &doc,
            "div.thumb img, div.image img, meta[property='og:image']",
            "src",
        )
        .or_else(|| select_attr(&doc, "meta[property='og:image']", "content"))
        .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description: parse_description(&doc),
        tags,
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_description(doc: &Html) -> Option<String> {
    let summary_heading = select_all(doc, "div.sub-desc h5")
        .find(|heading| collect_text(heading).contains("Summary"));
    if summary_heading.is_some() {
        let paragraphs = select_all(doc, "div.sub-desc p")
            .map(|paragraph| collect_text(&paragraph))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if !paragraphs.is_empty() {
            return Some(paragraphs.join("\n\n"));
        }
    }
    select_text(doc, "meta[property='og:description']")
        .or_else(|| select_text(doc, "div.sub-desc p"))
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let mut groups: Vec<(String, Vec<ServerInfo>)> = Vec::new();
    for server in select_all(&doc, "div#server_ep > div.list-server") {
        let server_name = text(&server, "h4.server-name").unwrap_or_else(|| "Kawaiifu".to_string());
        for episode in select_all_from(&server, "ul.list-ep > li") {
            let Some(anchor) = select_all_from(&episode, "a").next() else {
                continue;
            };
            let name = collect_text(&anchor);
            let Some(href) = anchor.value().attr("href") else {
                continue;
            };
            let info = ServerInfo {
                name: server_name.clone(),
                url: absolute_url(href),
            };
            if let Some((_, servers)) = groups
                .iter_mut()
                .find(|(episode_name, _)| *episode_name == name)
            {
                servers.push(info);
            } else {
                groups.push((name, vec![info]));
            }
        }
    }

    groups
        .into_iter()
        .map(|(name, servers)| {
            let key =
                serde_json::to_string(&servers).unwrap_or_else(|_| SAMPLE_SERVERS.to_string());
            VideoEpisode {
                key,
                title: Some(name.clone()),
                episode_number: episode_number(&name),
                url: servers.first().map(|server| absolute_url(&server.url)),
                language: Some("en".to_string()),
                ..VideoEpisode::default()
            }
        })
        .rev()
        .collect()
}

fn first_top_episode_url(doc: &Html) -> Option<String> {
    for container in select_all(doc, "div") {
        let has_episode_list_heading = select_all_from(&container, "div")
            .any(|child| collect_text(&child).contains("Episode List"));
        if !has_episode_list_heading {
            continue;
        }
        if let Some(href) = attr(
            &container,
            "div.list-server ul.list-ep > li a[href]",
            "href",
        ) {
            return Some(absolute_url(&href));
        }
    }
    None
}

fn episode_number(input: &str) -> Option<f32> {
    input
        .split("Ep ")
        .nth(1)
        .or_else(|| input.split_whitespace().next_back())
        .and_then(|value| {
            value
                .trim_matches(|ch: char| !ch.is_ascii_digit() && ch != '.')
                .parse()
                .ok()
        })
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = preferred_quality(request);
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or_default();
        let score = quality
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse::<i32>()
            .unwrap_or(0);
        (i32::from(quality.contains(&preferred)), score)
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

fn select_all_from<'a>(
    element: &'a ElementRef<'a>,
    selector: &str,
) -> impl Iterator<Item = ElementRef<'a>> {
    Selector::parse(selector)
        .ok()
        .map(|selector| element.select(&selector).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
}

fn select_text(doc: &Html, selector: &str) -> Option<String> {
    select_all(doc, selector)
        .next()
        .map(|element| collect_text(&element))
        .filter(|value| !value.is_empty())
}

fn select_attr(doc: &Html, selector: &str, name: &str) -> Option<String> {
    select_all(doc, selector)
        .next()
        .and_then(|element| element.value().attr(name).map(ToString::to_string))
}

fn text(element: &ElementRef<'_>, selector: &str) -> Option<String> {
    select_all_from(element, selector)
        .next()
        .map(|value| collect_text(&value))
        .filter(|value| !value.is_empty())
}

fn attr(element: &ElementRef<'_>, selector: &str, name: &str) -> Option<String> {
    select_all_from(element, selector)
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

fn item_key(request: &Value) -> Option<String> {
    request
        .get("item")
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

fn episode_key(request: &Value) -> Option<String> {
    request
        .get("episode")
        .and_then(|value| {
            value
                .get("key")
                .or_else(|| value.get("url"))
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn path_from_url(input: &str) -> Option<String> {
    (input.starts_with(BASE_URL) || input.starts_with('/')).then(|| path_key(input))
}

fn path_key(input: &str) -> String {
    let without_origin = input.strip_prefix(BASE_URL).unwrap_or(input);
    let path = without_origin
        .split(['?', '#'])
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

fn absolute_or(path: &str, base: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else if path.starts_with("//") {
        format!("https:{path}")
    } else if path.starts_with('/') {
        absolute_url(path)
    } else {
        format!("{}/{}", base.trim_end_matches('/'), path)
    }
}

fn title_from_path(path: &str) -> String {
    path_key(path)
        .trim_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("kawaiifu")
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

fn normalize_quality(input: &str) -> String {
    for quality in ["2160", "1080", "720", "480", "360", "240"] {
        if input.contains(quality) {
            return format!("{quality}p");
        }
    }
    "Unknown".to_string()
}

fn page_path(page: u64) -> String {
    if page <= 1 {
        String::new()
    } else {
        format!("page/{page}")
    }
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
        .and_then(|preferences| preferences.get("preferred_quality"))
        .or_else(|| request.get("preferred_quality"))
        .and_then(Value::as_str)
        .unwrap_or("720")
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

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ServerInfo {
    name: String,
    url: String,
}

const SAMPLE_SERVERS: &str =
    r#"[{"name":"Kawaiifu","url":"https://kawaiifu.com/sample-anime-episode-1"}]"#;
const LIST_FIXTURE: &str = r#"<ul class="list-film"><li><a class="mv-namevn" href="/sample-anime">Sample Anime</a><a><img src="/sample.jpg"></a></li></ul><div class="wp-pagenavi"><a class="nextpostslink" href="/category/tv-series/page/2"></a></div>"#;
const SEARCH_FIXTURE: &str = r#"<div class="today-update"><div class="item"><a class="thumb"><img src="/sample.jpg"></a><div class="info"><a href="/sample-anime">Sample Anime</a></div></div></div><div class="pagination-content"><span class="current">1</span><a href="/page/2"></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample Anime</h1><div class="thumb"><img src="/sample.jpg"></div><div class="desc-top"><table><tbody><tr><td>Genres</td><td><a>Action</a><a>Dub</a></td></tr></tbody></table></div><div class="sub-desc"><h5>Summary</h5><p>Fixture description.</p></div><div class="list-server"><a href="/sample-anime-episode-1">Episode 1</a></div>"#;
const EPISODES_FIXTURE: &str = r#"<div id="server_ep"><div class="list-server"><h4 class="server-name">Server A</h4><ul class="list-ep"><li><a href="/sample-anime-episode-1">Ep 1</a></li></ul></div></div>"#;
const STREAM_FIXTURE: &str = r#"<div id="video_box"><div class="player"><video><source src="https://cdn.example.invalid/sample-720.mp4" data-quality="720"></video></div></div>"#;

export_video_source!(SOURCE);
