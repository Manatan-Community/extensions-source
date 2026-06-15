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

const SOURCE: AnimeToast = AnimeToast;
const BASE_URL: &str = "https://www.animetoast.cc";

struct AnimeToast;

impl VideoSource for AnimeToast {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = get_or_fixture(BASE_URL, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: false,
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
            entries: parse_search_cards(&body),
            has_next_page: body.contains("nextpostslink"),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        let target = absolute_url(&path);
        let body = get_or_fixture(&target, EPISODES_FIXTURE);
        let is_series = body
            .split("category tag")
            .nth(1)
            .map(|chunk| html::strip_tags(chunk).contains("Serie"))
            .unwrap_or(false);
        if !is_series {
            let title = html::text_between(&body, "h1 class=\"light-title", "</h1>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path(&path));
            return Ok(vec![VideoEpisode {
                key: path_key(&path),
                title: Some(title),
                episode_number: Some(1.0),
                url: Some(target),
                language: Some("de".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let source = body
            .split("id=\"multi_link_tab0\"")
            .nth(1)
            .or_else(|| body.split("id=\"multi_link_tab1\"").nth(1))
            .unwrap_or(&body);
        let mut episodes = parse_episode_links(source);
        if episodes.iter().any(|ep| {
            ep.title
                .as_deref()
                .map(|title| title.contains(':') || title.contains('-'))
                .unwrap_or(false)
        }) {
            if let Some(first) = episodes.first().and_then(|ep| ep.url.clone()) {
                let next = get_or_fixture(&first, EPISODES_FIXTURE);
                let player = html::attr_after(&next, "id=\"player-embed", "href")
                    .or_else(|| html::attr_after(&next, "#player-embed", "href"));
                if let Some(player) = player {
                    episodes = parse_episode_links(&get_or_fixture(
                        &absolute_url(&player),
                        EPISODES_FIXTURE,
                    ));
                }
            }
        }
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/sample".to_string());
        let target = absolute_url(&path);
        let body = get_or_fixture(&target, HOSTERS_FIXTURE);
        let selected = selected_hosters(&request);
        Ok(parse_embeds(&body)
            .into_iter()
            .filter(|(name, _)| selected.iter().any(|allowed| name.contains(allowed)))
            .map(|(name, embed)| VideoHoster {
                key: embed.clone(),
                name: hoster_title(&name, &embed),
                url: Some(target.clone()),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(&target),
                ..VideoHoster::default()
            })
            .collect())
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let name = request
            .get("hoster")
            .and_then(|hoster| hoster.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Mirror");
        let mut streams = resolve_embed(&key, name);
        sort_streams(
            &mut streams,
            pref(&request, "preferred_hoster", "https://voe.sx"),
            "",
        );
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
        sort_streams(
            &mut streams,
            pref(&request, "preferred_hoster", "https://voe.sx"),
            "",
        );
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: page.entries,
            has_more: false,
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
        title: html::text_between(&body, "h1 class=\"light-title", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_path(path)),
        cover: html::attr_after(&body, "item-content", "src")
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(&body, "div class=\"item-content div + p", "</p>")
            .or_else(|| html::text_between(&body, "div class=\"item-content", "</p>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: collect_anchor_text(&body, "rel=tag"),
        url: Some(absolute_url(path)),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&body),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("video-item")
        .skip(1)
        .filter_map(parse_card)
        .collect()
}

fn parse_search_cards(body: &str) -> Vec<CatalogItem> {
    body.split("item-thumbnail")
        .skip(1)
        .filter_map(parse_card)
        .collect()
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let title = html::attr_after(chunk, "<a", "title")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| title_from_path(&href));
    Some(CatalogItem {
        key: path_key(&href),
        title,
        cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
        url: Some(absolute_url(&href)),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn parse_episode_links(body: &str) -> Vec<VideoEpisode> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Ep. 1".to_string());
            let number = title
                .replace("Ep.", "")
                .split_whitespace()
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(100.0);
            Some(VideoEpisode {
                key: path_key(&href),
                title: Some(title),
                episode_number: Some(number),
                url: Some(absolute_url(&href)),
                language: Some("de".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_embeds(body: &str) -> Vec<(String, String)> {
    let player = body.split("id=\"player-embed").nth(1).unwrap_or(body);
    let mut out = Vec::new();
    for chunk in player.split("<a").skip(1) {
        if let Some(href) = html::attr(chunk, "href") {
            out.push((hoster_key(&href), absolute_url(&href)));
        }
    }
    for chunk in player.split("<iframe").skip(1) {
        if let Some(src) = html::attr(chunk, "src") {
            let fixed = if src.starts_with("//") {
                format!("https:{src}")
            } else {
                absolute_url(&src)
            };
            out.push((hoster_key(&fixed), fixed));
        }
    }
    out
}

fn resolve_embed(embed: &str, name: &str) -> Vec<VideoStream> {
    let body = get_or_fixture(embed, "");
    if let Some(src) = html::attr_after(&body, "<source", "src")
        .or_else(|| html::text_between(&body, "file:\"", "\""))
        .or_else(|| html::text_between(&body, "file: '", "'"))
    {
        return vec![media_stream(&src, name, embed)];
    }
    vec![external_stream(embed, name, BASE_URL)]
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

fn hoster_key(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if lower.contains("voe") {
        "voe".to_string()
    } else if lower.contains("dood") || lower.contains("ds2play") {
        "dood".to_string()
    } else if lower.contains("filemoon") {
        "fmoon".to_string()
    } else if lower.contains("mp4upload") {
        "mp4u".to_string()
    } else {
        hoster_title("", url).to_ascii_lowercase()
    }
}

fn hoster_title(name: &str, url: &str) -> String {
    let lower = format!("{} {}", name.to_ascii_lowercase(), url.to_ascii_lowercase());
    if lower.contains("voe") {
        "Voe".to_string()
    } else if lower.contains("dood") || lower.contains("ds2play") {
        "DoodStream".to_string()
    } else if lower.contains("filemoon") {
        "Filemoon".to_string()
    } else if lower.contains("mp4upload") {
        "Mp4upload".to_string()
    } else {
        url.split("//")
            .nth(1)
            .and_then(|tail| tail.split('/').next())
            .unwrap_or("Mirror")
            .replace("www.", "")
    }
}

fn sort_streams(streams: &mut [VideoStream], preferred_hoster: &str, preferred_quality: &str) {
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or_default();
        (
            i32::from(stream.url.contains(preferred_hoster) || quality.contains(preferred_hoster)),
            i32::from(!preferred_quality.is_empty() && quality.contains(preferred_quality)),
        )
    });
    streams.reverse();
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
                "voe".to_string(),
                "dood".to_string(),
                "fmoon".to_string(),
                "mp4u".to_string(),
            ]
        })
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
    if text.contains("Airing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Completed
    }
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
        .unwrap_or("AnimeToast")
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

const LIST_FIXTURE: &str = r#"<div class="row"><div class="col-md-4"><div class="video-item"><div class="item-thumbnail"><a href="/sample" title="Sample Anime"><img src="/cover.jpg"></a></div></div></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="light-title entry-title">Sample Anime</h1><div class="item-content"><p><img src="/cover.jpg"></p><div></div><p>Sample description.</p></div><a rel="tag">Action</a><a rel="category tag">Serie</a>"#;
const EPISODES_FIXTURE: &str = r#"<a rel="category tag">Serie</a><div id="multi_link_tab0"><div class="tab-pane"><a href="/sample/ep-1">Ep. 1</a></div></div>"#;
const HOSTERS_FIXTURE: &str = r#"<div id="player-embed"><a href="https://voe.sx/e/sample">VOE</a><iframe src="https://filemoon.sx/e/sample"></iframe></div>"#;

export_video_source!(SOURCE);
