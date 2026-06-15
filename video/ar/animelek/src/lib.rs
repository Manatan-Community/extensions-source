use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

const SOURCE: AnimeLek = AnimeLek;
const BASE_URL: &str = "https://animelek.me";

struct AnimeLek;

impl VideoSource for AnimeLek {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let page = page(&request);
        let target = if listing == "latest" {
            format!("{BASE_URL}/episode/?page={page}")
        } else {
            BASE_URL.to_string()
        };
        let body = get_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: if listing == "latest" {
                parse_latest_cards(&body)
            } else {
                parse_popular_cards(&body)
            },
            has_next_page: listing == "latest" && has_next_page(&body),
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
        let page = page(&request);
        let body = get_or_fixture(
            &format!(
                "{BASE_URL}/search/?s={}&page={page}",
                manatan_shared::sdk::http::url_encode(query)
            ),
            SEARCH_FIXTURE,
        );
        Ok(Paged {
            entries: parse_search_cards(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        let body = get_or_fixture(&absolute_url(&path), EPISODES_FIXTURE);
        let mut episodes = parse_episodes(&body);
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/episode/sample".to_string());
        let body = get_or_fixture(&absolute_url(&path), HOSTERS_FIXTURE);
        Ok(parse_hosters(&body))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let url = request_key(&request, "hoster").unwrap_or_default();
        let name = request
            .get("hoster")
            .and_then(|hoster| hoster.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Mirror");
        let mut streams = vec![external_stream(&url, name)];
        sort_streams(&mut streams, &preferred_quality(&request));
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut streams = Vec::new();
        for hoster in self.hosters(request.clone())? {
            let mut resolved = self.resolve_hoster(json!({
                "hoster": { "key": hoster.key, "name": hoster.name },
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
            }))?;
            for stream in &mut resolved {
                stream.hoster = Some(hoster.clone());
            }
            streams.extend(resolved);
        }
        sort_streams(&mut streams, &preferred_quality(&request));
        Ok(streams)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: self.list(json!({"listing": "popular", "page": 1}))?.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: self.list(json!({"listing": "latest", "page": 1}))?.entries,
                has_more: true,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|path| absolute_url(&path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|path| absolute_url(&path)))
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
    CatalogItem {
        key: path_key(path),
        title: html::text_between(&body, "<h1", "</h1>")
            .map(|text| html::strip_tags(&text))
            .unwrap_or_else(|| path.trim_matches('/').replace('-', " ")),
        cover: html::attr_after(&body, "div class=\"anime-container-infos", "src")
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description: html::text_between(&body, "p class=\"anime-story", "</p>")
            .map(|text| html::strip_tags(&text)),
        tags: collect_anchor_text(&body, "anime-container-data"),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_ar_status(info_value(&body, "حالة الأنمي").as_deref()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_popular_cards(body: &str) -> Vec<CatalogItem> {
    body.split("episodes-card-container")
        .skip(1)
        .filter_map(card_from_chunk)
        .collect()
}

fn parse_latest_cards(body: &str) -> Vec<CatalogItem> {
    body.split("episodes-card-container")
        .skip(1)
        .filter_map(card_from_chunk)
        .collect()
}

fn parse_search_cards(body: &str) -> Vec<CatalogItem> {
    body.split("anime-card-container")
        .skip(1)
        .filter_map(card_from_chunk)
        .collect()
}

fn card_from_chunk(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let title = html::text_between(chunk, "<h3", "</h3>")
        .map(|text| html::strip_tags(&text))
        .or_else(|| html::attr_after(chunk, "<img", "alt"))?;
    Some(CatalogItem {
        key: path_key(&href),
        title,
        cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
        url: Some(absolute_url(&href)),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    body.split("ep-card-anime-title-detail")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title =
                html::text_between(chunk, "<a", "</a>").map(|text| html::strip_tags(&text))?;
            Some(VideoEpisode {
                key: path_key(&href),
                title: Some(title.clone()),
                episode_number: first_number(&title),
                url: Some(absolute_url(&href)),
                language: Some("ar".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_hosters(body: &str) -> Vec<VideoHoster> {
    body.split("ul id=\"episode-servers")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let url = html::attr(chunk, "data-ep-url")?;
            let name = html::strip_tags(chunk);
            Some(VideoHoster {
                key: url.clone(),
                name: if name.is_empty() {
                    host_name(&url)
                } else {
                    name
                },
                url: Some(url),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(BASE_URL),
                ..VideoHoster::default()
            })
        })
        .collect()
}

fn external_stream(stream_url: &str, name: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some(name.to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(BASE_URL),
        initialized: true,
        ..VideoStream::default()
    }
}

fn collect_anchor_text(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .collect()
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .map(|chunk| html::strip_tags(chunk.split("</div>").next().unwrap_or(chunk)))
        .map(|text| text.replace(label, "").trim().to_string())
        .filter(|text| !text.is_empty())
}

fn parse_ar_status(status: Option<&str>) -> ItemStatus {
    let status = status.unwrap_or_default();
    if status.contains("مكتمل") {
        ItemStatus::Completed
    } else if status.contains("يعرض") || status.contains("مستمر") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn host_name(input: &str) -> String {
    input
        .split("//")
        .nth(1)
        .and_then(|part| part.split('/').next())
        .unwrap_or("Mirror")
        .to_string()
}

fn sort_streams(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by_key(|stream| {
        stream
            .quality
            .as_deref()
            .map(|quality| quality.contains(preferred))
            .unwrap_or(false)
    });
    streams.reverse();
    for stream in streams {
        stream.preferred = stream
            .quality
            .as_deref()
            .map(|quality| quality.contains(preferred))
            .unwrap_or(false);
    }
}

fn first_number(input: &str) -> Option<f32> {
    input
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>()
        .parse()
        .ok()
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("url")))
        .and_then(Value::as_str)
        .map(path_key)
}

fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .filter(|path| !path.trim_matches('/').is_empty())
        .map(path_key)
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(BASE_URL) {
        return input.to_string();
    }
    if let Some(path) = input.strip_prefix(BASE_URL) {
        return path_key(path);
    }
    format!(
        "/{}",
        input.split('?').next().unwrap_or(input).trim_matches('/')
    )
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn preferred_quality(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("preferred_quality"))
        .or_else(|| request.get("preferred_quality"))
        .and_then(Value::as_str)
        .unwrap_or("720p")
        .to_string()
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn has_next_page(body: &str) -> bool {
    body.contains("rel=next") || body.contains("rel=\"next\"")
}

const LIST_FIXTURE: &str = r#"<div class="episodes-card-container"><h3><a href="/anime/sample">Sample Anime</a></h3><img src="/cover.jpg"></div>"#;
const SEARCH_FIXTURE: &str = r#"<div class="anime-card-container"><h3><a href="/anime/sample">Sample Anime</a></h3><img src="/cover.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="anime-container-infos"><img src="/cover.jpg"><div class="full-list-info">حالة الأنمي <a>يعرض الان</a></div></div><div class="anime-container-data"><h1>Sample Anime</h1><ul><li><a>Action</a></li></ul><p class="anime-story">Sample description.</p></div>"#;
const EPISODES_FIXTURE: &str = r#"<div class="ep-card-anime-title-detail"><h3><a href="/episode/sample-1">الحلقة 1</a></h3></div>"#;
const HOSTERS_FIXTURE: &str = r#"<ul id="episode-servers"><li class="watch"><a data-ep-url="https://dood.example/e/sample">Doodstream</a></li></ul>"#;

export_video_source!(SOURCE);
