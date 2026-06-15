#[path = "../../_shared/zh_video.rs"]
mod zh;

use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, abi::{ExtensionResult, system_time}, export_video_source, source::VideoSource,
};
use manatan_shared::{sdk::SearchRequest, url};
use md5::{Digest, Md5};
use scraper::ElementRef;
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Xfani = Xfani;
const BASE_URL: &str = "https://dm.xifanacg.com";
const LANG: &str = "zh";
const RATING: &str = "safe";
const UID: &str = "DCC147D11943AF75";

struct Xfani;

impl VideoSource for Xfani {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let mut next = request.clone();
        if let Some(map) = next.as_object_mut() {
            map.insert("sort".to_string(), Value::String(if zh::listing(&request) == "popular" { "hits" } else { "time" }.to_string()));
            map.entry("type".to_string()).or_insert(Value::String("1".to_string()));
        }
        self.search(next)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = zh::path_from_url(BASE_URL, query) {
            return Ok(Paged { entries: vec![fetch_details(&path)], has_next_page: false });
        }
        if !query.is_empty() {
            let target = if zh::page(&request) <= 1 {
                format!("{BASE_URL}/search.html?wd={}", url::query_escape(query))
            } else {
                format!("{BASE_URL}/search/wd/{}/page/{}.html", url::query_escape(query), zh::page(&request))
            };
            let body = zh::fetch_or_smoke_fixture(BASE_URL, &target, BASE_URL, SEARCH_FIXTURE);
            return Ok(parse_html_search(&body));
        }
        let body = fetch_api_list(&request)?;
        Ok(parse_api_list(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = zh::request_key(&request, "item").unwrap_or_else(|| "/bangumi/1.html".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = zh::request_key(&request, "item").unwrap_or_else(|| "/bangumi/1.html".to_string());
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &zh::absolute_url(BASE_URL, &path), BASE_URL, DETAILS_FIXTURE);
        Ok(parse_episodes(&body, &request))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = zh::request_key(&request, "episode").unwrap_or_else(|| "/play/1-1-1.html".to_string());
        let referer = zh::absolute_url(BASE_URL, &path);
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &referer, BASE_URL, PLAYER_FIXTURE);
        let Some(stream) = find_video_url(&body) else { return Ok(Vec::new()); };
        Ok(vec![zh::direct_stream(&stream, "稀饭动漫", &referer)])
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(zh::with_listing(&request, "popular"))?;
        let latest = self.list(zh::with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection { id: "popular".to_string(), title: "按热门".to_string(), style: Some(HomeSectionStyle::Featured), entries: popular.entries, has_more: popular.has_next_page, ..HomeSection::default() },
            HomeSection { id: "latest".to_string(), title: "按最新".to_string(), entries: latest.entries, has_more: latest.has_next_page, ..HomeSection::default() },
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
            if path.contains("/play/") {
                return Ok(Some(UrlResolveResult { episode: Some(serde_json::json!({"key": path, "url": input, "language": LANG})), url: Some(input.to_string()), ..UrlResolveResult::default() }));
            }
            return Ok(Some(UrlResolveResult { item: Some(fetch_details(&path)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn fetch_api_list(request: &Value) -> ExtensionResult<String> {
    let target = format!("{BASE_URL}/index.php/api/vod");
    let time = system_time().map(|time| time.unix_seconds.max(0)).unwrap_or(0);
    let mut form = vec![
        ("page".to_string(), zh::page(request).to_string()),
        ("time".to_string(), time.to_string()),
        ("key".to_string(), generate_key(time)),
        ("type".to_string(), zh::filter(request, "type").unwrap_or_else(|| "1".to_string())),
    ];
    for (id, field) in [("class", "class"), ("year", "year"), ("version", "version"), ("letter", "letter"), ("sort", "by")] {
        if let Some(value) = zh::filter(request, id).or_else(|| request.get(id).and_then(Value::as_str).map(ToString::to_string)) {
            if !value.is_empty() {
                form.push((field.to_string(), value));
            }
        }
    }
    let borrowed = form.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect::<Vec<_>>();
    match zh::client(BASE_URL, BASE_URL).post(&target).form(&borrowed).send_text() {
        Ok(body) => Ok(body),
        Err(error) if zh::is_smoke_http_disabled(&error) => Ok(API_FIXTURE.to_string()),
        Err(error) => Err(error),
    }
}

fn generate_key(time: i64) -> String {
    format!("{:x}", Md5::digest(format!("DS{time}{UID}").as_bytes()))
}

fn parse_api_list(body: &str) -> Paged<CatalogItem> {
    let response: VodResponse = serde_json::from_str(body).unwrap_or_default();
    Paged {
        has_next_page: !response.list.is_empty() && response.page * response.limit < response.total,
        entries: response.list.into_iter().map(|item| {
            let mut catalog = zh::catalog_item(
                BASE_URL,
                format!("/bangumi/{}.html", item.vod_id),
                item.vod_name,
                Some(if item.vod_pic_thumb.is_empty() { item.vod_pic } else { item.vod_pic_thumb }),
                LANG,
                RATING,
            );
            catalog.authors = vec![item.vod_actor.replace(",,,", "")].into_iter().filter(|v| !v.is_empty()).collect();
            catalog.description = Some(item.vod_blurb);
            catalog.tags = item.vod_class.split(',').map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect();
            catalog
        }).collect(),
    }
}

fn parse_html_search(body: &str) -> Paged<CatalogItem> {
    let doc = zh::document(body);
    let mut entries = Vec::new();
    for item in doc.select(&zh::selector("div.search-list")) {
        let Some(link) = item.select(&zh::selector("div.detail-info > a")).next() else { continue; };
        let Some(href) = zh::attr(&link, "href") else { continue; };
        let cover = item.select(&zh::selector("div.detail-pic img[data-src]")).next().and_then(|img| zh::attr(&img, "data-src"));
        entries.push(zh::catalog_item(BASE_URL, zh::path_key(BASE_URL, &href), zh::text(&link), cover, LANG, RATING));
    }
    zh::paged(entries, body)
}

fn fetch_details(path: &str) -> CatalogItem {
    let url = zh::absolute_url(BASE_URL, path);
    let body = zh::fetch_or_smoke_fixture(BASE_URL, &url, BASE_URL, DETAILS_FIXTURE);
    let doc = zh::document(&body);
    CatalogItem {
        key: zh::path_key(BASE_URL, path),
        title: zh::first_text(&doc, &[".slide-info-title", "h1"]).unwrap_or_else(|| path.trim_matches('/').replace('/', " ")),
        cover: zh::first_attr(&doc, &["img"], "data-src").or_else(|| zh::first_attr(&doc, &["img"], "src")).map(|value| zh::absolute_url(BASE_URL, &value)),
        url: Some(url),
        description: zh::first_text(&doc, &["#height_limit.text", ".text"]),
        tags: zh::first_text(&doc, &[".slide-info"]).into_iter().collect(),
        language: Some(LANG.to_string()),
        content_rating: Some(RATING.to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, request: &Value) -> Vec<VideoEpisode> {
    let doc = zh::document(body);
    let source_index = zh::pref(request, "video_source").and_then(|value| value.parse::<usize>().ok()).unwrap_or(0);
    let groups: Vec<_> = doc.select(&zh::selector("ul.anthology-list-play.size, .anthology-list-play")).collect();
    let group = groups.get(source_index).or_else(|| groups.first());
    let Some(group) = group else { return Vec::new(); };
    let mut out = Vec::new();
    for link in group.select(&zh::selector("li > a, a")) {
        let Some(href) = zh::attr(&link, "href") else { continue; };
        let title = zh::text(&link);
        let key = zh::path_key(BASE_URL, &href);
        out.push(VideoEpisode { key: key.clone(), title: Some(title.clone()), episode_number: zh::episode_number(&title), url: Some(zh::absolute_url(BASE_URL, &key)), language: Some(LANG.to_string()), ..VideoEpisode::default() });
    }
    out.reverse();
    zh::dedupe_episodes(out)
}

fn find_video_url(body: &str) -> Option<String> {
    let data = body.split("player_aaaa=").nth(1)?.split("</script>").next()?.trim().trim_end_matches(';');
    serde_json::from_str::<Value>(data).ok()?.get("url")?.as_str().map(ToString::to_string)
}

#[derive(Default, Deserialize)]
struct VodResponse {
    page: u64,
    limit: u64,
    total: u64,
    list: Vec<VodInfo>,
}

#[derive(Default, Deserialize)]
struct VodInfo {
    vod_id: u64,
    vod_name: String,
    vod_pic: String,
    #[serde(default)]
    vod_pic_thumb: String,
    #[serde(default)]
    vod_class: String,
    #[serde(default)]
    vod_actor: String,
    #[serde(default)]
    vod_blurb: String,
}

const API_FIXTURE: &str = r#"{"page":1,"limit":20,"total":1,"list":[{"vod_id":1,"vod_name":"Sample","vod_pic":"/s.jpg","vod_class":"动画","vod_actor":"","vod_blurb":""}]}"#;
const SEARCH_FIXTURE: &str = r#"<div class="search-list"><div class="detail-info"><a href="/bangumi/1.html">Sample</a></div><div class="detail-pic"><img data-src="/s.jpg"></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="slide-info-title">Sample</h1><ul class="anthology-list-play size"><li><a href="/play/1-1-1.html"><span>第1集</span></a></li></ul>"#;
const PLAYER_FIXTURE: &str = r#"<script>var player_aaaa={"url":"https://media.example/sample.m3u8"}</script>"#;

export_video_source!(SOURCE);
