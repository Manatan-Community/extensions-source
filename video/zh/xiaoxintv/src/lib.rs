#[path = "../../_shared/zh_video.rs"]
mod zh;

use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{sdk::SearchRequest, url};
use scraper::ElementRef;
use serde_json::Value;

const SOURCE: XiaoxinTv = XiaoxinTv;
const BASE_URL: &str = "https://xiaoxintv.cc";
const LANG: &str = "zh";
const RATING: &str = "safe";

struct XiaoxinTv;

impl VideoSource for XiaoxinTv {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let mut next = request.clone();
        if let Some(map) = next.as_object_mut() {
            if zh::listing(&request) == "popular" {
                map.insert("sort".to_string(), Value::String("hits".to_string()));
            }
            map.entry("category".to_string()).or_insert(Value::String("5".to_string()));
        }
        self.search(next)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = zh::path_from_url(BASE_URL, query) {
            return Ok(Paged { entries: vec![fetch_details(&path)], has_next_page: false });
        }
        let target = if query.is_empty() { filter_url(&request) } else { keyword_url(query, zh::page(&request)) };
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &target, BASE_URL, LIST_FIXTURE);
        Ok(if query.is_empty() { parse_filter_list(&body) } else { parse_keyword_list(&body) })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = zh::request_key(&request, "item").unwrap_or_else(|| "/index.php/vod/detail/id/1.html".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = zh::request_key(&request, "item").unwrap_or_else(|| "/index.php/vod/detail/id/1.html".to_string());
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &zh::absolute_url(BASE_URL, &path), BASE_URL, DETAILS_FIXTURE);
        Ok(parse_episodes(&body))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = zh::request_key(&request, "episode").unwrap_or_else(|| "/index.php/vod/play/id/1/sid/1/nid/1.html".to_string());
        let referer = zh::absolute_url(BASE_URL, &path);
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &referer, BASE_URL, PLAYER_FIXTURE);
        Ok(find_video_url(&body).map(|stream| vec![zh::direct_stream(&stream, "小宝影院", &referer)]).unwrap_or_default())
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(zh::with_listing(&request, "popular"))?;
        let latest = self.list(zh::with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection { id: "popular".to_string(), title: "人气".to_string(), style: Some(HomeSectionStyle::Featured), entries: popular.entries, has_more: popular.has_next_page, ..HomeSection::default() },
            HomeSection { id: "latest".to_string(), title: "时间".to_string(), entries: latest.entries, has_more: latest.has_next_page, ..HomeSection::default() },
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
            if path.contains("/vod/play/") {
                return Ok(Some(UrlResolveResult { episode: Some(serde_json::json!({"key": path, "url": input, "language": LANG})), url: Some(input.to_string()), ..UrlResolveResult::default() }));
            }
            return Ok(Some(UrlResolveResult { item: Some(fetch_details(&path)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn filter_url(request: &Value) -> String {
    let mut parts = Vec::new();
    parts.push(format!("id/{}", zh::filter(request, "category").unwrap_or_else(|| "5".to_string())));
    for (key, segment) in [("region", "area"), ("sort", "by"), ("class", "class"), ("type", "id"), ("lang", "lang"), ("year", "year")] {
        if let Some(value) = zh::filter(request, key).or_else(|| request.get(key).and_then(Value::as_str).map(ToString::to_string)) {
            if !value.is_empty() && !(key == "sort" && value == "time") {
                parts.push(format!("{segment}/{value}"));
            }
        }
    }
    if zh::page(request) > 1 {
        parts.push(format!("page/{}", zh::page(request)));
    }
    format!("{BASE_URL}/index.php/vod/show/{}.html", parts.join("/"))
}

fn keyword_url(query: &str, page: u64) -> String {
    if page > 1 {
        format!("{BASE_URL}/index.php/vod/search/page/{page}/wd/{}.html", url::query_escape(query))
    } else {
        format!("{BASE_URL}/index.php/vod/search?wd={}", url::query_escape(query))
    }
}

fn parse_filter_list(body: &str) -> Paged<CatalogItem> {
    parse_cards(body, ".myui-vodlist__box", ".myui-vodlist__thumb")
}

fn parse_keyword_list(body: &str) -> Paged<CatalogItem> {
    parse_cards(body, "#searchList li", "a.myui-vodlist__thumb")
}

fn parse_cards(body: &str, item_selector: &str, thumb_selector: &str) -> Paged<CatalogItem> {
    let doc = zh::document(body);
    let mut entries = Vec::new();
    for item in doc.select(&zh::selector(item_selector)) {
        if let Some(card) = parse_card(item, thumb_selector) {
            entries.push(card);
        }
    }
    zh::paged(entries, body)
}

fn parse_card(item: ElementRef<'_>, thumb_selector: &str) -> Option<CatalogItem> {
    let thumb = item.select(&zh::selector(thumb_selector)).next()?;
    let href = zh::attr(&thumb, "href")?;
    let title = zh::attr(&thumb, "title").or_else(|| item.select(&zh::selector(".title")).next().map(|n| zh::text(&n)))?;
    let cover = zh::attr(&thumb, "data-original")
        .or_else(|| thumb.select(&zh::selector("img")).next().and_then(|img| zh::attr(&img, "data-original")));
    Some(zh::catalog_item(BASE_URL, zh::path_key(BASE_URL, &href), title, cover, LANG, RATING))
}

fn fetch_details(path: &str) -> CatalogItem {
    let url = zh::absolute_url(BASE_URL, path);
    let body = zh::fetch_or_smoke_fixture(BASE_URL, &url, BASE_URL, DETAILS_FIXTURE);
    let doc = zh::document(&body);
    CatalogItem {
        key: zh::path_key(BASE_URL, path),
        title: zh::first_text(&doc, &[".myui-content__detail .title", "h1"]).unwrap_or_else(|| path.trim_matches('/').replace('/', " ")),
        cover: zh::first_attr(&doc, &[".myui-vodlist__thumb.picture img", "img"], "data-original").map(|value| zh::absolute_url(BASE_URL, &value)),
        url: Some(url),
        authors: data_text(&doc, "主演").into_iter().collect(),
        artists: data_text(&doc, "导演").into_iter().collect(),
        description: data_text(&doc, "简介"),
        language: Some(LANG.to_string()),
        content_rating: Some(RATING.to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn data_text(doc: &scraper::Html, label: &str) -> Option<String> {
    doc.select(&zh::selector("p.data"))
        .map(|node| zh::text(&node))
        .find(|text| text.contains(label))
        .map(|text| text.replace(&format!("{label}："), "").trim().to_string())
        .filter(|text| !text.is_empty())
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let doc = zh::document(body);
    let mut out = Vec::new();
    let items: Vec<_> = doc.select(&zh::selector("#playlist1 ul li")).collect();
    for (index, item) in items.into_iter().enumerate() {
        let Some(link) = item.select(&zh::selector("a")).next() else { continue; };
        let Some(href) = zh::attr(&link, "href") else { continue; };
        let title = zh::attr(&item, "title").or_else(|| Some(zh::text(&link))).filter(|v| !v.is_empty());
        let key = zh::path_key(BASE_URL, &href);
        out.push(VideoEpisode { key: key.clone(), title, episode_number: Some(index as f32), url: Some(zh::absolute_url(BASE_URL, &key)), language: Some(LANG.to_string()), ..VideoEpisode::default() });
    }
    out.reverse();
    zh::dedupe_episodes(out)
}

fn find_video_url(body: &str) -> Option<String> {
    let marker = "player_aaaa=";
    let data = body.split(marker).nth(1)?.split("</script>").next()?.trim().trim_end_matches(';');
    serde_json::from_str::<Value>(data).ok()?.get("url")?.as_str().map(ToString::to_string)
}

const LIST_FIXTURE: &str = r#"<div class="myui-vodlist__box"><a class="myui-vodlist__thumb" href="/index.php/vod/detail/id/1.html" title="Sample" data-original="/s.jpg"></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="title">Sample</h1><div id="playlist1"><ul><li title="第1集"><a href="/index.php/vod/play/id/1/sid/1/nid/1.html">第1集</a></li></ul></div>"#;
const PLAYER_FIXTURE: &str = r#"<script>var player_aaaa={"url":"https://media.example/sample.m3u8"}</script>"#;

export_video_source!(SOURCE);
