#[path = "../../_shared/zh_video.rs"]
mod zh;

use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::sdk::SearchRequest;
use scraper::ElementRef;
use serde_json::Value;

const SOURCE: Hanime1 = Hanime1;
const BASE_URL: &str = "https://hanime1.me";
const LANG: &str = "zh";
const RATING: &str = "adult";

struct Hanime1;

impl VideoSource for Hanime1 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let mut next = request.clone();
        if let Some(map) = next.as_object_mut() {
            if zh::listing(&request) == "popular" {
                map.insert("sort".to_string(), Value::String("本週排行".to_string()));
            }
        }
        self.search(next)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = zh::path_from_url(BASE_URL, query) {
            return Ok(Paged { entries: vec![fetch_details(&path)], has_next_page: false });
        }
        let target = search_url(&request, query);
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &target, BASE_URL, LIST_FIXTURE);
        Ok(parse_search(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = zh::request_key(&request, "item").unwrap_or_else(|| "/watch/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = zh::request_key(&request, "item").unwrap_or_else(|| "/watch/sample".to_string());
        let url = zh::absolute_url(BASE_URL, &path);
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &url, BASE_URL, DETAILS_FIXTURE);
        Ok(parse_episodes(&body, &url))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = zh::request_key(&request, "episode")
            .or_else(|| zh::request_key(&request, "item"))
            .unwrap_or_else(|| "/watch/sample".to_string());
        let url = zh::absolute_url(BASE_URL, &path);
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &url, BASE_URL, DETAILS_FIXTURE);
        let mut streams = parse_streams(&body, &url);
        zh::sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(zh::with_listing(&request, "popular"))?;
        let latest = self.list(zh::with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection { id: "popular".to_string(), title: "本週排行".to_string(), style: Some(HomeSectionStyle::Featured), entries: popular.entries, has_more: popular.has_next_page, ..HomeSection::default() },
            HomeSection { id: "latest".to_string(), title: "最新上市".to_string(), entries: latest.entries, has_more: latest.has_next_page, ..HomeSection::default() },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(zh::request_key(&request, "item").map(|path| zh::absolute_url(BASE_URL, &path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(zh::request_key(&request, "episode").map(|path| zh::absolute_url(BASE_URL, &path)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(path) = zh::path_from_url(BASE_URL, input) {
            return Ok(Some(UrlResolveResult { item: Some(fetch_details(&path)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn search_url(request: &Value, query: &str) -> String {
    let mut params = Vec::new();
    if !query.is_empty() {
        params.push(format!("query={}", manatan_shared::url::query_escape(query)));
    }
    for key in ["genre", "sort", "date"] {
        if let Some(value) = zh::filter(request, key).or_else(|| request.get(key).and_then(Value::as_str).map(ToString::to_string)) {
            if !value.is_empty() {
                params.push(format!("{key}={}", manatan_shared::url::query_escape(&value)));
            }
        }
    }
    if zh::pref_bool(request, "broad", false) || request.get("broad").and_then(Value::as_bool).unwrap_or(false) {
        params.push("broad=on".to_string());
    }
    if let Some(tags) = zh::filter(request, "tags") {
        for tag in tags.split(',').map(str::trim).filter(|tag| !tag.is_empty()) {
            params.push(format!("tags[]={}", manatan_shared::url::query_escape(tag)));
        }
    }
    let page = zh::page(request);
    if page > 1 {
        params.push(format!("page={page}"));
    }
    if params.is_empty() {
        format!("{BASE_URL}/search")
    } else {
        format!("{BASE_URL}/search?{}", params.join("&"))
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let doc = zh::document(body);
    let mut entries = Vec::new();
    for selector in [".horizontal-row .video-item-container", "a:not([target]) > .search-videos"] {
        let sel = zh::selector(selector);
        for node in doc.select(&sel) {
            if let Some(item) = parse_card(node) {
                entries.push(item);
            }
        }
        if !entries.is_empty() {
            break;
        }
    }
    Paged {
        entries: zh::dedupe_items(entries),
        has_next_page: !doc.select(&zh::selector("li.page-item a.page-link[rel=next]")).next().is_none(),
    }
}

fn parse_card(node: ElementRef<'_>) -> Option<CatalogItem> {
    let link_sel = zh::selector("a.video-link, a");
    let link = node.select(&link_sel).next().unwrap_or(node);
    let href = zh::attr(&link, "href")?;
    let key = zh::path_key(BASE_URL, &href);
    let title = node.select(&zh::selector(".title, .home-rows-videos-title"))
        .next()
        .map(|v| zh::text(&v))
        .filter(|v| !v.is_empty())
        .or_else(|| zh::attr(&link, "title"))
        .unwrap_or_else(|| key.trim_matches('/').to_string());
    let cover = node.select(&zh::selector(".main-thumb, img")).next().and_then(|img| {
        zh::attr(&img, "abs:src")
            .or_else(|| zh::attr(&img, "src"))
            .or_else(|| zh::attr(&img, "data-src"))
    });
    let mut item = zh::catalog_item(BASE_URL, key, title, cover, LANG, RATING);
    item.authors = node.select(&zh::selector(".subtitle")).next().map(|n| zh::text(&n).split('•').next().unwrap_or_default().trim().to_string()).filter(|v| !v.is_empty()).into_iter().collect();
    Some(item)
}

fn fetch_details(path: &str) -> CatalogItem {
    let url = zh::absolute_url(BASE_URL, path);
    let body = zh::fetch_or_smoke_fixture(BASE_URL, &url, BASE_URL, DETAILS_FIXTURE);
    let doc = zh::document(&body);
    let title = zh::first_text(&doc, &["div.video-description-panel > div:nth-child(2)", "h1", ".title"])
        .unwrap_or_else(|| path.trim_matches('/').replace('-', " "));
    CatalogItem {
        key: zh::path_key(BASE_URL, path),
        title,
        cover: zh::first_attr(&doc, &["video[poster]", ".main-thumb", "img"], "poster")
            .or_else(|| zh::first_attr(&doc, &[".main-thumb", "img"], "src"))
            .map(|value| zh::absolute_url(BASE_URL, &value)),
        url: Some(url),
        description: zh::first_text(&doc, &["div.video-description-panel > div:nth-child(3)"]),
        authors: zh::first_text(&doc, &["#video-artist-name"]).into_iter().collect(),
        tags: doc.select(&zh::selector(".single-video-tag:not([data-toggle])")).map(|n| zh::text(&n)).filter(|v| !v.is_empty()).collect(),
        language: Some(LANG.to_string()),
        content_rating: Some(RATING.to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, referer: &str) -> Vec<VideoEpisode> {
    let doc = zh::document(body);
    let nodes: Vec<_> = doc.select(&zh::selector("#playlist-scroll > div")).collect();
    let total = nodes.len();
    let mut episodes = Vec::new();
    for (index, node) in nodes.into_iter().enumerate() {
        let Some(link) = node.select(&zh::selector("a.overlay, a")).next() else { continue; };
        let Some(href) = zh::attr(&link, "href") else { continue; };
        let key = zh::path_key(BASE_URL, &href);
        episodes.push(VideoEpisode {
            key: key.clone(),
            title: node.select(&zh::selector("div.card-mobile-title, .title")).next().map(|n| zh::text(&n)).filter(|v| !v.is_empty()),
            episode_number: Some((total.saturating_sub(index)) as f32),
            url: Some(zh::absolute_url(BASE_URL, &key)),
            language: Some(LANG.to_string()),
            ..VideoEpisode::default()
        });
    }
    if episodes.is_empty() {
        episodes.push(VideoEpisode { key: zh::path_key(BASE_URL, referer), title: Some("Video".to_string()), episode_number: Some(1.0), url: Some(referer.to_string()), language: Some(LANG.to_string()), ..VideoEpisode::default() });
    }
    zh::dedupe_episodes(episodes)
}

fn parse_streams(body: &str, referer: &str) -> Vec<VideoStream> {
    let doc = zh::document(body);
    let mut streams = Vec::new();
    for source in doc.select(&zh::selector("video source")) {
        if let Some(src) = zh::attr(&source, "src") {
            let quality = zh::attr(&source, "size").map(|value| format!("{value}p")).unwrap_or_else(|| "Raw".to_string());
            streams.push(zh::direct_stream(&zh::absolute_url(BASE_URL, &src), &quality, referer));
        }
    }
    if streams.is_empty() {
        for script in doc.select(&zh::selector("script")) {
            let data = script.inner_html();
            if let Some(raw) = data.split("source = '").nth(1).and_then(|part| part.split('\'').next()) {
                streams.push(zh::direct_stream(raw, "Raw", referer));
            }
        }
    }
    streams
}

const LIST_FIXTURE: &str = r#"
<div class="horizontal-row"><div class="video-item-container"><a class="video-link" href="/watch/sample"><img class="main-thumb" src="/sample.jpg"><div class="title">Sample</div></a></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<video poster="/sample.jpg"><source src="https://media.example/sample.m3u8" size="720"></video>
<div id="playlist-scroll"><div><a class="overlay" href="/watch/sample"></a><div class="card-mobile-title">Sample</div></div></div>
<div class="video-description-panel"><div></div><div>Sample</div><div>Fixture only for smoke tests.</div></div>
"#;

export_video_source!(SOURCE);
