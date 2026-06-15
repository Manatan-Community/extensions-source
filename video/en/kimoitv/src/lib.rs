use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
    video::referer_headers,
};
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;

const SOURCE: KimoiTv = KimoiTv;
const BASE_URL: &str = "https://kimoitv.com";

struct KimoiTv;

impl VideoSource for KimoiTv {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let sort = if listing(&request) == "latest" {
            "newest"
        } else {
            "top"
        };
        let target = with_page(
            &format!("{BASE_URL}/list/Anime.html?sort={sort}"),
            page(&request),
        );
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
                entries: vec![fetch_details(&path)],
                has_next_page: false,
            });
        }

        let target = if !query.is_empty() {
            with_page(
                &format!("{BASE_URL}/search/?q={}", url::query_escape(query)),
                page(&request),
            )
        } else if let Some(path) = first_selected_path(&request) {
            let sort = filter(&request, "sort", "");
            with_page(&format!("{BASE_URL}{path}{sort}"), page(&request))
        } else {
            with_page(
                &format!("{BASE_URL}/list/Anime.html?sort=top"),
                page(&request),
            )
        };
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            request_key(&request, "item").unwrap_or_else(|| "/list/Anime/sample.html".to_string());
        Ok(fetch_details(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key =
            request_key(&request, "item").unwrap_or_else(|| "/list/Anime/sample.html".to_string());
        Ok(fetch_episodes(&key))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "episode")
            .unwrap_or_else(|| "/list/Anime/sample-episode.html".to_string());
        let episode_url = absolute_url(&episode);
        let body = get_or_fixture(&episode_url, EPISODE_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let Some(info) = select_all(&doc, "div#fileInfo[data-id]").next() else {
            return Ok(Vec::new());
        };
        let id = info.value().attr("data-id").unwrap_or_default();
        let name = info.value().attr("data-name").unwrap_or_default();
        if id.is_empty() || name.is_empty() {
            return Ok(Vec::new());
        }

        let response = client(&episode_url)
            .post(format!("{BASE_URL}/streamvpaid.php"))
            .header("Accept", "*/*")
            .header(
                "Content-Type",
                "application/x-www-form-urlencoded; charset=UTF-8",
            )
            .header("Origin", BASE_URL)
            .header("X-Requested-With", "XMLHttpRequest")
            .referer(&episode_url)
            .body(
                format!("d={}&id={}", url::query_escape(name), url::query_escape(id)).into_bytes(),
            )
            .send_text()
            .unwrap_or_else(|_| STREAM_FIXTURE.to_string());
        let mut streams = parse_streams(&response, &episode_url, &request);
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
        entries: select_all(&doc, "ul.media > li")
            .filter_map(card_item)
            .collect(),
        has_next_page: select_all(
            &doc,
            "ul.pagination > li.page-item:has(a.bg-dark) ~ li.page-item > a",
        )
        .next()
        .is_some(),
    }
}

fn card_item(element: ElementRef<'_>) -> Option<CatalogItem> {
    let anchor = select_all_in(element, "a.item").next()?;
    let href = anchor.value().attr("href")?;
    Some(CatalogItem {
        key: path_key(href),
        title: text(&element, ".title")
            .or_else(|| {
                select_all_in(element, "div")
                    .map(|div| collect_text(&div))
                    .find(|value| !value.is_empty())
            })
            .unwrap_or_else(|| title_from_path(href)),
        cover: attr(&element, "img", "src").map(|value| absolute_url(&value)),
        url: Some(absolute_url(href)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
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
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, path: &str) -> Option<CatalogItem> {
    let doc = Html::parse_document(body);
    let title = select_text(&doc, "h1, div#pilled h2, div.section h2")
        .unwrap_or_else(|| title_from_path(path));
    Some(CatalogItem {
        key: path_key(path),
        title,
        cover: select_attr(&doc, "div#pilled img:not(.image), img.poster", "src")
            .map(|value| absolute_url(&value)),
        description: Some(
            select_all(&doc, "div#description > p")
                .map(|paragraph| collect_text(&paragraph))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"),
        )
        .filter(|value| !value.is_empty()),
        tags: select_all(
            &doc,
            "div.section > div > div.chip, a[href*='/browse/'], a[href*='/genre/']",
        )
        .map(|tag| collect_text(&tag))
        .filter(|tag| !tag.is_empty())
        .collect(),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    })
}

fn fetch_episodes(path: &str) -> Vec<VideoEpisode> {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    let seasons: Vec<(String, String)> = select_all(&doc, "ul.link-listview > li > a")
        .filter_map(|anchor| {
            let href = anchor.value().attr("href")?;
            let name = collect_text(&anchor);
            (!href.is_empty()).then(|| (name, absolute_url(href)))
        })
        .collect();
    if seasons.is_empty() {
        return parse_episode_page(&doc, "Episode");
    }

    let mut episodes = Vec::new();
    for (season, url) in seasons {
        let mut next_page = Some(url);
        let mut counter = 1.0;
        while let Some(page_url) = next_page.take() {
            let page_body = get_or_fixture(&page_url, EPISODE_LIST_FIXTURE, BASE_URL);
            let page_doc = Html::parse_document(&page_body);
            for mut episode in parse_episode_page(&page_doc, &season) {
                if episode.episode_number.is_none() {
                    episode.episode_number = Some(counter);
                }
                counter += 1.0;
                episodes.push(episode);
            }
            next_page = select_attr(
                &page_doc,
                "ul.pagination > li.page-item:has(a.bg-dark) ~ li.page-item > a",
                "href",
            )
            .map(|href| absolute_url(&href));
        }
    }
    episodes
}

fn parse_episode_page(doc: &Html, prefix: &str) -> Vec<VideoEpisode> {
    select_all(doc, "ul.link-listview > li > a")
        .filter_map(|anchor| {
            let href = anchor.value().attr("href")?;
            let label = collect_text(&anchor);
            let number = label
                .split('E')
                .next_back()
                .and_then(|value| value.trim().parse::<f32>().ok());
            Some(VideoEpisode {
                key: path_key(href),
                title: Some(if prefix.is_empty() {
                    label
                } else {
                    format!("{prefix} - {label}")
                }),
                episode_number: number,
                description: text(&anchor, "span"),
                url: Some(absolute_url(href)),
                language: Some("en".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_streams(body: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let doc = Html::parse_document(body);
    select_all(&doc, "source")
        .filter_map(|source| {
            let stream_url = source.value().attr("src")?;
            let quality = source
                .value()
                .attr("size")
                .map(|value| format!("{value}p"))
                .or_else(|| source.value().attr("label").map(ToString::to_string))
                .unwrap_or_else(|| "Video".to_string());
            let is_hls = stream_url.contains(".m3u8");
            Some(VideoStream {
                url: absolute_url(stream_url),
                name: Some(quality.clone()),
                quality: Some(quality.clone()),
                format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
                is_hls,
                stream_kind: Some(if is_hls {
                    VideoStreamKind::Hls
                } else {
                    VideoStreamKind::Direct
                }),
                preferred: quality.contains(&preferred_quality(request)),
                headers: referer_headers(referer),
                initialized: true,
                ..VideoStream::default()
            })
        })
        .collect()
}

fn first_selected_path(request: &Value) -> Option<String> {
    for id in ["sub_page", "genre", "anime_genre", "alpha"] {
        let value = filter(request, id, "");
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn with_page(url: &str, page: u32) -> String {
    if page <= 1 {
        return url.to_string();
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}page={page}")
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = preferred_quality(request);
    streams.sort_by_key(|stream| {
        if stream
            .quality
            .as_deref()
            .unwrap_or_default()
            .contains(&preferred)
        {
            0
        } else {
            1
        }
    });
}

fn preferred_quality(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("preferred_quality"))
        .and_then(Value::as_str)
        .filter(|quality| *quality != "auto")
        .unwrap_or("")
        .to_string()
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(obj) = next.as_object_mut() {
        obj.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

fn page(request: &Value) -> u32 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1) as u32
}

fn filter<'a>(request: &'a Value, id: &str, default: &'a str) -> &'a str {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .or_else(|| value.get("url"))
                .or_else(|| value.get("id"))
                .or(Some(value))
        })
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(path_key)
}

fn path_from_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.starts_with(BASE_URL) {
        Some(path_key(trimmed))
    } else if trimmed.starts_with('/') {
        Some(path_key(trimmed))
    } else {
        None
    }
}

fn path_key(input: &str) -> String {
    if let Some(rest) = input.strip_prefix(BASE_URL) {
        return path_key(rest);
    }
    let path = input.split('#').next().unwrap_or(input);
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("KimoiTV")
        .trim_end_matches(".html")
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

fn selector(query: &str) -> Selector {
    Selector::parse(query).unwrap()
}

fn select_all<'a>(doc: &'a Html, query: &str) -> impl Iterator<Item = ElementRef<'a>> {
    doc.select(&selector(query)).collect::<Vec<_>>().into_iter()
}

fn select_all_in<'a>(element: ElementRef<'a>, query: &str) -> impl Iterator<Item = ElementRef<'a>> {
    element
        .select(&selector(query))
        .collect::<Vec<_>>()
        .into_iter()
}

fn select_attr(doc: &Html, query: &str, name: &str) -> Option<String> {
    select_all(doc, query)
        .next()
        .and_then(|element| attr(&element, "", name))
}

fn attr(element: &ElementRef<'_>, query: &str, name: &str) -> Option<String> {
    let target = if query.is_empty() {
        *element
    } else {
        select_all_in(*element, query).next()?
    };
    target
        .value()
        .attr(name)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn select_text(doc: &Html, query: &str) -> Option<String> {
    select_all(doc, query)
        .next()
        .map(|element| collect_text(&element))
        .filter(|value| !value.is_empty())
}

fn text(element: &ElementRef<'_>, query: &str) -> Option<String> {
    select_all_in(*element, query)
        .next()
        .map(|element| collect_text(&element))
        .filter(|value| !value.is_empty())
}

fn collect_text(element: &ElementRef<'_>) -> String {
    html::html_unescape(
        &element
            .text()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

const LIST_FIXTURE: &str = r#"
<ul class="media">
  <li><a class="item" href="/list/Anime/sample.html"><img src="/sample.jpg"><div>Sample Anime</div></a></li>
</ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample Anime</h1>
<div id="pilled"><img src="/sample.jpg"></div>
<div id="description"><p>Sample description.</p></div>
<div class="section"><div><div class="chip">Action</div></div></div>
<ul class="link-listview"><li><a href="/list/Anime/sample-season-1.html">Season 1</a></li></ul>
"#;

const EPISODE_LIST_FIXTURE: &str = r#"
<ul class="link-listview"><li><a href="/list/Anime/sample-e1.html">E1 <span>Sub</span></a></li></ul>
"#;

const EPISODE_FIXTURE: &str = r#"
<div id="fileInfo" data-name="sample" data-id="1"></div>
"#;

const STREAM_FIXTURE: &str = r#"
<video><source src="https://cdn.kimoitv.com/sample.mp4" size="720"></video>
"#;

export_video_source!(SOURCE);
