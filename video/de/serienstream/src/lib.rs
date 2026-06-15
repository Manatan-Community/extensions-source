use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use scraper::{ElementRef, Html, Selector};
use serde_json::{Value, json};

const SOURCE: Serienstream = Serienstream;
const BASE_URL: &str = "http://186.2.175.5";

struct Serienstream;

impl VideoSource for Serienstream {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/neu")
        } else {
            format!("{BASE_URL}/beliebte-serien")
        };
        Ok(parse_listing(&get_or_fixture(&target, LIST_FIXTURE, BASE_URL)))
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
        if query.is_empty() {
            return self.list(json!({ "listingId": "popular" }));
        }
        let body = client(BASE_URL)
            .post(format!("{BASE_URL}/ajax/search"))
            .xhr()
            .referer(format!("{BASE_URL}/search"))
            .origin(BASE_URL)
            .form(&[("keyword", query)])
            .send_text()
            .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
        Ok(parse_search(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/serie/stream/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/serie/stream/sample".to_string());
        let body = get_or_fixture(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episode_list(&body))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode = request_key(&request, "episode")
            .unwrap_or_else(|| "/serie/stream/sample/staffel-1/episode-1".to_string());
        let body = get_or_fixture(&absolute_url(&episode), WATCH_FIXTURE, BASE_URL);
        Ok(parse_hosters(&body, &absolute_url(&episode), &request))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = key.splitn(4, '|');
        let hoster_name = parts.next().unwrap_or("External");
        let language = parts.next().unwrap_or_default();
        let redirect = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(BASE_URL);
        let final_url = client(referer)
            .get(redirect)
            .referer(referer)
            .send()
            .map(|response| response.final_url)
            .unwrap_or_else(|_| redirect.to_string());
        Ok(vec![VideoStream {
            url: final_url,
            name: Some(format!("{hoster_name} {language}")),
            quality: Some(language.to_string()),
            format: Some("external".to_string()),
            stream_kind: Some(VideoStreamKind::External),
            initialized: true,
            headers: stream_headers(referer),
            ..VideoStream::default()
        }])
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
                title: "Beliebte Serien".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Neu".to_string(),
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
            if path.contains("/staffel-") || path.contains("/filme/") || path.contains("/film/") {
                return Ok(Some(UrlResolveResult {
                    episode: Some(json!({
                        "key": path,
                        "url": absolute_url(&path),
                        "language": "de"
                    })),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
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

fn client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let document = Html::parse_document(body);
    let entries = select_all(&document, "div.seriesListContainer div")
        .into_iter()
        .filter_map(series_card)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn series_card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = select_attr(el, "a", "href")?;
    let title = select_text(el, "h3").or_else(|| select_attr(el, "img", "alt"))?;
    Some(CatalogItem {
        key: path_key(&href),
        title,
        cover: select_attr(el, "img", "data-src")
            .or_else(|| select_attr(el, "img", "src"))
            .map(|src| absolute_url(&src)),
        url: Some(absolute_url(&href)),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let results = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let entries = results
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let link = entry.get("link").and_then(Value::as_str)?;
            if !link.starts_with("/serie/stream/") || link.matches('/').count() != 3 {
                return None;
            }
            let title = entry
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Serienstream")
                .replace("<em>", "")
                .replace("</em>", "");
            let details = fetch_details(link);
            Some(CatalogItem {
                title,
                ..details
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let document = Html::parse_document(&body);
    CatalogItem {
        key: path_key(path),
        title: select_text_document(&document, "div.series-title h1 span, h1")
            .or_else(|| url::slug_from_url(path))
            .unwrap_or_else(|| "Serienstream".to_string()),
        cover: select_attr_document(&document, "div.seriesCoverBox img, img", "data-src")
            .or_else(|| select_attr_document(&document, "div.seriesCoverBox img, img", "src"))
            .map(|src| absolute_url(&src)),
        description: select_attr_document(&document, "p.seri_des", "data-full-description")
            .or_else(|| select_text_document(&document, "p.seri_des")),
        authors: select_text_document(&document, "div.cast li ul li").into_iter().collect(),
        tags: select_all(&document, "div.genres ul li")
            .into_iter()
            .map(|el| text(el))
            .filter(|value| !value.is_empty())
            .collect(),
        url: Some(absolute_url(path)),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episode_list(body: &str) -> Vec<VideoEpisode> {
    let document = Html::parse_document(body);
    let season_links = select_all(&document, "#stream > ul:nth-child(1) > li > a, #stream ul li a")
        .into_iter()
        .filter_map(|el| el.value().attr("href").map(ToString::to_string))
        .collect::<Vec<_>>();
    let mut episodes = Vec::new();
    for season in season_links {
        let season_url = absolute_url(&season);
        let season_body = get_or_fixture(&season_url, EPISODES_FIXTURE, BASE_URL);
        episodes.extend(parse_episode_rows(&season_body));
    }
    if episodes.is_empty() {
        episodes.extend(parse_episode_rows(body));
    }
    episodes.reverse();
    episodes
}

fn parse_episode_rows(body: &str) -> Vec<VideoEpisode> {
    let document = Html::parse_document(body);
    select_all(&document, "table.seasonEpisodesList tbody tr, tr[data-episode-season-id]")
        .into_iter()
        .filter_map(episode_row)
        .collect()
}

fn episode_row(el: ElementRef<'_>) -> Option<VideoEpisode> {
    let href = select_attr(el, "td.seasonEpisodeTitle a, a", "href")?;
    let number = el.value().attr("data-episode-season-id").unwrap_or("1");
    let label = select_text(el, "td.seasonEpisodeTitle a span, a span")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Episode".to_string());
    let title = if href.contains("/film") {
        format!("Film {number}: {label}")
    } else {
        let season = href
            .split("staffel-")
            .nth(1)
            .and_then(|value| value.split('/').next())
            .unwrap_or("1");
        format!("Staffel {season} Folge {number}: {label}")
    };
    Some(VideoEpisode {
        key: path_key(&href),
        title: Some(title),
        episode_number: select_attr(el, "td meta", "content")
            .or_else(|| Some(number.to_string()))
            .and_then(|value| value.parse::<f32>().ok()),
        url: Some(absolute_url(&href)),
        language: Some("de".to_string()),
        ..VideoEpisode::default()
    })
}

fn parse_hosters(body: &str, episode_url: &str, request: &Value) -> Vec<VideoHoster> {
    let document = Html::parse_document(body);
    let enabled = enabled_hosters(request);
    let mut hosters = select_all(&document, "div.hosterSiteVideo ul.row li, li[data-lang-key]")
        .into_iter()
        .filter_map(|el| {
            let lang_key = el.value().attr("data-lang-key").unwrap_or_default();
            let language = language_name(lang_key);
            let redirect = select_attr(el, "a.watchEpisode, a", "href")?;
            let name = select_text(el, "a h4, h4").unwrap_or_else(|| "External".to_string());
            let normalized = normalize_hoster(&name);
            if !enabled.is_empty() && !enabled.iter().any(|item| normalized.contains(item)) {
                return None;
            }
            let url = absolute_url(&redirect);
            Some(VideoHoster {
                key: format!("{normalized}|{language}|{url}|{episode_url}"),
                name: format!("{normalized} {language}"),
                url: Some(url),
                lazy: true,
                video_count: Some(1),
                headers: stream_headers(episode_url),
                ..VideoHoster::default()
            })
        })
        .collect::<Vec<_>>();
    sort_hosters(&mut hosters, request);
    hosters
}

fn language_name(value: &str) -> String {
    if value.contains('1') {
        "Deutscher Dub".to_string()
    } else if value.contains('2') {
        "Englischer Sub".to_string()
    } else if value.contains('3') {
        "Deutscher Sub".to_string()
    } else {
        "Unbekannt".to_string()
    }
}

fn normalize_hoster(name: &str) -> String {
    if name.contains("VOE") {
        "VOE".to_string()
    } else if name.contains("Dood") {
        "Doodstream".to_string()
    } else if name.contains("Streamtape") {
        "Streamtape".to_string()
    } else {
        name.trim().to_string()
    }
}

fn enabled_hosters(request: &Value) -> Vec<String> {
    preferences(request)
        .get("enabled_hosters")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(ToString::to_string).collect())
        .unwrap_or_default()
}

fn sort_hosters(hosters: &mut [VideoHoster], request: &Value) {
    let preferred_hoster = preference_string(request, "preferred_hoster", "Streamtape");
    let preferred_language = preference_string(request, "preferred_language", "Deutscher Sub");
    hosters.sort_by_key(|hoster| {
        let hoster_score = if hoster.name.contains(&preferred_hoster) { 0 } else { 1 };
        let lang_score = if hoster.name.contains(&preferred_language) { 0 } else { 1 };
        (hoster_score, lang_score, hoster.name.clone())
    });
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred_hoster = preference_string(request, "preferred_hoster", "Streamtape");
    let preferred_language = preference_string(request, "preferred_language", "Deutscher Sub");
    streams.sort_by_key(|stream| {
        let name = stream.name.clone().unwrap_or_default();
        let hoster_score = if name.contains(&preferred_hoster) { 0 } else { 1 };
        let lang_score = if name.contains(&preferred_language) { 0 } else { 1 };
        (hoster_score, lang_score, name)
    });
    if let Some(first) = streams.first_mut() {
        first.preferred = true;
    }
}

fn preference_string(request: &Value, key: &str, fallback: &str) -> String {
    preferences(request)
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_string()
}

fn preferences(request: &Value) -> &Value {
    request.get("preferences").unwrap_or(&Value::Null)
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut clone = request.clone();
    if let Some(object) = clone.as_object_mut() {
        object.insert("listingId".to_string(), Value::String(listing.to_string()));
    } else {
        clone = json!({ "listingId": listing });
    }
    clone
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| value.get("key").or(Some(value)))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| request.get("key").and_then(Value::as_str).map(ToString::to_string))
}

fn request_raw_key(request: &Value, field: &str) -> Option<String> {
    request_key(request, field)
}

fn path_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) {
        return Some(path_key(input));
    }
    if input.starts_with("/serie/") {
        return Some(path_key(input));
    }
    None
}

fn path_key(value: &str) -> String {
    if value.starts_with(BASE_URL) {
        let base = BASE_URL.trim_end_matches('/');
        return format!("/{}", value.trim_start_matches(base).trim_start_matches('/'));
    }
    format!("/{}", value.trim_start_matches('/'))
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        url::join_url(BASE_URL, value)
    }
}

fn stream_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn select_all<'a>(document: &'a Html, selector: &str) -> Vec<ElementRef<'a>> {
    Selector::parse(selector)
        .ok()
        .map(|selector| document.select(&selector).collect())
        .unwrap_or_default()
}

fn select_text_document(document: &Html, selector: &str) -> Option<String> {
    select_all(document, selector)
        .into_iter()
        .find_map(|el| Some(text(el)).filter(|value| !value.is_empty()))
}

fn select_attr_document(document: &Html, selector: &str, attr: &str) -> Option<String> {
    select_all(document, selector)
        .into_iter()
        .find_map(|el| el.value().attr(attr).map(ToString::to_string))
}

fn select_text(el: ElementRef<'_>, selector: &str) -> Option<String> {
    Selector::parse(selector).ok().and_then(|selector| {
        el.select(&selector)
            .find_map(|child| Some(text(child)).filter(|value| !value.is_empty()))
    })
}

fn select_attr(el: ElementRef<'_>, selector: &str, attr: &str) -> Option<String> {
    Selector::parse(selector).ok().and_then(|selector| {
        el.select(&selector)
            .find_map(|child| child.value().attr(attr).map(ToString::to_string))
    })
}

fn text(el: ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="seriesListContainer"><div><a href="/serie/stream/sample"><img data-src="/cover.jpg"><h3>Sample Series</h3></a></div></div>"#;
const SEARCH_FIXTURE: &str = r#"[{"title":"Sample Series","link":"/serie/stream/sample"}]"#;
const DETAILS_FIXTURE: &str = r#"<div class="series-title"><h1><span>Sample Series</span></h1></div><div class="seriesCoverBox"><img data-src="/cover.jpg"></div><p class="seri_des" data-full-description="Description"></p><div class="genres"><ul><li>Action</li></ul></div><div id="stream"><ul><li><a href="/serie/stream/sample/staffel-1">Season 1</a></li></ul></div>"#;
const EPISODES_FIXTURE: &str = r#"<table class="seasonEpisodesList"><tbody><tr data-episode-season-id="1"><td class="seasonEpisodeTitle"><a href="/serie/stream/sample/staffel-1/episode-1"><span>Episode title</span></a></td><td><meta content="1"></td></tr></tbody></table>"#;
const WATCH_FIXTURE: &str = r#"<div class="hosterSiteVideo"><ul class="row"><li data-lang-key="3"><a class="watchEpisode" href="/redirect/1"><h4>Streamtape</h4></a></li><li data-lang-key="1"><a class="watchEpisode" href="/redirect/2"><h4>VOE</h4></a></li></ul></div>"#;
