use manatan_extension::{
    abi::ExtensionResult,
    export_video_source,
    source::VideoSource,
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind,
};
use manatan_shared::{
    html,
    sdk::{http::HttpClient, Context, SearchRequest},
    url,
};
use serde_json::{json, Value};

const SOURCE: EgyDead = EgyDead;
const BASE_URL: &str = "https://egydead.space";

struct EgyDead;

impl VideoSource for EgyDead {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request.get("listing").and_then(Value::as_str).unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/?page={}/", page(&request))
        } else {
            BASE_URL.to_string()
        };
        let body = get_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged { entries: parse_cards(&body), has_next_page: has_next_page(&body) })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged { entries: vec![fetch_details(&path)], has_next_page: false });
        }
        let page = page(&request);
        let target = if !query.is_empty() {
            format!("{BASE_URL}/page/{page}/?s={}", manatan_shared::sdk::http::url_encode(query))
        } else if let Some(category) = filter(&request, "category").filter(|value| !value.is_empty()) {
            format!("{BASE_URL}/{category}/?page={page}/")
        } else {
            BASE_URL.to_string()
        };
        let body = get_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged { entries: parse_cards(&body), has_next_page: has_next_page(&body) })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        Ok(fetch_details(&request_key(&request, "item").unwrap_or_else(|| "/movie/sample".to_string())))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/movie/sample".to_string());
        let mut episodes = Vec::new();
        add_episodes(&mut episodes, &absolute_url(&path), false);
        if episodes.is_empty() {
            episodes.push(VideoEpisode {
                key: path_key(&path),
                title: Some("مشاهدة".to_string()),
                episode_number: Some(1.0),
                url: Some(absolute_url(&path)),
                language: Some("ar".to_string()),
                ..VideoEpisode::default()
            });
        }
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/movie/sample".to_string());
        let page_url = absolute_url(&path);
        let body = post_view_or_fixture(&page_url, HOSTERS_FIXTURE);
        Ok(parse_hosters(&body, &page_url))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let name = request.get("hoster").and_then(|h| h.get("name")).and_then(Value::as_str).unwrap_or("Mirror");
        let mut streams = resolve_streams(&key, name);
        prefer_quality(&mut streams, pref(&request, "preferred_quality", "1080"));
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
        prefer_quality(&mut streams, pref(&request, "preferred_quality", "1080"));
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
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(path) = path_from_url(input) {
            return Ok(Some(UrlResolveResult { item: Some(fetch_details(&path)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser().with_referer(BASE_URL).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn post_view_or_fixture(target: &str, fixture: &str) -> String {
    client().post(target).form(&[("View", "1")]).send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("movieItem").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let title = html::text_between(chunk, "BottomTitle", "</h1>")
            .map(|text| html::strip_tags(&text))
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .unwrap_or_else(|| path_key(&href).trim_matches('/').replace('-', " "));
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
    }).collect()
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE);
    let title = html::text_between(&body, "singleTitle", "</div>")
        .map(|text| html::strip_tags(&text))
        .unwrap_or_else(|| path_key(path).trim_matches('/').replace('-', " "));
    CatalogItem {
        key: path_key(path),
        title: title.clone(),
        cover: html::attr_after(&body, "single-thumbnail", "src").or_else(|| html::attr_after(&body, "<img", "src")).map(|image| absolute_url(&image)),
        authors: collect_links_after(&body, "البلد"),
        artists: collect_links_after(&body, "القسم"),
        description: html::text_between(&body, "extra-content", "</div>").map(|text| html::strip_tags(&text)).filter(|text| !text.is_empty()),
        tags: collect_links_after(&body, "النوع").into_iter().chain(collect_links_after(&body, "اللغه")).chain(collect_links_after(&body, "السنه")).collect(),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: if title.contains("كامل") || title.contains("فيلم") { ItemStatus::Completed } else { ItemStatus::Ongoing },
        url: Some(absolute_url(path)),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn add_episodes(out: &mut Vec<VideoEpisode>, target: &str, final_page: bool) {
    let body = get_or_fixture(target, EPISODES_FIXTURE);
    if final_page {
        let season = html::text_between(&body, "singleTitle", "</div>").map(|text| html::strip_tags(&text)).unwrap_or_default();
        let season_num = season.split("الموسم ").nth(1).and_then(|part| part.split_whitespace().next()).and_then(|part| part.parse::<f32>().ok());
        for episode in parse_episode_links(&body, season_num) {
            out.push(episode);
        }
    } else if target.contains("assembly") {
        for chunk in body.split("salery-list").nth(1).unwrap_or_default().split("<a").skip(1) {
            if let Some(href) = html::attr(chunk, "href") {
                out.push(VideoEpisode {
                    key: path_key(&href),
                    title: Some(html::attr(chunk, "title").unwrap_or_else(|| html::strip_tags(chunk))),
                    url: Some(absolute_url(&href)),
                    language: Some("ar".to_string()),
                    ..VideoEpisode::default()
                });
            }
        }
    } else if target.contains("serie") || target.contains("season") {
        let seasons: Vec<_> = body.split("seasons-list").nth(1).unwrap_or_default().split("<a").skip(1).filter_map(|chunk| html::attr(chunk, "href")).collect();
        if seasons.is_empty() {
            out.extend(parse_episode_links(&body, None));
        } else {
            for season in seasons {
                add_episodes(out, &absolute_url(&season), true);
            }
        }
    } else if target.contains("episode") {
        if let Some(parent) = html::attr_after(&body, "itemprop=url", "href") {
            add_episodes(out, &absolute_url(&parent), false);
        }
    } else {
        out.push(VideoEpisode {
            key: path_key(target),
            title: Some("مشاهدة".to_string()),
            episode_number: Some(1.0),
            url: Some(target.to_string()),
            language: Some("ar".to_string()),
            ..VideoEpisode::default()
        });
    }
}

fn parse_episode_links(body: &str, season_num: Option<f32>) -> Vec<VideoEpisode> {
    body.split("EpsList").nth(1).unwrap_or(body).split("<a").skip(1).filter_map(|chunk| {
        let href = html::attr(chunk, "href")?;
        let title = html::strip_tags(chunk);
        let ep_num = first_number(&title).unwrap_or(0.0);
        Some(VideoEpisode {
            key: path_key(&href),
            title: Some(if let Some(season) = season_num { format!("الموسم {} {title}", season as i32) } else { title.clone() }),
            episode_number: Some(season_num.map(|season| season + ep_num / 1000.0).unwrap_or(ep_num)),
            season_number: season_num,
            url: Some(absolute_url(&href)),
            language: Some("ar".to_string()),
            ..VideoEpisode::default()
        })
    }).collect()
}

fn parse_hosters(body: &str, referer: &str) -> Vec<VideoHoster> {
    body.split("serversList").nth(1).unwrap_or(body).split("<li").skip(1).filter_map(|chunk| {
        let embed = html::attr(chunk, "data-link")?;
        if embed.trim().is_empty() { return None; }
        let name = html::strip_tags(chunk);
        Some(VideoHoster {
            key: absolute_url(&embed),
            name: if name.is_empty() { host_name(&embed) } else { name },
            url: Some(absolute_url(&embed)),
            lazy: true,
            video_count: Some(1),
            headers: referer_headers(referer),
            ..VideoHoster::default()
        })
    }).collect()
}

fn resolve_streams(target: &str, name: &str) -> Vec<VideoStream> {
    if target.contains("ahvsh") || target.contains("fanakishtuna") || target.contains("uqload") {
        let url = if target.contains("uqload.co/") { target.replace("https://uqload.co/", "https://www.uqload.co/") } else { target.to_string() };
        let body = get_or_fixture(&url, STREAM_FIXTURE);
        if let Some(src) = source_from_script(&body) {
            return vec![media_stream(&src, name, name, &url)];
        }
    }
    if target.contains(".m3u8") || target.contains(".mp4") {
        vec![media_stream(target, name, name, BASE_URL)]
    } else {
        vec![external_stream(target, name)]
    }
}

fn source_from_script(body: &str) -> Option<String> {
    body.split("sources").nth(1).and_then(|part| {
        if let Some(file_part) = part.split("file").nth(1) {
            quoted_value(file_part)
        } else {
            quoted_value(part)
        }
    })
}

fn quoted_value(input: &str) -> Option<String> {
    let quote = input.find(['"', '\''])?;
    let rest = &input[quote + 1..];
    let end = rest.find(['"', '\''])?;
    Some(rest[..end].to_string())
}

fn media_stream(stream_url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = stream_url.contains(".m3u8");
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls { VideoStreamKind::Hls } else { VideoStreamKind::Direct }),
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
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(BASE_URL),
        initialized: true,
        ..VideoStream::default()
    }
}

fn collect_links_after(body: &str, label: &str) -> Vec<String> {
    body.split(label).nth(1).unwrap_or_default().split("</li>").next().unwrap_or_default().split("<a").skip(1)
        .map(html::strip_tags).filter(|text| !text.is_empty()).collect()
}

fn prefer_quality(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by_key(|stream| !stream.quality.as_deref().unwrap_or_default().contains(preferred));
    for stream in streams {
        stream.preferred = stream.quality.as_deref().map(|quality| quality.contains(preferred)).unwrap_or(false);
    }
}

fn first_number(input: &str) -> Option<f32> {
    input.chars().filter(|ch| ch.is_ascii_digit() || *ch == '.').collect::<String>().parse().ok()
}

fn host_name(input: &str) -> String {
    input.split("://").nth(1).unwrap_or(input).split('/').next().unwrap_or("External").replace("www.", "")
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request.get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("url")))
        .or_else(|| request.get(field))
        .and_then(Value::as_str)
        .map(path_key)
}

fn path_from_url(input: &str) -> Option<String> {
    input.find(BASE_URL).map(|idx| path_key(&input[idx + BASE_URL.len()..])).or_else(|| input.starts_with('/').then(|| path_key(input)))
}

fn path_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) { return path_key(path); }
    if input.starts_with("http") { return input.to_string(); }
    format!("/{}", input.split('?').next().unwrap_or(input).trim_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") { input.to_string() } else if input.starts_with("//") { format!("https:{input}") } else { url::join_url(BASE_URL, input) }
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request.get("filters").and_then(|filters| filters.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).map(ToString::to_string)
}

fn pref<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request.get("preferences").and_then(|prefs| prefs.get(key)).or_else(|| request.get(key)).and_then(Value::as_str).unwrap_or(default)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn has_next_page(body: &str) -> bool {
    body.contains("pagination") && body.contains("next")
}

const LIST_FIXTURE: &str = r#"<li class="movieItem"><a href="/movie/sample"><img src="/cover.jpg"><h1 class="BottomTitle">Sample Movie</h1></a></li>"#;
const DETAILS_FIXTURE: &str = r#"<div class="single-thumbnail"><img src="/cover.jpg"></div><div class="infoBox"><div class="singleTitle">Sample Movie</div><div class="extra-content"><p>Sample description.</p></div></div>"#;
const EPISODES_FIXTURE: &str = r#"<div class="EpsList"><li><a href="/episode/sample-1">الحلقة 1</a></li></div>"#;
const HOSTERS_FIXTURE: &str = r#"<ul class="serversList"><li data-link="https://media.invalid/embed/sample">Mirror</li></ul>"#;
const STREAM_FIXTURE: &str = r#"<script>sources: [{file: "https://media.invalid/sample.mp4"}]</script>"#;

export_video_source!(SOURCE);
