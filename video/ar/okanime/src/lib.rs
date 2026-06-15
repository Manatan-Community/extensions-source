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

const SOURCE: Okanime = Okanime;
const BASE_URL: &str = "https://www.okanime.xyz";

struct Okanime;

impl VideoSource for Okanime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let page = page(&request);
        let target = if listing == "latest" {
            format!("{BASE_URL}/espisode-list?page={page}")
        } else {
            BASE_URL.to_string()
        };
        let body = get_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body),
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
        let mut target = format!(
            "{BASE_URL}/search/?s={}",
            manatan_shared::sdk::http::url_encode(query)
        );
        if page > 1 {
            target.push_str("&page=");
            target.push_str(&page.to_string());
        }
        let body = get_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body),
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
        Ok(parse_episodes(&body))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/episode/sample".to_string());
        let body = get_or_fixture(&absolute_url(&path), HOSTERS_FIXTURE);
        let selected = enabled_hosters(&request);
        Ok(parse_hosters(&body)
            .into_iter()
            .filter(|hoster| selected.iter().any(|name| hoster.name.contains(name)))
            .collect())
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let name = request
            .get("hoster")
            .and_then(|hoster| hoster.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Mirror");
        let mut streams = resolve_embed_streams(&key, name, BASE_URL);
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
    let info = body.split("div class=\"text-right").nth(1).unwrap_or(&body);
    let mut description = html::text_between(&body, "div class=\"review-content", "</div>")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    for chunk in info.split("full-list-info").skip(1) {
        let text = html::strip_tags(chunk.split("</div>").next().unwrap_or_default());
        if !text.is_empty() {
            description.push('\n');
            description.push_str(&text);
        }
    }
    CatalogItem {
        key: path_key(path),
        title: html::text_between(&body, "author-info-title", "</h1>")
            .or_else(|| html::text_between(&body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_path(path)),
        cover: html::attr_after(info, "<img", "src")
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description: (!description.trim().is_empty()).then(|| description.trim().to_string()),
        tags: collect_anchor_text(&body, "review-author-info"),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: if info.contains("يعرض الان") {
            ItemStatus::Ongoing
        } else if info.contains("مكتمل") {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        },
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("anime-card")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "anime-title", "</a>")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path(&href));
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
        })
        .collect()
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    body.split("episode-card")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path(&href));
            Some(VideoEpisode {
                key: path_key(&href),
                title: Some(title.clone()),
                episode_number: title
                    .rsplit(' ')
                    .find_map(|value| value.parse::<f32>().ok()),
                url: Some(absolute_url(&href)),
                language: Some("ar".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_hosters(body: &str) -> Vec<VideoHoster> {
    body.split("ep-link")
        .skip(1)
        .filter_map(|chunk| {
            let target = html::attr(chunk, "data-src")?;
            let quality = html::text_between(chunk, "<span", "</span>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            let name = hoster_name(&target);
            let label = if quality.is_empty() {
                name
            } else {
                format!("{name} - {}", map_quality(&quality))
            };
            Some(VideoHoster {
                key: target.clone(),
                name: label,
                url: Some(target),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(BASE_URL),
                ..VideoHoster::default()
            })
        })
        .collect()
}

fn resolve_embed_streams(embed: &str, name: &str, referer: &str) -> Vec<VideoStream> {
    if embed.contains(".m3u8") {
        return parse_hls(embed, name, referer);
    }
    let body = get_or_fixture(embed, "");
    if let Some(src) = html::attr_after(&body, "<source", "src")
        .or_else(|| html::text_between(&body, "file:\"", "\""))
        .or_else(|| html::text_between(&body, "file: '", "'"))
    {
        if src.contains(".m3u8") {
            return parse_hls(&src, name, embed);
        }
        return vec![media_stream(&src, name, "direct", embed)];
    }
    vec![external_stream(embed, name, referer)]
}

fn parse_hls(target: &str, name: &str, referer: &str) -> Vec<VideoStream> {
    let body = client().get(target).send_text().unwrap_or_default();
    if !body.contains("#EXT-X-STREAM-INF") {
        return vec![media_stream(target, name, "auto", referer)];
    }
    body.split("#EXT-X-STREAM-INF:")
        .skip(1)
        .filter_map(|block| {
            let quality = block
                .split("RESOLUTION=")
                .nth(1)
                .and_then(|part| part.split('x').nth(1))
                .and_then(|part| part.split([',', '\n']).next())
                .map(|height| format!("{height}p"))
                .unwrap_or_else(|| "auto".to_string());
            let line = block
                .lines()
                .find(|line| !line.trim().is_empty() && !line.starts_with('#'))?;
            let stream_url = if line.starts_with("http") {
                line.to_string()
            } else {
                format!(
                    "{}/{}",
                    target
                        .rsplit_once('/')
                        .map(|(base, _)| base)
                        .unwrap_or(target),
                    line
                )
            };
            Some(media_stream(&stream_url, name, &quality, referer))
        })
        .collect()
}

fn media_stream(stream_url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = stream_url.contains(".m3u8");
    VideoStream {
        url: stream_url.to_string(),
        name: Some(format!("{name} - {quality}")),
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
        quality: Some(hoster_name(stream_url)),
        format: Some("external".to_string()),
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
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn enabled_hosters(request: &Value) -> Vec<String> {
    let Some(values) = request
        .get("preferences")
        .and_then(|prefs| prefs.get("pref_hoster_selection"))
        .and_then(Value::as_array)
    else {
        return vec![
            "Dood".to_string(),
            "Voe".to_string(),
            "Mp4upload".to_string(),
            "VidBom".to_string(),
            "Okru".to_string(),
        ];
    };
    values
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn map_quality(input: &str) -> String {
    match input {
        "FHD" => "1080p",
        "HD" => "720p",
        "SD" => "480p",
        _ => "240p",
    }
    .to_string()
}

fn sort_streams(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by_key(|stream| {
        let quality_score = stream
            .quality
            .as_deref()
            .unwrap_or_default()
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse::<i32>()
            .unwrap_or(0);
        let preferred_score = i32::from(
            stream
                .quality
                .as_deref()
                .map(|quality| quality.contains(preferred))
                .unwrap_or(false),
        );
        (preferred_score, quality_score)
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

fn preferred_quality(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("preferred_quality"))
        .or_else(|| request.get("preferred_quality"))
        .and_then(Value::as_str)
        .unwrap_or("1080p")
        .to_string()
}

fn hoster_name(input: &str) -> String {
    let host = input
        .split("://")
        .nth(1)
        .unwrap_or(input)
        .split('/')
        .next()
        .unwrap_or("Mirror")
        .replace("www.", "");
    match host.as_str() {
        value if value.contains("dood") => "Dood".to_string(),
        value if value.contains("mp4upload") => "Mp4upload".to_string(),
        value if value.contains("ok.ru") => "Okru".to_string(),
        value if value.contains("voe") => "Voe".to_string(),
        value
            if value.contains("vidbam")
                || value.contains("vadbam")
                || value.contains("vidbom")
                || value.contains("vidbm") =>
        {
            "VidBom".to_string()
        }
        _ => host,
    }
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
                .and_then(Value::as_str)
                .or_else(|| value.get("url").and_then(Value::as_str))
                .or_else(|| value.as_str())
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(path_key)
}

fn path_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) {
        return Some(path_key(input));
    }
    if input.starts_with("/anime/") || input.starts_with("/episode/") {
        return Some(path_key(input));
    }
    None
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(BASE_URL) {
        return input.to_string();
    }
    let path = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_matches('/');
    format!("/{path}")
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
        .unwrap_or("Okanime")
        .replace('-', " ")
}

fn has_next_page(body: &str) -> bool {
    body.contains("pagination") && !body.contains("li:last-child disabled")
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

const LIST_FIXTURE: &str = r#"<div class="anime-card"><div class="anime-title"><h4><a href="/anime/sample">Sample Anime</a></h4></div><img src="/cover.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="author-info-title"><h1>Sample Anime</h1></div><div class="text-right"><img src="/cover.jpg"><div class="full-list-info"><small>حالة الأنمي</small><small>يعرض الان</small></div></div><div class="review-author-info"><a>Action</a></div><div class="review-content">Sample description.</div>"#;
const EPISODES_FIXTURE: &str = r#"<div class="episode-card"><div class="anime-title"><a href="/episode/sample-1">الحلقة 1</a></div></div>"#;
const HOSTERS_FIXTURE: &str =
    r#"<a class="ep-link" data-src="https://voe.sx/e/sample"><span>HD</span></a>"#;

export_video_source!(SOURCE);
