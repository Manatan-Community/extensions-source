#[path = "../../_shared/zh_video.rs"]
mod zh;

use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{sdk::SearchRequest, url};
use scraper::ElementRef;
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Nivod = Nivod;
const BASE_URL: &str = "https://www.nivod.cc";
const SEARCH_BASE: &str = "https://e.kortw.cc";
const LANG: &str = "zh";
const RATING: &str = "safe";

struct Nivod;

impl VideoSource for Nivod {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &format!("{BASE_URL}/class.html?channel=anime"), BASE_URL, HOME_FIXTURE);
        Ok(parse_home_section(&body, zh::listing(&request) == "latest"))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = zh::path_from_url(BASE_URL, query) {
            return Ok(Paged { entries: vec![fetch_details(&path)], has_next_page: false });
        }
        let target = if query.is_empty() { filter_url(&request) } else { format!("{SEARCH_BASE}/vodsearch/-------------.html?keyword={}", url::query_escape(query)) };
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &target, BASE_URL, SEARCH_FIXTURE);
        Ok(parse_search(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = zh::request_key(&request, "item").unwrap_or_else(|| "/detail/1.html".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = zh::request_key(&request, "item").unwrap_or_else(|| "/detail/1.html".to_string());
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &zh::absolute_url(BASE_URL, &path), BASE_URL, DETAILS_FIXTURE);
        Ok(parse_episodes(&body))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = zh::request_key(&request, "episode").unwrap_or_else(|| "/play/1.html".to_string());
        let referer = zh::absolute_url(BASE_URL, &path);
        let page = zh::fetch_or_smoke_fixture(BASE_URL, &referer, BASE_URL, PLAYER_PAGE_FIXTURE);
        let Some(info_path) = page.split("xhr_playinfo").nth(1).and_then(|part| part.split("url = '").nth(1)).and_then(|part| part.split('\'').next()) else {
            return Ok(Vec::new());
        };
        let info_url = zh::absolute_url(BASE_URL, info_path);
        let body = match zh::fetch(BASE_URL, &info_url, &referer) {
            Ok(body) => body,
            Err(error) if zh::is_smoke_http_disabled(&error) => PLAY_INFO_FIXTURE.to_string(),
            Err(error) => return Err(error),
        };
        let info: PlayInfo = serde_json::from_str(&body).unwrap_or_default();
        let mut streams = info.pdatas.into_iter().map(|route| {
            let quality = format!("{}云", route.from.chars().take(2).collect::<String>().to_uppercase());
            zh::direct_stream(&route.play_url, &quality, &referer)
        }).collect::<Vec<_>>();
        zh::sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let latest = self.list(zh::with_listing(&request, "latest"))?;
        let popular = self.list(zh::with_listing(&request, "popular"))?;
        Ok(vec![
            HomeSection { id: "popular".to_string(), title: "热门".to_string(), style: Some(HomeSectionStyle::Featured), entries: popular.entries, has_more: popular.has_next_page, ..HomeSection::default() },
            HomeSection { id: "latest".to_string(), title: "最新".to_string(), entries: latest.entries, has_more: latest.has_next_page, ..HomeSection::default() },
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

fn filter_url(request: &Value) -> String {
    let mut parts = vec![format!("channel={}", zh::filter(request, "channel").unwrap_or_else(|| "anime".to_string()))];
    for key in ["region", "showtype", "year"] {
        if let Some(value) = zh::filter(request, key) {
            if !value.is_empty() {
                parts.push(format!("{key}={}", url::query_escape(&value)));
            }
        }
    }
    format!("{BASE_URL}/filter.html?{}", parts.join("&"))
}

fn parse_home_section(body: &str, latest: bool) -> Paged<CatalogItem> {
    let doc = zh::document(body);
    let layouts: Vec<_> = doc.select(&zh::selector(".tl-layout")).collect();
    let layout = layouts.get(if latest { 0 } else { 1 }).or_else(|| layouts.first());
    let mut entries = Vec::new();
    if let Some(layout) = layout {
        for node in layout.select(&zh::selector(".qy-mod-img.vertical")) {
            if let Some(item) = parse_home_card(node) {
                entries.push(item);
            }
        }
    }
    Paged { entries: zh::dedupe_items(entries), has_next_page: false }
}

fn parse_home_card(node: ElementRef<'_>) -> Option<CatalogItem> {
    let link = node.select(&zh::selector("a.qy-mod-link, a")).next()?;
    let href = zh::attr(&link, "href")?;
    let title = node.select(&zh::selector(".title-wrap .main a, .main a, a")).next().map(|n| zh::text(&n)).filter(|v| !v.is_empty())?;
    let cover = node.select(&zh::selector("picture img, img")).next().and_then(|img| zh::attr(&img, "src"));
    Some(zh::catalog_item(BASE_URL, zh::path_key(BASE_URL, &href), title, cover, LANG, RATING))
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let doc = zh::document(body);
    let mut entries = Vec::new();
    for node in doc.select(&zh::selector(".qy-list-img.vertical")) {
        let Some(link) = node.select(&zh::selector("a.qy-mod-link, a")).next() else { continue; };
        let Some(href) = zh::attr(&link, "href") else { continue; };
        let title = node.select(&zh::selector(".title-wrap .main a, .main a, a")).next().map(|n| zh::text(&n)).unwrap_or_else(|| href.clone());
        let cover = node.select(&zh::selector("div.qy-mod-cover")).next().and_then(|cover| zh::attr(&cover, "style").and_then(|style| style.split("url(").nth(1).map(|v| v.split(')').next().unwrap_or_default().to_string())))
            .or_else(|| node.select(&zh::selector("img")).next().and_then(|img| zh::attr(&img, "src")));
        entries.push(zh::catalog_item(BASE_URL, zh::path_key(BASE_URL, &href), title, cover, LANG, RATING));
    }
    zh::paged(entries, body)
}

fn fetch_details(path: &str) -> CatalogItem {
    let url = zh::absolute_url(BASE_URL, path);
    let body = zh::fetch_or_smoke_fixture(BASE_URL, &url, BASE_URL, DETAILS_FIXTURE);
    let doc = zh::document(&body);
    CatalogItem {
        key: zh::path_key(BASE_URL, path),
        title: zh::first_text(&doc, &[".right-title", "h1"]).unwrap_or_else(|| path.trim_matches('/').replace('/', " ")),
        cover: zh::first_attr(&doc, &[".left-img-c img", "img"], "src").map(|value| zh::absolute_url(BASE_URL, &value)),
        url: Some(url),
        description: right_type_text(&doc, 6),
        authors: right_type_text(&doc, 4).into_iter().collect(),
        artists: right_type_text(&doc, 5).into_iter().collect(),
        tags: doc.select(&zh::selector(".right-label-c .right-label")).map(|n| zh::text(&n)).filter(|v| !v.is_empty()).collect(),
        language: Some(LANG.to_string()),
        content_rating: Some(RATING.to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn right_type_text(doc: &scraper::Html, nth: usize) -> Option<String> {
    doc.select(&zh::selector(".right-type-c")).nth(nth.saturating_sub(1)).and_then(|node| node.select(&zh::selector(".right-label")).next().map(|v| zh::text(&v))).filter(|v| !v.is_empty())
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let doc = zh::document(body);
    let mut out = Vec::new();
    for link in doc.select(&zh::selector(".list-ruku a")) {
        let Some(href) = zh::attr(&link, "href") else { continue; };
        let title = link.select(&zh::selector(".item")).next().map(|n| zh::text(&n)).unwrap_or_else(|| zh::text(&link));
        let key = zh::path_key(BASE_URL, &href);
        out.push(VideoEpisode { key: key.clone(), title: Some(title.clone()), episode_number: zh::episode_number(&title), url: Some(zh::absolute_url(BASE_URL, &key)), language: Some(LANG.to_string()), ..VideoEpisode::default() });
    }
    zh::dedupe_episodes(out)
}

#[derive(Default, Deserialize)]
struct PlayInfo {
    pdatas: Vec<RouteInfo>,
}

#[derive(Default, Deserialize)]
struct RouteInfo {
    #[serde(rename = "playurl")]
    play_url: String,
    from: String,
}

const HOME_FIXTURE: &str = r#"<div class="tl-layout"><div class="qy-mod-img vertical"><a class="qy-mod-link" href="/detail/1.html"></a><div class="title-wrap"><div class="main"><a>Sample</a></div></div><picture><img src="/s.jpg"></picture></div></div><div class="tl-layout"><div class="qy-mod-img vertical"><a class="qy-mod-link" href="/detail/2.html"></a><div class="title-wrap"><div class="main"><a>Popular</a></div></div><picture><img src="/p.jpg"></picture></div></div>"#;
const SEARCH_FIXTURE: &str = r#"<div class="qy-list-img vertical"><a class="qy-mod-link" href="/detail/1.html"></a><div class="title-wrap"><div class="main"><a>Sample</a></div></div><div class="qy-mod-cover" style="background-image:url(/s.jpg)"></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="right-title">Sample</h1><div class="list-ruku"><a href="/play/1.html"><span class="item">第1集</span></a></div>"#;
const PLAYER_PAGE_FIXTURE: &str = r#"<script>var xhr_playinfo=true; url = '/playinfo.json'</script>"#;
const PLAY_INFO_FIXTURE: &str = r#"{"pdatas":[{"playurl":"https://media.example/sample.m3u8","from":"hw","name":"default"}]}"#;

export_video_source!(SOURCE);
