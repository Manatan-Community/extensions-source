use base64::{Engine, engine::general_purpose::STANDARD};
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

const SOURCE: WitAnime = WitAnime;
const BASE_URL: &str = "https://witanime.cyou";

struct WitAnime;

impl VideoSource for WitAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let page = page(&request);
        let target = if listing == "latest" {
            format!("{BASE_URL}/episode/page/{page}/")
        } else {
            format!("{BASE_URL}/قائمة-الانمي/page/{page}")
        };
        let body = get_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: has_next_page(&body),
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
                "{BASE_URL}/?search_param=animes&s={}",
                manatan_shared::sdk::http::url_encode(query)
            ),
            LIST_FIXTURE,
        );
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
        let body = get_or_fixture(&absolute_url(&path), DETAILS_FIXTURE);
        let body = real_document(&body);
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
        let key = request_key(&request, "hoster").unwrap_or_default();
        let name = request
            .get("hoster")
            .and_then(|hoster| hoster.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Mirror");
        let mut streams = resolve_embed_streams(&key, name);
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
                title: "Anime List".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: self.list(json!({"listing": "popular", "page": 1}))?.entries,
                has_more: true,
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
    let body = real_document(&body);
    CatalogItem {
        key: path_key(path),
        title: html::text_between(&body, "anime-details-title", "</h1>")
            .or_else(|| html::text_between(&body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_path(path)),
        cover: html::attr_after(&body, "img.thumbnail", "src")
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description: description(&body),
        tags: collect_anchor_text(&body, "anime-genres")
            .into_iter()
            .chain(collect_anchor_text(&body, "anime-info"))
            .collect(),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: if body.contains("يعرض الان") {
            ItemStatus::Ongoing
        } else if body.contains("مكتمل") {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        },
        initialized: true,
        ..CatalogItem::default()
    }
}

fn real_document(body: &str) -> String {
    if let Some(link_block) = body.split("anime-page-link").nth(1) {
        if let Some(href) = html::attr_after(link_block, "<a", "href") {
            return get_or_fixture(&href, body);
        }
    }
    body.to_string()
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("anime-card-poster")
        .skip(1)
        .filter_map(|chunk| {
            let href =
                html::attr_after(chunk, "<a", "href").or_else(|| encoded_from_onclick(chunk))?;
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
                language: Some("ar".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    body.split("episodes-card-title")
        .skip(1)
        .filter_map(|chunk| {
            let href =
                encoded_from_onclick(chunk).or_else(|| html::attr_after(chunk, "<a", "href"))?;
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
    body.split("episode-servers")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let target = html::attr(chunk, "data-url")
                .filter(|value| !value.is_empty())
                .and_then(|value| STANDARD.decode(value).ok())
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .or_else(|| encoded_from_onclick(chunk))
                .or_else(|| html::attr(chunk, "href"))?;
            let name = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| hoster_name(&target));
            Some(VideoHoster {
                key: normalize_url(&target),
                name,
                url: Some(normalize_url(&target)),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(BASE_URL),
                ..VideoHoster::default()
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .fold(Vec::new(), |mut out, hoster| {
            if !out
                .iter()
                .any(|item: &VideoHoster| item.name == hoster.name)
            {
                out.push(hoster);
            }
            out
        })
}

fn resolve_embed_streams(embed: &str, name: &str) -> Vec<VideoStream> {
    let embed = normalize_url(embed);
    if embed.contains("yonaplay") || embed.contains("soraplay") && embed.contains("/mirror") {
        let body = get_or_fixture(&embed, "");
        let mut out = Vec::new();
        for chunk in body
            .split(".OD")
            .nth(1)
            .unwrap_or(&body)
            .split("<li")
            .skip(1)
        {
            if let Some(target) = html::attr(chunk, "onclick").and_then(|value| {
                value
                    .split("go_to_player('")
                    .nth(1)
                    .map(|part| part.split("')").next().unwrap_or(part).to_string())
            }) {
                out.extend(resolve_embed_streams(
                    &normalize_url(&target),
                    &hoster_name(&target),
                ));
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    if embed.contains("soraplay") {
        let body = get_or_fixture(&embed, "");
        let data = body
            .split("sources: [")
            .nth(1)
            .and_then(|part| part.split("],").next())
            .unwrap_or_default();
        let streams = data
            .split("\"file\":\"")
            .skip(1)
            .map(|source| {
                let src = normalize_url(source.split('"').next().unwrap_or_default());
                let quality = source
                    .split("\"label\":\"")
                    .nth(1)
                    .and_then(|part| part.split('"').next())
                    .unwrap_or("Soraplay");
                media_stream(&src, "Soraplay", quality, "https://yonaplay.org/")
            })
            .collect::<Vec<_>>();
        if !streams.is_empty() {
            return streams;
        }
    }
    if embed.contains("4shared") {
        let body = get_or_fixture(&embed, "");
        if let Some(src) = html::attr_after(&body, "<source", "src") {
            return vec![media_stream(&src, "4Shared", "mirror", &embed)];
        }
    }
    if embed.contains("dropbox") {
        return vec![external_stream(&embed, "Dropbox", BASE_URL)];
    }
    if embed.contains(".m3u8") {
        return parse_hls(&embed, name, &embed);
    }
    let body = get_or_fixture(&embed, "");
    if let Some(src) = html::attr_after(&body, "<source", "src")
        .or_else(|| html::text_between(&body, "file:\"", "\""))
        .or_else(|| html::text_between(&body, "file: '", "'"))
    {
        if src.contains(".m3u8") {
            return parse_hls(&src, name, &embed);
        }
        return vec![media_stream(&src, name, "direct", &embed)];
    }
    vec![external_stream(&embed, name, BASE_URL)]
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
        name: Some(format!("{name}: {quality}")),
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

fn description(body: &str) -> Option<String> {
    let mut text = String::new();
    for value in body.split("anime-info").skip(1) {
        let info = html::strip_tags(value.split("</div>").next().unwrap_or_default());
        if !info.is_empty() {
            text.push_str(&info);
            text.push('\n');
        }
    }
    if let Some(story) =
        html::text_between(body, "anime-story", "</p>").map(|value| html::strip_tags(&value))
    {
        if !story.is_empty() {
            text.push('\n');
            text.push_str(&story);
        }
    }
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

fn encoded_from_onclick(chunk: &str) -> Option<String> {
    let encoded = html::attr(chunk, "onclick")?
        .split('\'')
        .nth(1)?
        .to_string();
    STANDARD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
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

fn sort_streams(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or_default();
        let digits = quality
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse::<i32>()
            .unwrap_or(0);
        (i32::from(quality.contains(preferred)), digits)
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
        .unwrap_or("1080")
        .to_string()
}

fn hoster_name(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if lower.contains("soraplay") {
        "Soraplay".to_string()
    } else if lower.contains("dood") {
        "Dood".to_string()
    } else if lower.contains("4shared") {
        "4Shared".to_string()
    } else if lower.contains("dailymotion") {
        "Dailymotion".to_string()
    } else if lower.contains("ok.ru") {
        "Okru".to_string()
    } else if lower.contains("mp4upload") {
        "Mp4upload".to_string()
    } else if lower.contains("vidbom") || lower.contains("vidbam") || lower.contains("vadbam") {
        "VidBom".to_string()
    } else {
        input
            .split("://")
            .nth(1)
            .unwrap_or(input)
            .split('/')
            .next()
            .unwrap_or("Mirror")
            .replace("www.", "")
    }
}

fn normalize_url(input: &str) -> String {
    if input.starts_with("//") {
        format!("https:{input}")
    } else {
        input.to_string()
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
        .unwrap_or("WIT ANIME")
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

fn has_next_page(body: &str) -> bool {
    body.contains("pagination") && body.contains("next")
}

const LIST_FIXTURE: &str = r#"<div class="anime-card-poster"><div class="ehover6"><a href="/anime/sample"><img alt="Sample Anime" src="/cover.jpg"></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<img class="thumbnail" src="/cover.jpg"><h1 class="anime-details-title">Sample Anime</h1><ul class="anime-genres"><li><a>Action</a></li></ul><div class="anime-info">حالة الأنمي : يعرض الان</div><p class="anime-story">Sample description.</p><div class="ehover6"><div class="episodes-card-title"><h3><a href="/episode/sample-1">الحلقة 1</a></h3></div></div>"#;
const HOSTERS_FIXTURE: &str = r#"<ul id="episode-servers"><li><a data-url="aHR0cHM6Ly92b2Uuc3gvZS9zYW1wbGU=">Voe - HD</a></li></ul>"#;

export_video_source!(SOURCE);
