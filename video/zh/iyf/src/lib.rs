#[path = "../../_shared/zh_video.rs"]
mod zh;

use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{sdk::SearchRequest, url};
use md5::{Digest, Md5};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Iyf = Iyf;
const BASE_URL: &str = "https://www.iyf.tv";
const API_BASE: &str = "https://m10.iyf.tv";
const RANK_BASE: &str = "https://rankv21.iyf.tv";
const LANG: &str = "zh";
const RATING: &str = "safe";
const PAGE_SIZE: u64 = 32;

struct Iyf;

impl VideoSource for Iyf {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let mut next = request.clone();
        if let Some(map) = next.as_object_mut() {
            map.insert("cid".to_string(), Value::String("0,1,6".to_string()));
            map.insert("orderby".to_string(), Value::String(if zh::listing(&request) == "latest" { "1" } else { "2" }.to_string()));
            map.insert("desc".to_string(), Value::Bool(true));
        }
        self.search(next)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = zh::path_from_url(BASE_URL, query) {
            return Ok(Paged { entries: vec![fetch_details(&path)], has_next_page: false });
        }
        let config = fetch_pconfig();
        let target = if query.is_empty() {
            signed_url(&format!("{API_BASE}/api/list/Search"), list_query(&request), &config)
        } else {
            signed_url(&format!("{RANK_BASE}/v3/list/briefsearch"), keyword_query(&request, query), &config)
        };
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &target, BASE_URL, SEARCH_FIXTURE);
        Ok(parse_search(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = zh::request_key(&request, "item").unwrap_or_else(|| "/play/1#1".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = zh::request_key(&request, "item").unwrap_or_else(|| "/play/1#1".to_string());
        let (vid, cid) = split_vid_cid(&path);
        let config = fetch_pconfig();
        let target = signed_url(&format!("{API_BASE}/v3/video/languagesplaylist"), vec![
            ("cinema", "1".to_string()),
            ("vid", vid),
            ("lsk", "1".to_string()),
            ("taxis", "0".to_string()),
            ("cid", cid.unwrap_or_default()),
        ], &config);
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &target, BASE_URL, PLAYLIST_FIXTURE);
        Ok(parse_episodes(&body))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = zh::request_key(&request, "episode").unwrap_or_else(|| "/1".to_string());
        let vid = key.trim_matches('/').to_string();
        let config = fetch_pconfig();
        let target = signed_url(&format!("{API_BASE}/v3/video/play"), vec![
            ("cinema", "1".to_string()),
            ("id", vid),
            ("a", "0".to_string()),
            ("lang", "none".to_string()),
            ("usersign", "1".to_string()),
            ("region", "SG".to_string()),
            ("device", "1".to_string()),
            ("isMasterSupport", "1".to_string()),
        ], &config);
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &target, BASE_URL, PLAY_FIXTURE);
        let response: CommonResponse<Play> = serde_json::from_str(&body).unwrap_or_default();
        let mut streams = response.data.info.into_iter().next().map(|play| {
            play.clarity.into_iter().filter_map(|clarity| {
                let path = clarity.path?;
                let clean = path.rtmp.split("?us=").next().unwrap_or(&path.rtmp).to_string();
                Some(zh::direct_stream(&clean, &clarity.title, BASE_URL))
            }).collect::<Vec<_>>()
        }).unwrap_or_default();
        zh::sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(zh::with_listing(&request, "popular"))?;
        let latest = self.list(zh::with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection { id: "popular".to_string(), title: "人气高低".to_string(), style: Some(HomeSectionStyle::Featured), entries: popular.entries, has_more: popular.has_next_page, ..HomeSection::default() },
            HomeSection { id: "latest".to_string(), title: "更新时间".to_string(), entries: latest.entries, has_more: latest.has_next_page, ..HomeSection::default() },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(zh::request_key(&request, "item").map(|path| zh::absolute_url(BASE_URL, &path)))
    }

    fn episode_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(None)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(path) = zh::path_from_url(BASE_URL, input) {
            return Ok(Some(UrlResolveResult { item: Some(fetch_details(&path)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn fetch_details(path: &str) -> CatalogItem {
    let (vid, _) = split_vid_cid(path);
    let config = fetch_pconfig();
    let target = signed_url(&format!("{API_BASE}/v3/video/detail"), vec![
        ("cinema", "1".to_string()),
        ("device", "1".to_string()),
        ("player", "CkPlayer".to_string()),
        ("tech", "HLS".to_string()),
        ("country", "HU".to_string()),
        ("lang", "cns".to_string()),
        ("v", "1".to_string()),
        ("id", vid),
        ("region", "SG".to_string()),
    ], &config);
    let body = zh::fetch_or_smoke_fixture(BASE_URL, &target, BASE_URL, DETAIL_FIXTURE);
    let response: CommonResponse<VideoDetail> = serde_json::from_str(&body).unwrap_or_default();
    if let Some(detail) = response.data.info.into_iter().next() {
        return CatalogItem {
            key: zh::path_key(BASE_URL, path),
            title: detail.title,
            cover: Some(detail.img_path),
            url: Some(zh::absolute_url(BASE_URL, path)),
            description: Some(format!("添加：{}\n更新：{}\n简介：{}", detail.add_date, detail.updateweekly, detail.contxt)),
            authors: detail.directors,
            artists: detail.stars,
            tags: detail.key_word.split(',').map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect(),
            language: Some(LANG.to_string()),
            content_rating: Some(RATING.to_string()),
            status: if detail.serial_count == 0 { ItemStatus::Completed } else { ItemStatus::Ongoing },
            initialized: true,
            ..CatalogItem::default()
        };
    }
    CatalogItem { key: zh::path_key(BASE_URL, path), title: path.trim_matches('/').to_string(), url: Some(zh::absolute_url(BASE_URL, path)), language: Some(LANG.to_string()), content_rating: Some(RATING.to_string()), ..CatalogItem::default() }
}

fn list_query(request: &Value) -> Vec<(&'static str, String)> {
    let mut query = vec![
        ("cinema", "1".to_string()),
        ("page", zh::page(request).to_string()),
        ("size", PAGE_SIZE.to_string()),
        ("isIndex", "-1".to_string()),
        ("isfree", "-1".to_string()),
    ];
    for key in ["cid", "region", "language", "year", "vipResource", "isserial", "orderby"] {
        if let Some(value) = zh::filter(request, key).or_else(|| request.get(key).and_then(Value::as_str).map(ToString::to_string)) {
            if !value.is_empty() {
                query.push((key, value));
            }
        }
    }
    let desc = request.get("desc").and_then(Value::as_bool).or_else(|| request.get("filters").and_then(|filters| filters.get("desc")).and_then(Value::as_bool)).unwrap_or(true);
    query.push(("desc", if desc { "1" } else { "0" }.to_string()));
    query
}

fn keyword_query(request: &Value, query: &str) -> Vec<(&'static str, String)> {
    vec![
        ("tags", query.to_string()),
        ("orderby", "4".to_string()),
        ("page", zh::page(request).to_string()),
        ("size", PAGE_SIZE.to_string()),
        ("desc", "1".to_string()),
        ("isserial", "-1".to_string()),
    ]
}

fn signed_url(base: &str, query: Vec<(&'static str, String)>, config: &PConfig) -> String {
    let raw = query.iter().map(|(key, value)| format!("{key}={}", url::query_escape(value))).collect::<Vec<_>>().join("&");
    let vv = signature(&raw, config);
    format!("{base}?{raw}&vv={vv}&pub={}", url::query_escape(&config.public_key))
}

fn signature(query: &str, config: &PConfig) -> String {
    let input = format!("{}&{}&{}", config.public_key, query.to_lowercase(), config.private_key);
    format!("{:x}", Md5::digest(input.as_bytes()))
}

fn fetch_pconfig() -> PConfig {
    let body = zh::fetch_or_smoke_fixture(BASE_URL, &format!("{BASE_URL}/list"), BASE_URL, PCONFIG_FIXTURE);
    let public_key = body.split("\"publicKey\":\"").nth(1).and_then(|v| v.split('"').next()).unwrap_or("pub").to_string();
    let private_key = body.split("\"privateKey\":[\"").nth(1).and_then(|v| v.split('"').next()).unwrap_or("priv").to_string();
    PConfig { public_key, private_key }
}

fn split_vid_cid(path: &str) -> (String, Option<String>) {
    let mut parts = path.trim_start_matches('/').split('#');
    let vid = parts.next().unwrap_or_default().split('/').next_back().unwrap_or_default().to_string();
    let cid = parts.next().map(ToString::to_string);
    (vid, cid)
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let response: CommonResponse<SearchResult> = serde_json::from_str(body).unwrap_or_default();
    let result = response.data.info.into_iter().next().map(|value| value.result).unwrap_or_default();
    Paged {
        has_next_page: result.len() as u64 >= PAGE_SIZE,
        entries: result.into_iter().map(|item| {
            zh::catalog_item(
                BASE_URL,
                format!("/play/{}#{}", item.key.unwrap_or(item.contxt), item.video_class_id),
                item.title,
                Some(item.img_path),
                LANG,
                RATING,
            )
        }).collect(),
    }
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let response: CommonResponse<PlayList> = serde_json::from_str(body).unwrap_or_default();
    let mut episodes = response.data.info.into_iter().next().map(|value| value.play_list).unwrap_or_default().into_iter().map(|item| {
        VideoEpisode {
            key: format!("/{}", item.key),
            title: Some(item.name.clone()),
            episode_number: zh::episode_number(&item.name),
            url: Some(format!("{BASE_URL}/play/{}", item.key)),
            language: Some(LANG.to_string()),
            ..VideoEpisode::default()
        }
    }).collect::<Vec<_>>();
    episodes.reverse();
    zh::dedupe_episodes(episodes)
}

struct PConfig {
    public_key: String,
    private_key: String,
}

#[derive(Default, Deserialize)]
struct CommonResponse<T> {
    data: ResponseData<T>,
}

#[derive(Default, Deserialize)]
struct ResponseData<T> {
    info: Vec<T>,
}

#[derive(Default, Deserialize)]
struct SearchResult {
    result: Vec<SearchResultItem>,
}

#[derive(Default, Deserialize)]
struct SearchResultItem {
    #[serde(rename = "imgPath")]
    img_path: String,
    key: Option<String>,
    title: String,
    #[serde(rename = "videoClassID")]
    video_class_id: String,
    contxt: String,
}

#[derive(Default, Deserialize)]
struct VideoDetail {
    add_date: String,
    contxt: String,
    updateweekly: String,
    #[serde(rename = "imgPath")]
    img_path: String,
    title: String,
    directors: Vec<String>,
    stars: Vec<String>,
    #[serde(rename = "keyWord")]
    key_word: String,
    #[serde(rename = "serialCount")]
    serial_count: i64,
}

#[derive(Default, Deserialize)]
struct PlayList {
    #[serde(rename = "playList")]
    play_list: Vec<PlayListItem>,
}

#[derive(Default, Deserialize)]
struct PlayListItem {
    key: String,
    name: String,
}

#[derive(Default, Deserialize)]
struct Play {
    clarity: Vec<PlayClarity>,
}

#[derive(Default, Deserialize)]
struct PlayClarity {
    title: String,
    path: Option<PlayPath>,
}

#[derive(Default, Deserialize)]
struct PlayPath {
    rtmp: String,
}

const PCONFIG_FIXTURE: &str = r#"<script>window.injectJson={"pConfig":{"publicKey":"pub","privateKey":["priv"]}}</script>"#;
const SEARCH_FIXTURE: &str = r#"{"data":{"info":[{"result":[{"imgPath":"/s.jpg","key":"1","title":"Sample","videoClassID":"6","contxt":"1"}]}]}}"#;
const DETAIL_FIXTURE: &str = r#"{"data":{"info":[{"add_date":"","contxt":"Sample","updateweekly":"","imgPath":"/s.jpg","title":"Sample","directors":[],"stars":[],"keyWord":"动漫","serialCount":0}]}}"#;
const PLAYLIST_FIXTURE: &str = r#"{"data":{"info":[{"playList":[{"key":"1","name":"第1集","updateDate":""}]}]}}"#;
const PLAY_FIXTURE: &str = r#"{"data":{"info":[{"clarity":[{"title":"720P","path":{"rtmp":"https://media.example/sample.m3u8?us=1"}}]}]}}"#;

export_video_source!(SOURCE);
