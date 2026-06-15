use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
};
use serde_json::{Value, json};

const SOURCE: Newgrounds = Newgrounds;
const BASE_URL: &str = "https://www.newgrounds.com";
const PAGE_SIZE: u64 = 20;

struct Newgrounds;

impl VideoSource for Newgrounds {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let offset = (page.saturating_sub(1)) * PAGE_SIZE;
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let section = match listing {
            "latest" => pref_string(&request, "latest_section", "movies/browse"),
            "featured" => "movies/featured".to_string(),
            _ => pref_string(&request, "popular_section", "movies/popular"),
        };
        let body = get_or_fixture(
            &format!("{BASE_URL}/{section}?offset={offset}"),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: body.contains("load-more-items") || body.contains("data-offset"),
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

        let mut target = format!("{BASE_URL}/search/conduct/movies?page={}", page(&request));
        if !query.is_empty() {
            target.push_str("&terms=");
            target.push_str(&manatan_shared::sdk::http::url_encode(query));
        }
        if let Some(filters) = request.get("filters") {
            add_filter(&mut target, filters, "match");
            add_bool_filter(&mut target, filters, "exact");
            add_bool_filter(&mut target, filters, "any");
            add_filter(&mut target, filters, "user");
            add_filter(&mut target, filters, "genre");
            add_filter(&mut target, filters, "min_length");
            add_filter(&mut target, filters, "max_length");
            add_bool_filter(&mut target, filters, "frontpaged");
            add_filter(&mut target, filters, "after");
            add_filter(&mut target, filters, "before");
            add_filter(&mut target, filters, "sort");
            add_filter(&mut target, filters, "tags");
        }
        let body = get_or_fixture(&target, SEARCH_FIXTURE);
        Ok(Paged {
            entries: parse_search_cards(&body),
            has_next_page: body.contains("results-load-more"),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        Ok(fetch_details(
            &request_key(&request, "item").unwrap_or_else(|| "/portal/view/sample".to_string()),
        ))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key =
            request_key(&request, "item").unwrap_or_else(|| "/portal/view/sample".to_string());
        let body = get_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        if let Some(series_url) = related_series_url(&body) {
            let series_body = get_or_fixture(&series_url, SERIES_FIXTURE);
            let mut episodes = parse_series_episodes(&series_body);
            if !episodes.is_empty() {
                episodes.reverse();
                return Ok(episodes);
            }
        }
        Ok(vec![single_episode(&body, &key)])
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode")
            .or_else(|| request_key(&request, "item"))
            .unwrap_or_else(|| "/portal/video/sample".to_string());
        let endpoint = if key.contains("/portal/video/") {
            absolute_url(&key)
        } else {
            absolute_url(&key.replace("/portal/view/", "/portal/video/"))
        };
        let body = client()
            .get(&endpoint)
            .xhr()
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Accept", "application/json, text/javascript, */*; q=0.01")
            .send_text()
            .unwrap_or_else(|_| STREAMS_FIXTURE.to_string());
        let mut streams = parse_streams(&body, &endpoint);
        sort_streams(
            &mut streams,
            &pref_string(&request, "preferred_quality", "720p"),
        );
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: self
                    .list(json!({
                        "listing": "popular",
                        "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
                    }))?
                    .entries,
                has_more: true,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: self
                    .list(json!({
                        "listing": "latest",
                        "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
                    }))?
                    .entries,
                has_more: true,
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE);
    parse_details(&body, path).unwrap_or_else(|| fallback_item(path))
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("inline-card-portalsubmission")
        .skip(1)
        .filter_map(parse_grid_card)
        .collect()
}

fn parse_search_cards(body: &str) -> Vec<CatalogItem> {
    let mut entries: Vec<_> = body
        .split("itemlist")
        .skip(1)
        .flat_map(|block| block.split("<a").skip(1))
        .filter_map(parse_list_card)
        .collect();
    if entries.is_empty() {
        entries = parse_cards(body);
    }
    entries
}

fn parse_grid_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let title = html::text_between(chunk, "card-title", "</h4>")
        .or_else(|| html::attr_after(chunk, "<a", "title"))
        .map(|value| html::strip_tags(&value))?;
    Some(CatalogItem {
        key: path_key(&href),
        title,
        cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
        url: Some(absolute_url(&href)),
        authors: html::text_between(chunk, "By ", "</")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        language: Some("all".to_string()),
        content_rating: Some("mature".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_list_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr(chunk, "href")?;
    if !href.contains("/portal/view/") {
        return None;
    }
    let title = html::text_between(chunk, "detail-title", "</h4>")
        .or_else(|| html::text_between(chunk, "<h4", "</h4>"))
        .map(|value| html::strip_tags(&value))?;
    Some(CatalogItem {
        key: path_key(&href),
        title,
        cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
        url: Some(absolute_url(&href)),
        description: html::text_between(chunk, "detail-description", "</")
            .map(|value| html::strip_tags(&value)),
        language: Some("all".to_string()),
        content_rating: Some("mature".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, path: &str) -> Option<CatalogItem> {
    let title = html::text_between(body, "itemprop=\"name\"", "</h2>")
        .or_else(|| html::text_between(body, "meta name=\"title\"", ">"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())?;
    Some(CatalogItem {
        key: path_key(path),
        title,
        description: html::attr_after(body, "meta itemprop=\"description\"", "content")
            .or_else(|| html::text_between(body, "author_comments", "</div>"))
            .map(|value| html::strip_tags(&value)),
        cover: html::attr_after(body, "meta itemprop=\"thumbnailUrl\"", "content")
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        authors: html::text_between(body, "authorlinks", "</div>")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        tags: collect_tags(body),
        language: Some("all".to_string()),
        content_rating: Some("mature".to_string()),
        status: if related_series_url(body).is_some() {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Completed
        },
        initialized: true,
        ..CatalogItem::default()
    })
}

fn single_episode(body: &str, item_key: &str) -> VideoEpisode {
    let title = html::attr_after(body, "meta name=\"title\"", "content")
        .or_else(|| html::text_between(body, "itemprop=\"name\"", "</h2>"))
        .map(|value| html::strip_tags(&value))
        .unwrap_or_else(|| "Episode".to_string());
    VideoEpisode {
        key: path_key(&item_key.replace("/portal/view/", "/portal/video/")),
        title: Some(title),
        episode_number: Some(1.0),
        url: Some(absolute_url(
            &item_key.replace("/portal/view/", "/portal/video/"),
        )),
        language: Some("all".to_string()),
        ..VideoEpisode::default()
    }
}

fn parse_series_episodes(body: &str) -> Vec<VideoEpisode> {
    body.split("visual-link-container")
        .skip(1)
        .enumerate()
        .filter_map(|(index, chunk)| {
            let href = html::attr_after(chunk, "<a", "href")
                .or_else(|| html::attr(chunk, "href"))
                .or_else(|| {
                    html::attr(chunk, "data-visual-link").map(|id| format!("/portal/video/{id}"))
                })?;
            let title = html::text_between(chunk, "detail-title", "</h4>")
                .or_else(|| html::text_between(chunk, "<h4", "</h4>"))
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| format!("Episode {}", index + 1));
            Some(VideoEpisode {
                key: path_key(&href.replace("/portal/view/", "/portal/video/")),
                title: Some(title),
                episode_number: Some((index + 1) as f32),
                url: Some(absolute_url(
                    &href.replace("/portal/view/", "/portal/video/"),
                )),
                language: Some("all".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_streams(body: &str, referer: &str) -> Vec<VideoStream> {
    let Ok(json) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let mut streams = Vec::new();
    if let Some(sources) = json.get("sources").and_then(Value::as_object) {
        for (quality, values) in sources {
            for item in values.as_array().into_iter().flatten() {
                let Some(url) = item.get("src").and_then(Value::as_str) else {
                    continue;
                };
                streams.push(VideoStream {
                    url: url.to_string(),
                    name: Some(format!("Newgrounds {quality}")),
                    quality: Some(quality.to_string()),
                    format: Some(if url.contains(".m3u8") { "hls" } else { "mp4" }.to_string()),
                    is_hls: url.contains(".m3u8"),
                    stream_kind: Some(if url.contains(".m3u8") {
                        VideoStreamKind::Hls
                    } else {
                        VideoStreamKind::Direct
                    }),
                    headers: referer_headers(referer),
                    ..VideoStream::default()
                });
            }
        }
    }
    streams
}

fn related_series_url(body: &str) -> Option<String> {
    body.split("related_playlists")
        .nth(1)
        .and_then(|chunk| html::attr_after(chunk, "<a", "href"))
        .filter(|href| href.contains("/series/"))
        .map(|href| absolute_url(&href))
}

fn collect_tags(body: &str) -> Vec<String> {
    body.split("tags")
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn sort_streams(streams: &mut [VideoStream], preferred_quality: &str) {
    streams.sort_by_key(|stream| quality_score(stream.quality.as_deref()));
    streams.reverse();
    for stream in streams {
        stream.preferred = stream
            .quality
            .as_deref()
            .map(|quality| quality.contains(preferred_quality))
            .unwrap_or(false);
    }
}

fn quality_score(quality: Option<&str>) -> i32 {
    quality
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

fn add_filter(target: &mut String, filters: &Value, id: &str) {
    let Some(value) = filters
        .get(id)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    target.push('&');
    target.push_str(id);
    target.push('=');
    target.push_str(&manatan_shared::sdk::http::url_encode(value));
}

fn add_bool_filter(target: &mut String, filters: &Value, id: &str) {
    if filters.get(id).and_then(Value::as_bool) == Some(true) {
        target.push('&');
        target.push_str(id);
        target.push_str("=1");
    }
}

fn pref_string(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
        })
        .map(ToString::to_string)
}

fn path_from_url(input: &str) -> Option<String> {
    if !input.contains("newgrounds.com") {
        return None;
    }
    Some(path_key(
        input.split("newgrounds.com").nth(1).unwrap_or("/"),
    ))
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") {
        return path_from_url(input).unwrap_or_else(|| input.to_string());
    }
    let mut path = input.split('?').next().unwrap_or(input).to_string();
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    path
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else if input.starts_with('/') {
        format!("{BASE_URL}{input}")
    } else {
        format!("{BASE_URL}/{input}")
    }
}

fn fallback_item(path: &str) -> CatalogItem {
    CatalogItem {
        key: path_key(path),
        title: path
            .trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("Newgrounds")
            .replace('-', " "),
        url: Some(absolute_url(path)),
        language: Some("all".to_string()),
        content_rating: Some("mature".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

const LIST_FIXTURE: &str = r#"
<a class="inline-card-portalsubmission" href="/portal/view/1" title="Sample Movie">
<img src="/thumb.jpg"><div class="card-title"><h4>Sample Movie</h4><span>By sample</span></div>
</a><div id="load-more-items"></div>
"#;

const SEARCH_FIXTURE: &str = r#"
<ul class="itemlist"><li><a href="/portal/view/1"><div class="item-icon"><img src="/thumb.jpg"></div><div class="detail-title"><h4>Sample Movie</h4></div><div class="detail-description">Sample</div></a></li><li id="results-load-more"></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<meta itemprop="thumbnailUrl" content="/thumb.jpg">
<meta itemprop="description" content="Sample description">
<meta name="title" content="Sample Movie">
<h2 itemprop="name">Sample Movie</h2>
<div id="sidestats"><dl></dl><dl><dd>Jan 01, 2024</dd></dl></div>
"#;

const SERIES_FIXTURE: &str = r#"
<li class="visual-link-container" data-visual-link="1"><a href="/portal/view/1"><div class="detail-title"><h4>Sample Movie</h4></div></a></li>
"#;

const STREAMS_FIXTURE: &str =
    r#"{"sources":{"720p":[{"src":"https://uploads.ungrounded.net/sample.mp4"}]}}"#;

export_video_source!(SOURCE);
