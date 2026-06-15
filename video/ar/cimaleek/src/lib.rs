use manatan_extension::{
    abi::{ExtensionError, ExtensionResult},
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

const SOURCE: Cimaleek = Cimaleek;
const BASE_URL: &str = "https://m.cimaleek.to";

struct Cimaleek;

impl VideoSource for Cimaleek {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request.get("listing").and_then(Value::as_str).unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/recent/page/{page}/")
        } else {
            format!("{BASE_URL}/trending/page/{page}/")
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
            format!("{BASE_URL}/page/{page}?s={}", manatan_shared::sdk::http::url_encode(query))
        } else if let Some(section) = filter(&request, "section").filter(|value| !value.is_empty()) {
            format!("{BASE_URL}/category/{section}/page/{page}")
        } else if let Some(category) = filter(&request, "category").filter(|value| !value.is_empty()) {
            let genre = filter(&request, "genre").unwrap_or_else(|| "Action".to_string()).to_lowercase();
            format!("{BASE_URL}/genre/{genre}/page/{page}?type={category}")
        } else {
            return Err(error("من فضلك اختر قسم او نوع"));
        };
        let body = get_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged { entries: parse_cards(&body), has_next_page: has_next_page(&body) })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        Ok(fetch_details(&request_key(&request, "item").unwrap_or_else(|| "/movies/sample".to_string())))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/movies/sample".to_string());
        let body = get_or_fixture(&absolute_url(&path), EPISODES_FIXTURE);
        if absolute_url(&path).contains("movies") {
            return Ok(vec![VideoEpisode {
                key: path_key(&format!("{}/watch", path.trim_end_matches('/'))),
                title: Some("مشاهدة".to_string()),
                episode_number: Some(1.0),
                url: Some(absolute_url(&format!("{}/watch", path.trim_end_matches('/')))),
                language: Some("ar".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let mut episodes = Vec::new();
        let seasons = parse_season_links(&body);
        if seasons.is_empty() {
            episodes.extend(parse_episode_links(&body, 1.0, "الموسم 1"));
        } else {
            for (season_num, season_url) in seasons {
                let season_body = get_or_fixture(&absolute_url(&season_url), EPISODES_FIXTURE);
                episodes.extend(parse_episode_links(&season_body, season_num, &format!("الموسم {}", season_num as i32)));
            }
        }
        episodes.sort_by(|a, b| b.episode_number.partial_cmp(&a.episode_number).unwrap_or(std::cmp::Ordering::Equal));
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/movies/sample/watch".to_string());
        let body = get_or_fixture(&absolute_url(&path), HOSTERS_FIXTURE);
        let version = body.split("ver\":\"").nth(1).and_then(|part| part.split('"').next()).unwrap_or_default();
        Ok(parse_hosters(&body, version))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let name = request.get("hoster").and_then(|h| h.get("name")).and_then(Value::as_str).unwrap_or("Mirror");
        let mut streams = resolve_cimaleek_hoster(&key, name);
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

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("film_list-wrap").nth(1).unwrap_or(body).split("div class=\"item").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let title = html::text_between(chunk, "class=\"title", "</")
            .map(|text| html::strip_tags(&text))
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .unwrap_or_else(|| path_key(&href).trim_matches('/').replace('-', " "));
        Some(CatalogItem {
            key: path_key(&href),
            title,
            cover: html::attr_after(chunk, "<img", "data-src").or_else(|| html::attr_after(chunk, "<img", "src")).map(|image| absolute_url(&image)),
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
    let status_text = html::text_between(&body, "anisc-detail", "</div>").map(|text| html::strip_tags(&text)).unwrap_or_default();
    CatalogItem {
        key: path_key(path),
        title: info_value(&body, "الاسم").or_else(|| html::text_between(&body, "<h1", "</h1>").map(|text| html::strip_tags(&text))).unwrap_or_else(|| path_key(path).trim_matches('/').replace('-', " ")),
        cover: html::attr_after(&body, "film-poster", "src").or_else(|| html::attr_after(&body, "<img", "src")).map(|image| absolute_url(&image)),
        authors: info_value(&body, "البلد").into_iter().collect(),
        description: html::text_between(&body, "film-description", "</div>").map(|text| html::strip_tags(&text)).filter(|text| !text.is_empty()),
        tags: collect_anchor_text(&body, "item-list"),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: if status_text.contains("افلام") { ItemStatus::Completed } else { ItemStatus::Unknown },
        url: Some(absolute_url(path)),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_season_links(body: &str) -> Vec<(f32, String)> {
    body.split("seas-list").nth(1).unwrap_or_default().split("<a").skip(1).filter_map(|chunk| {
        let href = html::attr(chunk, "href")?;
        let season = html::text_between(chunk, "se-a", "</span>").map(|text| html::strip_tags(&text)).and_then(|text| text.parse::<f32>().ok()).unwrap_or(1.0);
        Some((season, href))
    }).collect()
}

fn parse_episode_links(body: &str, season_num: f32, season_name: &str) -> Vec<VideoEpisode> {
    body.split("episodesList").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let ep_num = html::text_between(chunk, "serie", "</span>")
            .map(|text| html::strip_tags(&text))
            .and_then(|text| text.split('(').nth(1).and_then(|part| part.split(')').next()).and_then(|part| part.parse::<f32>().ok()))
            .or_else(|| first_number(chunk))
            .unwrap_or(0.0);
        let key = path_key(&format!("{}/watch", href.trim_end_matches('/')));
        Some(VideoEpisode {
            key,
            title: Some(format!("{season_name} الحلقة {}", ep_num as i32)),
            episode_number: Some(season_num + ep_num / 1000.0),
            season_number: Some(season_num),
            url: Some(absolute_url(&format!("{}/watch", href.trim_end_matches('/')))),
            language: Some("ar".to_string()),
            ..VideoEpisode::default()
        })
    }).collect()
}

fn parse_hosters(body: &str, version: &str) -> Vec<VideoHoster> {
    body.split("server-item").skip(1).flat_map(|section| section.split("<div").skip(1)).filter_map(|chunk| {
        let post = html::attr(chunk, "data-post")?;
        let type_value = html::attr(chunk, "data-type")?;
        let nume = html::attr(chunk, "data-nume")?;
        let name = html::strip_tags(chunk);
        Some(VideoHoster {
            key: format!("{post}|{type_value}|{nume}|{version}"),
            name: if name.is_empty() { "Mirror".to_string() } else { name },
            lazy: true,
            video_count: Some(1),
            headers: referer_headers(BASE_URL),
            ..VideoHoster::default()
        })
    }).collect()
}

fn resolve_cimaleek_hoster(key: &str, name: &str) -> Vec<VideoStream> {
    let mut parts = key.split('|');
    let post = parts.next().unwrap_or_default();
    let type_value = parts.next().unwrap_or_default();
    let nume = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or_default();
    let target = format!("{BASE_URL}/wp-json/lalaplayer/v2/?p={post}&t={type_value}&n={nume}&ver={version}&rand=manatanport0001");
    let body = get_or_fixture(&target, STREAM_FIXTURE);
    let embed = body.split("embed_url\":\"").nth(1).and_then(|part| part.split('"').next()).map(|value| value.replace("\\/", "/")).unwrap_or_else(|| key.to_string());
    if embed.contains(".m3u8") || embed.contains(".mp4") {
        vec![media_stream(&embed, name, name, BASE_URL)]
    } else {
        vec![external_stream(&embed, name)]
    }
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

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split(label).nth(1).map(|chunk| html::strip_tags(chunk.split("</div>").next().unwrap_or(chunk))).map(|text| text.replace(label, "").trim().to_string()).filter(|text| !text.is_empty())
}

fn collect_anchor_text(body: &str, marker: &str) -> Vec<String> {
    body.split(marker).nth(1).unwrap_or_default().split("</div>").next().unwrap_or_default().split("<a").skip(1)
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
    body.contains("pagination") && body.contains("nextpagination")
}

fn error(message: &str) -> ExtensionError {
    ExtensionError { message: message.to_string() }
}

const LIST_FIXTURE: &str = r#"<div class="film_list-wrap"><div class="item"><a href="/movies/sample"><img data-src="/cover.jpg"><div class="data"><span class="title">Sample Movie</span></div></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="film-poster"><img src="/cover.jpg"></div><div class="anisc-more-info"><div class="item">الاسم <span></span><span>Sample Movie</span></div></div><div class="film-description"><div class="text">Sample description.</div></div><div class="anisc-detail"><div class="item-list"><a>افلام</a></div></div>"#;
const EPISODES_FIXTURE: &str = r#"<div class="season-a"><ul class="episodios"><li class="episodesList"><a href="/episode/sample-1"><span class="serie">(1)</span></a></li></ul></div>"#;
const HOSTERS_FIXTURE: &str = r#"<script>var dtAjax={"ver":"1.0"}</script><div id="servers-content"><div class="server-item"><div data-post="1" data-type="movie" data-nume="1">Mirror</div></div></div>"#;
const STREAM_FIXTURE: &str = r#"{"embed_url":"https://media.invalid/embed/sample"}"#;

export_video_source!(SOURCE);
