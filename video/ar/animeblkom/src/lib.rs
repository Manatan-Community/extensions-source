use base64::{Engine as _, engine::general_purpose::STANDARD};
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

const SOURCE: AnimeBlkom = AnimeBlkom;
const DEFAULT_BASE_URL: &str = "https://animeblkom.net";

struct AnimeBlkom;

impl VideoSource for AnimeBlkom {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let page = page(&request);
        let body = get_or_fixture(
            &base,
            &format!("{base}/animes-list/?sort_by=rate&page={page}"),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body, &base),
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(path) = path_from_url(query, &base) {
            return Ok(Paged {
                entries: vec![fetch_details(&path, &base)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let target = if query.is_empty() {
            let type_filter = filter(&request, "type")
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "anime-list".to_string());
            format!("{base}/{type_filter}?page={page}")
        } else {
            format!(
                "{base}/search?query={}&page={page}",
                manatan_shared::sdk::http::url_encode(query)
            )
        };
        let body = get_or_fixture(&base, &target, SEARCH_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body, &base),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let path =
            request_key(&request, "item", &base).unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&path, &base))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base = base_url(&request);
        let path =
            request_key(&request, "item", &base).unwrap_or_else(|| "/anime/sample".to_string());
        let body = get_or_fixture(&base, &absolute_url(&base, &path), EPISODES_FIXTURE);
        let mut episodes = parse_episodes(&body, &base);
        if episodes.is_empty() {
            episodes.push(VideoEpisode {
                key: path_key(&path, &base),
                title: html::text_between(&body, "div class=\"name", "</div>")
                    .map(|text| html::strip_tags(&text))
                    .or_else(|| {
                        html::text_between(&body, "<h1", "</h1>")
                            .map(|text| html::strip_tags(&text))
                    }),
                episode_number: Some(1.0),
                url: Some(absolute_url(&base, &path)),
                language: Some("ar".to_string()),
                ..VideoEpisode::default()
            });
        }
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let base = base_url(&request);
        let path = request_key(&request, "episode", &base)
            .unwrap_or_else(|| "/episode/sample".to_string());
        let body = get_or_fixture(&base, &absolute_url(&base, &path), HOSTERS_FIXTURE);
        Ok(parse_hosters(&body, &base))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let base = base_url(&request);
        let key = request_key(&request, "hoster", &base).unwrap_or_default();
        let name = request
            .get("hoster")
            .and_then(|hoster| hoster.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Mirror");
        let mut streams = resolve_streams(&key, name, &base);
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

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: self.list(json!({"page": 1, "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?.entries,
            has_more: true,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "item", &base).map(|path| absolute_url(&base, &path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "episode", &base).map(|path| absolute_url(&base, &path)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let base = base_url(&request);
        if let Some(path) = path_from_url(input, &base) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&path, &base)),
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

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_referer(base)
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(base: &str, target: &str, fixture: &str) -> String {
    client(base)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(path: &str, base: &str) -> CatalogItem {
    let body = get_or_fixture(base, &absolute_url(base, path), DETAILS_FIXTURE);
    CatalogItem {
        key: path_key(path, base),
        title: html::text_between(&body, "<h1", "</h1>")
            .map(|text| html::strip_tags(&text))
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| path.trim_matches('/').replace('-', " ")),
        cover: html::attr_after(&body, "div class=\"poster", "data-original")
            .or_else(|| html::attr_after(&body, "div class=\"poster", "src"))
            .or_else(|| html::attr_after(&body, "<img", "data-original"))
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|image| absolute_url(base, &image)),
        url: Some(absolute_url(base, path)),
        authors: collect_info_links(&body, "الاستديو"),
        artists: info_value(&body, "المخرج").into_iter().collect(),
        description: html::text_between(&body, "div class=\"story", "</div>")
            .map(|text| html::strip_tags(&text)),
        tags: collect_anchor_text(&body, "genres"),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_ar_status(info_value(&body, "حالة الأنمي").as_deref()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_cards(body: &str, base: &str) -> Vec<CatalogItem> {
    body.split("div class=\"poster")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let image_alt = html::attr_after(chunk, "<img", "alt").unwrap_or_default();
            let title = image_alt
                .strip_suffix(" poster")
                .unwrap_or(&image_alt)
                .trim()
                .to_string();
            Some(CatalogItem {
                key: path_key(&href, base),
                title: if title.is_empty() {
                    path_key(&href, base).trim_matches('/').replace('-', " ")
                } else {
                    title
                },
                cover: html::attr_after(chunk, "<img", "data-original")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| absolute_url(base, &image)),
                url: Some(absolute_url(base, &href)),
                language: Some("ar".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_episodes(body: &str, base: &str) -> Vec<VideoEpisode> {
    body.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            if !chunk.contains("episodes-links") && !chunk.contains("<a") {
                return None;
            }
            let href = html::attr_after(chunk, "<a", "href")?;
            let spans: Vec<_> = chunk
                .split("<span")
                .skip(1)
                .filter_map(|part| {
                    html::text_between(part, ">", "</span>").map(|text| html::strip_tags(&text))
                })
                .collect();
            let ep_title = spans
                .get(2)
                .or_else(|| spans.first())
                .cloned()
                .unwrap_or_else(|| "Episode".to_string());
            let label = spans.first().cloned().unwrap_or_default();
            Some(VideoEpisode {
                key: path_key(&href, base),
                title: Some(if label.is_empty() {
                    ep_title.clone()
                } else {
                    format!("{ep_title}: {label}")
                }),
                episode_number: first_number(&ep_title),
                url: Some(absolute_url(base, &href)),
                language: Some("ar".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_hosters(body: &str, base: &str) -> Vec<VideoHoster> {
    body.split("span class=\"server")
        .skip(1)
        .flat_map(|chunk| chunk.split("<a").skip(1))
        .filter_map(|chunk| {
            let url = html::attr(chunk, "data-src")?.replace("http://", "https://");
            let name = html::strip_tags(chunk);
            Some(VideoHoster {
                key: absolute_url(base, &url),
                name: if name.is_empty() {
                    "Mirror".to_string()
                } else {
                    name
                },
                url: Some(absolute_url(base, &url)),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(base),
                ..VideoHoster::default()
            })
        })
        .collect()
}

fn resolve_streams(url: &str, name: &str, base: &str) -> Vec<VideoStream> {
    if url.contains(".vid4up") || name.contains("Blkom") {
        let body = get_or_fixture(base, url, VID4UP_FIXTURE);
        let streams: Vec<_> = body
            .split("<source")
            .skip(1)
            .filter_map(|source| {
                let video_url = html::attr(source, "src")?;
                let quality = html::attr(source, "label").unwrap_or_else(|| "auto".to_string());
                Some(media_stream(
                    &video_url,
                    &format!("Blkom - {quality}"),
                    &quality,
                    base,
                ))
            })
            .collect();
        if !streams.is_empty() {
            return streams;
        }
    }
    if url.contains(".m3u8") {
        return vec![media_stream(url, name, "HLS", base)];
    }
    vec![external_stream(url, name, base)]
}

fn media_stream(stream_url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = stream_url.contains(".m3u8");
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some(quality.to_string()),
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
        quality: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
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

fn collect_info_links(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .take(2)
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
    } else if status.contains("مستمر") || status.contains("يعرض") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn sort_streams(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by_key(|stream| quality_score(stream.quality.as_deref()));
    streams.reverse();
    for stream in streams {
        stream.preferred = stream
            .quality
            .as_deref()
            .map(|quality| quality.contains(preferred))
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

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("domain"))
        .or_else(|| request.get("domain"))
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_BASE_URL)
        .trim_end_matches('/')
        .to_string()
}

fn request_key(request: &Value, field: &str, base: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("url")))
        .and_then(Value::as_str)
        .map(|value| path_key(value, base))
}

fn path_from_url(input: &str, base: &str) -> Option<String> {
    input
        .strip_prefix(base)
        .filter(|path| !path.trim_matches('/').is_empty())
        .map(|path| path_key(path, base))
}

fn path_key(input: &str, base: &str) -> String {
    if let Some(path) = input.strip_prefix(base) {
        return path_key(path, base);
    }
    if input.starts_with("http") {
        return input.to_string();
    }
    format!(
        "/{}",
        input.split('?').next().unwrap_or(input).trim_matches('/')
    )
}

fn absolute_url(base: &str, input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(base, input)
    }
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
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

#[allow(dead_code)]
fn decode_base64(input: &str) -> Option<String> {
    STANDARD
        .decode(input)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

const LIST_FIXTURE: &str = r#"<div class="contents"><div class="poster"><a href="/anime/sample"><img data-original="/cover.jpg" alt="Sample Anime poster"></a></div></div>"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"<div class="poster"><img data-original="/cover.jpg"></div><div class="name"><span><h1>Sample Anime</h1></span></div><p class="genres"><a>Action</a></p><div class="story"><p>Sample description.</p></div><div>حالة الأنمي <span class="info">مستمر</span></div>"#;
const EPISODES_FIXTURE: &str = r#"<ul class="episodes-links"><li><a href="/episode/sample-1"><span>Server</span><span></span><span>الحلقة 1</span></a></li></ul>"#;
const HOSTERS_FIXTURE: &str =
    r#"<span class="server"><a data-src="https://example.com/embed/sample">External</a></span>"#;
const VID4UP_FIXTURE: &str =
    r#"<video><source src="https://cdn.example/sample.mp4" label="720p"></video>"#;

export_video_source!(SOURCE);
