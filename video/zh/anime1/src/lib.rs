#[path = "../../_shared/zh_video.rs"]
mod zh;

use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Anime1 = Anime1;
const BASE_URL: &str = "https://anime1.me";
const VIDEO_API: &str = "https://v.anime1.me/api";
const DATA_URL: &str = "https://d1zquzjgwo9yb.cloudfront.net";
const LANG: &str = "zh-Hant";
const RATING: &str = "safe";
const FIX_COVER: &str = "https://sta.anicdn.com/playerImg/8.jpg";
const PAGE_SIZE: usize = 20;

struct Anime1;

impl VideoSource for Anime1 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_catalog_json(&fetch_catalog(), zh::page(&request)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = zh::path_from_url(BASE_URL, query) {
            return Ok(Paged { entries: vec![item_from_path(&path, None)], has_next_page: false });
        }
        let mut target = format!("{BASE_URL}/");
        if zh::page(&request) > 1 {
            target.push_str(&format!("page/{}/", zh::page(&request)));
        }
        target.push_str(&format!("?s={}", url::query_escape(query)));
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &target, BASE_URL, SEARCH_FIXTURE);
        Ok(parse_search(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = zh::request_key(&request, "item").unwrap_or_else(|| "/?cat=1".to_string());
        Ok(item_from_path(&path, None))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = zh::request_key(&request, "item").unwrap_or_else(|| "/?cat=1".to_string());
        let url = zh::absolute_url(BASE_URL, &path);
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &url, BASE_URL, SEARCH_FIXTURE);
        Ok(parse_episodes(&body, &url))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = zh::request_key(&request, "episode").unwrap_or_else(|| "/sample".to_string());
        let referer = zh::absolute_url(BASE_URL, &path);
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &referer, BASE_URL, EPISODE_FIXTURE);
        let api_req = zh::first_attr(&zh::document(&body), &["video[data-apireq]"], "data-apireq");
        let Some(api_req) = api_req else { return Ok(Vec::new()); };
        let response = zh::client(BASE_URL, &referer)
            .post(VIDEO_API)
            .referer(&referer)
            .form(&[("d", api_req.as_str())])
            .send_text();
        let body = match response {
            Ok(body) => body,
            Err(error) if zh::is_smoke_http_disabled(&error) => VIDEO_FIXTURE.to_string(),
            Err(error) => return Err(error),
        };
        let parsed: VideoResponse = serde_json::from_str(&body).unwrap_or_default();
        let mut streams = parsed.s.into_iter().map(|source| {
            let stream_url = if source.src.starts_with("//") { format!("https:{}", source.src) } else { source.src };
            zh::direct_stream(&stream_url, &source.kind, &referer)
        }).collect::<Vec<_>>();
        zh::sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let latest = self.list(zh::with_listing(&request, "latest"))?;
        Ok(vec![HomeSection { id: "latest".to_string(), title: "動畫列表".to_string(), style: Some(HomeSectionStyle::Featured), entries: latest.entries, has_more: latest.has_next_page, ..HomeSection::default() }])
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
            return Ok(Some(UrlResolveResult { item: Some(item_from_path(&path, None)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn fetch_catalog() -> Value {
    let target = format!("{DATA_URL}/?_=0");
    let body = zh::fetch_or_smoke_fixture(BASE_URL, &target, BASE_URL, CATALOG_FIXTURE);
    serde_json::from_str(&body).unwrap_or(Value::Array(Vec::new()))
}

fn parse_catalog_json(data: &Value, page: u64) -> Paged<CatalogItem> {
    let empty = Vec::new();
    let array = data.as_array().unwrap_or(&empty);
    let start = ((page.max(1) - 1) as usize) * PAGE_SIZE;
    let end = (start + PAGE_SIZE).min(array.len());
    let entries = array.get(start..end).unwrap_or(&[]).iter().filter_map(|item| {
        let row = item.as_array()?;
        let id = row.first().and_then(Value::as_str).unwrap_or("0");
        let raw_title = row.get(1).and_then(Value::as_str).unwrap_or("Anime1");
        let (title, path) = if raw_title.contains("<a") {
            let doc = zh::document(raw_title);
            let link = doc.select(&zh::selector("a")).next();
            let title = link.as_ref().map(zh::text).unwrap_or_else(|| raw_title.replace(['<', '>'], " "));
            let href = link.and_then(|node| zh::attr(&node, "href")).unwrap_or_else(|| format!("?cat={id}"));
            (title, zh::path_key(BASE_URL, &href))
        } else {
            (raw_title.to_string(), format!("/?cat={id}"))
        };
        let mut catalog = item_from_path(&path, Some(title));
        catalog.tags = row.iter().skip(3).filter_map(Value::as_str).filter(|v| !v.is_empty()).map(ToString::to_string).collect();
        catalog.status = if row.get(2).and_then(Value::as_str).unwrap_or_default().contains("連載中") { ItemStatus::Ongoing } else { ItemStatus::Completed };
        Some(catalog)
    }).collect();
    Paged { entries, has_next_page: end < array.len() }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let doc = zh::document(body);
    let mut entries = Vec::new();
    for link in doc.select(&zh::selector("article.post .entry-title a")) {
        if let Some(href) = zh::attr(&link, "href") {
            entries.push(item_from_path(&zh::path_key(BASE_URL, &href), Some(zh::text(&link))));
        }
    }
    Paged { entries: zh::dedupe_items(entries), has_next_page: doc.select(&zh::selector(".nav-previous a")).next().is_some() }
}

fn item_from_path(path: &str, title: Option<String>) -> CatalogItem {
    CatalogItem {
        key: zh::path_key(BASE_URL, path),
        title: title.unwrap_or_else(|| path.trim_matches('/').replace(['-', '/'], " ")),
        cover: Some(FIX_COVER.to_string()),
        url: Some(zh::absolute_url(BASE_URL, path)),
        language: Some(LANG.to_string()),
        content_rating: Some(RATING.to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, request_url: &str) -> Vec<VideoEpisode> {
    let doc = zh::document(body);
    let mut episodes = Vec::new();
    for article in doc.select(&zh::selector("article.post")) {
        let Some(link) = article.select(&zh::selector(".entry-title a")).next() else { continue; };
        let href = zh::attr(&link, "href").unwrap_or_else(|| request_url.to_string());
        let key = zh::path_key(BASE_URL, &href);
        let title = zh::text(&link);
        episodes.push(VideoEpisode {
            key: key.clone(),
            title: Some(title.clone()),
            episode_number: zh::episode_number(&title),
            url: Some(zh::absolute_url(BASE_URL, &key)),
            language: Some(LANG.to_string()),
            ..VideoEpisode::default()
        });
    }
    zh::dedupe_episodes(episodes)
}

#[derive(Default, Deserialize)]
struct VideoResponse {
    s: Vec<VideoSourceDto>,
}

#[derive(Default, Deserialize)]
struct VideoSourceDto {
    src: String,
    #[serde(rename = "type")]
    kind: String,
}

const CATALOG_FIXTURE: &str = r#"[["1","Sample","已完結","動畫"]]"#;
const SEARCH_FIXTURE: &str = r#"<article class="post"><h2 class="entry-title"><a href="https://anime1.me/sample">Sample</a></h2></article>"#;
const EPISODE_FIXTURE: &str = r#"<video data-apireq="fixture"></video>"#;
const VIDEO_FIXTURE: &str = r#"{"s":[{"src":"//media.example/sample.m3u8","type":"hls"}]}"#;

export_video_source!(SOURCE);
