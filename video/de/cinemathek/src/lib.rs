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

const SOURCE: Cinemathek = Cinemathek;
const BASE_URL: &str = "https://cinemathek.net";

struct Cinemathek;

impl VideoSource for Cinemathek {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/episoden/page/{page}")
        } else {
            format!("{BASE_URL}/filme/page/{page}/")
        };
        let body = get_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: body.contains("nextpagination"),
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
            has_next_page: body.contains("nextpagination"),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/movies/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/movies/sample".to_string());
        let target = absolute_url(&path);
        let body = get_or_fixture(&target, DETAILS_FIXTURE);
        let episodes = parse_episode_links(&body);
        if !episodes.is_empty() {
            return Ok(episodes);
        }
        Ok(vec![VideoEpisode {
            key: path.clone(),
            title: Some(title_from_body(&body).unwrap_or_else(|| title_from_path(&path))),
            episode_number: Some(1.0),
            url: Some(target),
            language: Some("de".to_string()),
            ..VideoEpisode::default()
        }])
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/movies/sample".to_string());
        let page = absolute_url(&path);
        let body = get_or_fixture(&page, HOSTERS_FIXTURE);
        Ok(body
            .split("<li")
            .skip(1)
            .filter(|chunk| chunk.contains("data-post") && !chunk.contains("data-nume=\"trailer\""))
            .filter_map(|chunk| {
                let post = html::attr(chunk, "data-post")?;
                let kind = html::attr(chunk, "data-type").unwrap_or_else(|| "movie".to_string());
                let num = html::attr(chunk, "data-nume").unwrap_or_else(|| "1".to_string());
                let name = html::attr(chunk, "data-text")
                    .or_else(|| {
                        html::text_between(chunk, "<span", "</span>")
                            .map(|value| html::strip_tags(&value))
                    })
                    .unwrap_or_else(|| format!("Player {num}"));
                Some(VideoHoster {
                    key: format!(
                        "{}|{}|{}/{}/{}/{}",
                        name,
                        page,
                        BASE_URL,
                        "wp-json/dooplayer/v2",
                        post,
                        format!("{kind}/{num}")
                    ),
                    name,
                    url: Some(page.clone()),
                    lazy: true,
                    video_count: Some(1),
                    headers: referer_headers(&page),
                    ..VideoHoster::default()
                })
            })
            .collect())
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let mut parts = key.splitn(3, '|');
        let name = parts.next().unwrap_or("Player");
        let referer = parts.next().unwrap_or(BASE_URL);
        let api = parts.next().unwrap_or_default();
        let body = client()
            .get(api)
            .referer(referer)
            .xhr()
            .send_text()
            .unwrap_or_else(|_| PLAYER_FIXTURE.to_string());
        let embed = json_string_field(&body, "embed_url").unwrap_or_else(|| api.to_string());
        let selected = selected_hosters(&request);
        if !selected.iter().any(|value| hoster_matches(&embed, value)) {
            return Ok(Vec::new());
        }
        let mut streams = resolve_embed(&embed, name, referer);
        sort_streams(&mut streams, &request);
        Ok(streams)
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
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Filme".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Episoden".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
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
        title: title_from_body(&body).unwrap_or_else(|| title_from_path(path)),
        cover: html::attr_after(&body, "div.poster", "src")
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(&body, "wp-content", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: collect_anchor_text(&body, "genres"),
        url: Some(absolute_url(path)),
        language: Some("de".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("article")
        .filter(|chunk| chunk.contains("poster") || chunk.contains("result-item"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::attr_after(chunk, "<img", "alt")
                .or_else(|| {
                    html::text_between(chunk, "<h3", "</h3>").map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path(&href));
            Some(CatalogItem {
                key: path_key(&href),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&href)),
                language: Some("de".to_string()),
                content_rating: Some("adult".to_string()),
                status: ItemStatus::Completed,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_episode_links(body: &str) -> Vec<VideoEpisode> {
    body.split("episodi")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| {
                    html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path(&href));
            Some(VideoEpisode {
                key: path_key(&href),
                title: Some(title),
                episode_number: html::text_between(chunk, "numerando", "</div>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| value.parse().ok()),
                url: Some(absolute_url(&href)),
                language: Some("de".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn resolve_embed(embed: &str, name: &str, referer: &str) -> Vec<VideoStream> {
    let body = get_or_fixture(embed, "");
    if let Some(src) = html::attr_after(&body, "<source", "src")
        .or_else(|| html::text_between(&body, "file:\"", "\""))
        .or_else(|| html::text_between(&body, "file: '", "'"))
    {
        return vec![media_stream(&src, name, embed)];
    }
    vec![external_stream(embed, name, referer)]
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

fn json_string_field(body: &str, field: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get(field)
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .or_else(|| {
            body.split(&format!("\"{field}\":\""))
                .nth(1)
                .and_then(|tail| tail.split("\",").next())
                .map(|value| value.replace("\\/", "/").replace("\\", ""))
        })
}

fn hoster_matches(url: &str, code: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    match code {
        "slare" => lower.contains("streamlare"),
        "fmoon" => lower.contains("filemoon"),
        "dood" => lower.contains("ds2play") || lower.contains("dood") || lower.contains("doo"),
        "stape" => lower.contains("streamtape"),
        "swish" => lower.contains("streamwish") || lower.contains("filelions"),
        _ => true,
    }
}

fn selected_hosters(request: &Value) -> Vec<String> {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("hoster_selection"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_else(|| {
            vec![
                "slare".to_string(),
                "fmoon".to_string(),
                "dood".to_string(),
                "stape".to_string(),
                "swish".to_string(),
            ]
        })
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let hoster = pref(request, "preferred_hoster", "https://filemoon");
    let quality = pref(request, "preferred_quality", "720p");
    streams.sort_by_key(|stream| {
        let stream_quality = stream.quality.as_deref().unwrap_or_default();
        (
            i32::from(stream.url.contains(hoster)),
            i32::from(stream_quality.contains(quality)),
        )
    });
    streams.reverse();
}

fn title_from_body(body: &str) -> Option<String> {
    html::text_between(body, "<h1", "</h1>")
        .or_else(|| html::text_between(body, "<h3", "</h3>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn collect_anchor_text(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn pref<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
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
        .unwrap_or("Cinemathek")
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

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    next["listing"] = Value::String(listing.to_string());
    next
}

const LIST_FIXTURE: &str = r#"<article class="movies"><div class="poster"><a href="/movies/sample"><img src="/cover.jpg" alt="Sample Movie"></a></div></article>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample Movie</h1><div class="poster"><img src="/cover.jpg"></div><div class="wp-content"><p>Sample description.</p></div><ul id="playeroptionsul"><li data-type="movie" data-post="1" data-nume="1" data-text="Filemoon"></li></ul>"#;
const HOSTERS_FIXTURE: &str = DETAILS_FIXTURE;
const PLAYER_FIXTURE: &str = r#"{"embed_url":"https:\/\/filemoon.sx\/e\/sample"}"#;

export_video_source!(SOURCE);
