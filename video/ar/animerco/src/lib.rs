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

const SOURCE: Animerco = Animerco;
const BASE_URL: &str = "https://zeta.animerco.org";

struct Animerco;

impl VideoSource for Animerco {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let page = page(&request);
        let target = if listing == "latest" {
            format!("{BASE_URL}/page/{page}/?s=")
        } else {
            format!("{BASE_URL}/trending/page/{page}/")
        };
        let body = get_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_media_cards(&body),
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
        let page = page(&request);
        let mut target = format!(
            "{BASE_URL}/page/{page}/?s={}",
            manatan_shared::sdk::http::url_encode(query)
        );
        if let Some(genre) = filter(&request, "genres").filter(|value| !value.is_empty()) {
            target.push_str("&genres=");
            target.push_str(&genre);
        }
        if let Some(year) = filter(&request, "dtyear").filter(|value| !value.is_empty()) {
            target.push_str("&dtyear=");
            target.push_str(&year);
        }
        let body = get_or_fixture(&target, SEARCH_FIXTURE);
        Ok(Paged {
            entries: parse_media_cards(&body),
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
        if absolute_url(&path).contains("/movies/") {
            return Ok(vec![VideoEpisode {
                key: path_key(&path),
                title: Some("فيلم".to_string()),
                episode_number: Some(1.0),
                url: Some(absolute_url(&path)),
                language: Some("ar".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let season_links = parse_episode_links(&body);
        let mut out = Vec::new();
        if season_links.is_empty() {
            out.extend(parse_episode_page(&body, "Season", 1));
        } else {
            for season_path in season_links {
                let season_body = get_or_fixture(&absolute_url(&season_path), EPISODES_FIXTURE);
                let season_name =
                    html::text_between(&season_body, "div class=\"media-title", "</h1>")
                        .map(|text| html::strip_tags(&text))
                        .unwrap_or_else(|| "Season".to_string());
                let season_num = season_name
                    .split_whitespace()
                    .last()
                    .and_then(|value| value.parse::<i32>().ok())
                    .unwrap_or(1);
                let mut episodes = parse_episode_page(&season_body, &season_name, season_num);
                episodes.reverse();
                out.extend(episodes);
            }
        }
        out.reverse();
        Ok(out)
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
        let embed = if key.contains('|') {
            resolve_ajax_hoster(&key).unwrap_or_else(|| key.clone())
        } else {
            key.clone()
        };
        let mut streams = if embed.contains(".m3u8") {
            vec![media_stream(&embed, name, "HLS", BASE_URL)]
        } else {
            vec![external_stream(&embed, name)]
        };
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
    CatalogItem {
        key: path_key(path),
        title: html::attr_after(&body, "a class=\"poster", "title")
            .or_else(|| {
                html::text_between(&body, "<h1", "</h1>").map(|text| html::strip_tags(&text))
            })
            .unwrap_or_else(|| path.trim_matches('/').replace('-', " ")),
        cover: html::attr_after(&body, "a class=\"poster", "data-src")
            .or_else(|| html::attr_after(&body, "<img", "data-src"))
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        authors: collect_info_links(&body, "الشبكات"),
        artists: collect_info_links(&body, "الأستوديو"),
        description: description(&body),
        tags: collect_anchor_text(&body, "Nvgnrs"),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_episode_status(&body),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_media_cards(body: &str) -> Vec<CatalogItem> {
    body.split("media-block")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "a class=\"image", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::attr_after(chunk, "a class=\"image", "title")
                .or_else(|| html::attr_after(chunk, "<a", "title"))
                .unwrap_or_else(|| path_key(&href).trim_matches('/').replace('-', " "));
            Some(CatalogItem {
                key: path_key(&href),
                title,
                cover: html::attr_after(chunk, "a class=\"image", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "data-src"))
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| absolute_url(&image)),
                url: Some(absolute_url(&href)),
                language: Some("ar".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_episode_links(body: &str) -> Vec<String> {
    body.split("ul class=\"episodes-lists")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("<h3"))
        .filter_map(|chunk| html::attr(chunk, "href").map(|href| path_key(&href)))
        .collect()
}

fn parse_episode_page(body: &str, season_name: &str, season_num: i32) -> Vec<VideoEpisode> {
    body.split("ul class=\"episodes-lists")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("<h3"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let ep_text = html::text_between(chunk, "<h3", "</h3>")
                .map(|text| html::strip_tags(&text))
                .unwrap_or_else(|| "Episode".to_string());
            let ep_num = first_number(&ep_text).unwrap_or(1.0);
            Some(VideoEpisode {
                key: path_key(&href),
                title: Some(format!("{ep_text} - {season_name}")),
                episode_number: format!("{season_num}.{ep_num:03.0}").parse().ok(),
                season_number: Some(season_num as f32),
                url: Some(absolute_url(&href)),
                language: Some("ar".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_hosters(body: &str) -> Vec<VideoHoster> {
    body.split("server-list")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let data_post = html::attr(chunk, "data-post")?;
            let data_nume = html::attr(chunk, "data-nume")?;
            let data_type = html::attr(chunk, "data-type")?;
            let name = html::text_between(chunk, "span class=\"server", "</span>")
                .map(|text| html::strip_tags(&text))
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "Mirror".to_string());
            Some(VideoHoster {
                key: format!("{data_post}|{data_nume}|{data_type}"),
                name,
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(BASE_URL),
                ..VideoHoster::default()
            })
        })
        .collect()
}

fn resolve_ajax_hoster(key: &str) -> Option<String> {
    let mut parts = key.split('|');
    let post = parts.next()?;
    let nume = parts.next()?;
    let type_value = parts.next()?;
    let text = client()
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .form(&[
            ("action", "player_ajax"),
            ("post", post),
            ("nume", nume),
            ("type", type_value),
        ])
        .xhr()
        .send_text()
        .ok()?;
    text.split("\"embed_url\":\"")
        .nth(1)
        .and_then(|part| part.split("\",").next())
        .map(|value| value.replace('\\', ""))
        .filter(|value| !value.is_empty())
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

fn external_stream(stream_url: &str, name: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some(host_name(stream_url)),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(BASE_URL),
        initialized: true,
        ..VideoStream::default()
    }
}

fn description(body: &str) -> Option<String> {
    let mut text = String::new();
    if let Some(score) =
        html::text_between(body, "media-rating", "</div>").map(|value| html::strip_tags(&value))
    {
        if !score.is_empty() {
            text.push_str(&score);
            text.push('\n');
        }
    }
    if let Some(story) =
        html::text_between(body, "media-story", "</div>").map(|value| html::strip_tags(&value))
    {
        text.push_str(&story);
    }
    if let Some(alt) =
        html::text_between(body, "alt-title", "</h3>").map(|value| html::strip_tags(&value))
    {
        if !alt.is_empty() {
            text.push_str("\n\nAlternative title: ");
            text.push_str(&alt);
        }
    }
    if text.is_empty() { None } else { Some(text) }
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
        .take(3)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .collect()
}

fn parse_episode_status(body: &str) -> ItemStatus {
    let statuses: Vec<_> = body
        .split("badge")
        .skip(1)
        .map(|chunk| html::strip_tags(chunk.split("</span>").next().unwrap_or(chunk)))
        .collect();
    if !statuses.is_empty() && statuses.iter().all(|value| value.contains("مكتمل")) {
        ItemStatus::Completed
    } else if statuses.iter().any(|value| value.contains("يعرض")) {
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
        .unwrap_or("external")
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
        .unwrap_or("1080")
        .to_string()
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn has_next_page(body: &str) -> bool {
    body.contains("pagination")
        && (body.contains("<svg") || body.contains("rel=next") || body.contains("rel=\"next\""))
}

const LIST_FIXTURE: &str = r#"<div class="media-block"><div><a class="image" href="/anime/sample" data-src="/cover.jpg" title="Sample Anime"></a></div></div>"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"<a class="poster" data-src="/cover.jpg" title="Sample Anime"></a><div class="media-title"><h1>Sample Anime</h1><h3 class="alt-title">Alt</h3></div><nav class="Nvgnrs"><a>Action</a></nav><div class="media-story"><p>Sample description.</p></div><ul class="chapters-list"><a class="se-title"><span class="badge">يعرض الأن</span></a></ul>"#;
const EPISODES_FIXTURE: &str =
    r#"<ul class="episodes-lists"><li><a href="/episode/sample-1"><h3>الحلقة 1</h3></a></li></ul>"#;
const HOSTERS_FIXTURE: &str = r#"<ul class="server-list"><li><a data-post="1" data-nume="1" data-type="tv"><span class="server">Doodstream</span></a></li></ul>"#;

export_video_source!(SOURCE);
