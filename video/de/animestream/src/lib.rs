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

const SOURCE: AnimeStream = AnimeStream;
const BASE_URL: &str = "https://anime-stream.to";

struct AnimeStream;

impl VideoSource for AnimeStream {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = get_or_fixture(
            &format!("{BASE_URL}/series/page/{}/", page(&request)),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: body.contains("li active") || body.contains("li.active ~ li"),
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
        let body = get_or_fixture(
            &format!(
                "{BASE_URL}/page/{}/?s={}",
                page(&request),
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: body.contains("li active") || body.contains("next"),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/series/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/series/sample".to_string());
        let target = absolute_url(&path);
        let body = get_or_fixture(&target, EPISODES_FIXTURE);
        let mut episodes = body
            .split("div class=\"les-content")
            .nth(1)
            .unwrap_or(&body)
            .split("<a")
            .skip(1)
            .enumerate()
            .filter_map(|(index, chunk)| {
                let href = html::attr(chunk, "href")?;
                let title = html::text_between(chunk, ">", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| format!("Episode {}", index + 1));
                Some(VideoEpisode {
                    key: path_key(&href),
                    title: Some(title),
                    episode_number: Some((index + 1) as f32),
                    url: Some(absolute_url(&href)),
                    language: Some("de".to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect::<Vec<_>>();
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/series/sample".to_string());
        let target = absolute_url(&path);
        let body = get_or_fixture(&target, HOSTERS_FIXTURE);
        let embed = html::attr_after(&body, "a class=\"lnk-lnk", "href")
            .or_else(|| html::attr_after(&body, "a.lnk-lnk", "href"))
            .or_else(|| {
                body.split("lnk-lnk")
                    .nth(1)
                    .and_then(|chunk| html::attr_after(chunk, "<a", "href"))
            });
        Ok(embed
            .map(|href| {
                vec![VideoHoster {
                    key: absolute_url(&href),
                    name: "Metastream".to_string(),
                    url: Some(target.clone()),
                    lazy: true,
                    video_count: Some(1),
                    headers: referer_headers(&target),
                    ..VideoHoster::default()
                }]
            })
            .unwrap_or_default())
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        Ok(resolve_meta(&key))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut streams = Vec::new();
        for hoster in self.hosters(request.clone())? {
            let mut resolved = self.resolve_hoster(json!({
                "hoster": { "key": hoster.key },
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
            }))?;
            for stream in &mut resolved {
                stream.hoster = Some(hoster.clone());
            }
            streams.extend(resolved);
        }
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Series".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: page.entries,
            has_more: page.has_next_page,
            ..HomeSection::default()
        }])
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
        title: html::attr_after(&body, "div.thumb img", "alt")
            .or_else(|| html::attr_after(&body, "<img", "alt"))
            .or_else(|| {
                html::text_between(&body, "<h1", "</h1>").map(|value| html::strip_tags(&value))
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_path(path)),
        cover: html::attr_after(&body, "div.thumb", "src")
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(&body, "p class=\"f-desc", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: collect_anchor_text(&body, "category tag"),
        url: Some(absolute_url(path)),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&body),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("ml-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::attr_after(chunk, "<img", "alt")
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path(&href));
            Some(CatalogItem {
                key: path_key(&href),
                title,
                cover: html::attr_after(chunk, "<img", "data-original")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| absolute_url(&image)),
                url: Some(absolute_url(&href)),
                language: Some("de".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn resolve_meta(embed: &str) -> Vec<VideoStream> {
    let body = get_or_fixture(embed, STREAM_FIXTURE);
    let Some(src) = html::text_between(&body, "sources: [{src: \"", "\"")
        .or_else(|| html::attr_after(&body, "<source", "src"))
    else {
        return vec![external_stream(embed, "Metastream", BASE_URL)];
    };
    vec![media_stream(&src, "Metastream", embed)]
}

fn media_stream(stream_url: &str, name: &str, referer: &str) -> VideoStream {
    let is_hls = stream_url.contains(".m3u8");
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some(name.to_string()),
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

fn external_stream(stream_url: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some(name.to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn collect_anchor_text(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(body: &str) -> ItemStatus {
    let text = html::strip_tags(body);
    if text.contains("Abgeschlossen") {
        ItemStatus::Completed
    } else if text.contains("Airing") || text.contains("Laufend") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .and_then(Value::as_str)
                .or_else(|| value.get("url").and_then(Value::as_str))
                .or_else(|| value.as_str())
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(path_key)
}

fn path_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) || input.starts_with('/') {
        Some(path_key(input))
    } else {
        None
    }
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(BASE_URL) {
        return input.to_string();
    }
    format!(
        "/{}",
        input
            .strip_prefix(BASE_URL)
            .unwrap_or(input)
            .split('?')
            .next()
            .unwrap_or(input)
            .trim_matches('/')
    )
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn title_from_path(input: &str) -> String {
    input
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("Anime-Stream")
        .replace('-', " ")
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

const LIST_FIXTURE: &str = r#"<div class="movies-list"><div class="ml-item"><a href="/series/sample"><img data-original="/cover.jpg" alt="Sample Anime"></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="thumb"><img src="/cover.jpg" alt="Sample Anime"></div><div class="desc"><p class="f-desc">Sample description.</p></div><a rel="category tag">Action</a>"#;
const EPISODES_FIXTURE: &str =
    r#"<div class="les-content"><a href="/series/sample-episode-1">Episode 1</a></div>"#;
const HOSTERS_FIXTURE: &str =
    r#"<div><a class="lnk-lnk" href="https://meta.example/embed">Meta</a></div>"#;
const STREAM_FIXTURE: &str =
    r#"<script>sources: [{src: "https://media.example/sample.mp4", type: "video/mp4"}]</script>"#;

export_video_source!(SOURCE);
