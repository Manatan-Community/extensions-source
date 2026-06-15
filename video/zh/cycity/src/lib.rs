#[path = "../../_shared/zh_video.rs"]
mod zh;

use aes::Aes128;
use base64::{Engine as _, engine::general_purpose};
use cbc::{
    Decryptor,
    cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7},
};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{sdk::SearchRequest, url};
use md5::{Digest, Md5};
use scraper::ElementRef;
use serde::Deserialize;
use serde_json::Value;

type Aes128CbcDec = Decryptor<Aes128>;

const SOURCE: Cycity = Cycity;
const BASE_URL: &str = "https://www.cycani.org";
const API_URL: &str = "https://www.cycani.org/index.php/ds_api";
const PARSE_URL: &str = "https://player.cycanime.com/?url=";
const LANG: &str = "zh";
const RATING: &str = "safe";

struct Cycity;

impl VideoSource for Cycity {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = if zh::listing(&request) == "latest" {
            vod_list_url("time", zh::page(&request))
        } else if zh::pref_bool(&request, "popular_weekly_schedule", false) {
            weekly_url()
        } else {
            vod_list_url("hits", zh::page(&request))
        };
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &target, BASE_URL, LIST_FIXTURE);
        if body.trim_start().starts_with('{') {
            Ok(parse_weekly(&body))
        } else {
            Ok(parse_vod_list(&body))
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(path) = zh::path_from_url(BASE_URL, query) {
            return Ok(Paged { entries: vec![fetch_details(&path)], has_next_page: false });
        }
        let target = if query.is_empty() { filter_url(&request) } else { search_url(query, zh::page(&request)) };
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &target, BASE_URL, LIST_FIXTURE);
        Ok(if query.is_empty() { parse_vod_list(&body) } else { parse_search(&body) })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = zh::request_key(&request, "item").unwrap_or_else(|| "/bangumi/1.html".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = zh::request_key(&request, "item").unwrap_or_else(|| "/bangumi/1.html".to_string());
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &zh::absolute_url(BASE_URL, &path), BASE_URL, DETAILS_FIXTURE);
        Ok(parse_episodes(&body))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = zh::request_key(&request, "episode").unwrap_or_else(|| "/play/1-1-1.html".to_string());
        let referer = zh::absolute_url(BASE_URL, &path);
        let body = zh::fetch_or_smoke_fixture(BASE_URL, &referer, BASE_URL, PLAYER_FIXTURE);
        let Some(encoded) = extract_player_url(&body) else { return Ok(Vec::new()); };
        let decoded = general_purpose::STANDARD.decode(encoded).ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(|value| percent_decode(&value))
            .unwrap_or_default();
        if decoded.is_empty() {
            return Ok(Vec::new());
        }
        let parser = format!("{PARSE_URL}{decoded}");
        let parser_body = match zh::fetch(BASE_URL, &parser, &referer) {
            Ok(body) => body,
            Err(error) if zh::is_smoke_http_disabled(&error) => PARSER_FIXTURE.to_string(),
            Err(error) => return Err(error),
        };
        let stream_url = decrypt_parser(&parser_body).unwrap_or(decoded);
        Ok(vec![zh::direct_stream(&stream_url, "默认", &referer)])
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(zh::with_listing(&request, "popular"))?;
        let latest = self.list(zh::with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection { id: "popular".to_string(), title: "热门动画".to_string(), style: Some(HomeSectionStyle::Featured), entries: popular.entries, has_more: popular.has_next_page, ..HomeSection::default() },
            HomeSection { id: "latest".to_string(), title: "最新动画".to_string(), entries: latest.entries, has_more: latest.has_next_page, ..HomeSection::default() },
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

fn vod_list_url(by: &str, page: u64) -> String {
    format!("{BASE_URL}/show/20/by/{by}/page/{page}.html")
}

fn weekly_url() -> String {
    format!("{API_URL}/weekday?weekday=一")
}

fn filter_url(request: &Value) -> String {
    let channel = zh::filter(request, "type").unwrap_or_else(|| "20".to_string());
    let mut parts = vec![format!("show/{channel}")];
    if let Some(class) = zh::filter(request, "class") {
        if !class.is_empty() {
            parts.push(format!("class/{class}"));
        }
    }
    if let Some(year) = zh::filter(request, "year") {
        if !year.is_empty() {
            parts.push(format!("year/{year}"));
        }
    }
    parts.push(format!("page/{}.html", zh::page(request)));
    format!("{BASE_URL}/{}", parts.join("/"))
}

fn search_url(query: &str, page: u64) -> String {
    format!("{BASE_URL}/search/wd/{}/page/{page}.html", url::query_escape(query))
}

fn parse_vod_list(body: &str) -> Paged<CatalogItem> {
    let doc = zh::document(body);
    let mut entries = Vec::new();
    for node in doc.select(&zh::selector(".public-list-box")) {
        if let Some(item) = parse_public_box(node) {
            entries.push(item);
        }
    }
    zh::paged(entries, body)
}

fn parse_public_box(node: ElementRef<'_>) -> Option<CatalogItem> {
    let link = node.select(&zh::selector(".public-list-button a, a")).next()?;
    let href = zh::attr(&link, "href")?;
    let title = zh::text(&link);
    let cover = node.select(&zh::selector("img")).next().and_then(|img| zh::attr(&img, "data-src").or_else(|| zh::attr(&img, "src")));
    Some(zh::catalog_item(BASE_URL, zh::path_key(BASE_URL, &href), title, cover, LANG, RATING))
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let doc = zh::document(body);
    let mut entries = Vec::new();
    for node in doc.select(&zh::selector(".search-list")) {
        let Some(link) = node.select(&zh::selector(".detail-info > a, a")).next() else { continue; };
        let Some(href) = zh::attr(&link, "href") else { continue; };
        let image = node.select(&zh::selector(".detail-pic img[data-src], img")).next();
        let title = image.as_ref().and_then(|img| zh::attr(img, "alt")).unwrap_or_else(|| zh::text(&link));
        let cover = image.and_then(|img| zh::attr(&img, "data-src").or_else(|| zh::attr(&img, "src")));
        entries.push(zh::catalog_item(BASE_URL, zh::path_key(BASE_URL, &href), title, cover, LANG, RATING));
    }
    zh::paged(entries, body)
}

fn parse_weekly(body: &str) -> Paged<CatalogItem> {
    let response: VodResponse = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: response.list.into_iter().map(|item| {
            let mut catalog = zh::catalog_item(BASE_URL, format!("/bangumi/{}.html", item.id), item.name, Some(item.pic), LANG, RATING);
            catalog.authors = vec![item.actor.replace(",,,", "")].into_iter().filter(|v| !v.is_empty()).collect();
            catalog
        }).collect(),
        has_next_page: false,
    }
}

fn fetch_details(path: &str) -> CatalogItem {
    let url = zh::absolute_url(BASE_URL, path);
    let body = zh::fetch_or_smoke_fixture(BASE_URL, &url, BASE_URL, DETAILS_FIXTURE);
    let doc = zh::document(&body);
    let remark = zh::first_text(&doc, &[".slide-info-remarks"]);
    CatalogItem {
        key: zh::path_key(BASE_URL, path),
        title: zh::first_text(&doc, &[".slide-info-title", "h1"]).unwrap_or_else(|| path.trim_matches('/').replace('/', " ")),
        cover: zh::first_attr(&doc, &["img"], "data-src").or_else(|| zh::first_attr(&doc, &["img"], "src")).map(|v| zh::absolute_url(BASE_URL, &v)),
        url: Some(url),
        description: zh::first_text(&doc, &["#height_limit.text", ".text"]),
        tags: doc.select(&zh::selector(".slide-info a")).map(|n| zh::text(&n)).filter(|v| !v.is_empty()).collect(),
        language: Some(LANG.to_string()),
        content_rating: Some(RATING.to_string()),
        status: match remark.as_deref() { Some("已完结") => ItemStatus::Completed, Some(value) if value.contains('|') => ItemStatus::Ongoing, _ => ItemStatus::Unknown },
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let doc = zh::document(body);
    let hosts: Vec<_> = doc.select(&zh::selector(".anthology-tab a")).map(|node| zh::text(&node)).collect();
    let mut out = Vec::new();
    for (group_index, group) in doc.select(&zh::selector(".anthology-list-play")).enumerate() {
        let host = hosts.get(group_index).cloned();
        for link in group.select(&zh::selector("a")) {
            let Some(href) = zh::attr(&link, "href") else { continue; };
            let title = zh::text(&link);
            let key = zh::path_key(BASE_URL, &href);
            out.push(VideoEpisode {
                key: key.clone(),
                title: Some(title.clone()),
                episode_number: zh::episode_number(&title),
                url: Some(zh::absolute_url(BASE_URL, &key)),
                language: Some(LANG.to_string()),
                labels: host.clone().into_iter().collect(),
                ..VideoEpisode::default()
            });
        }
    }
    out.reverse();
    zh::dedupe_episodes(out)
}

fn extract_player_url(body: &str) -> Option<&str> {
    let start = body.find("player_aaaa")?;
    let chunk = &body[start..];
    chunk.split("\"url\"").nth(1)?.split('"').nth(2)
}

fn decrypt_parser(body: &str) -> Option<String> {
    let keys: Vec<_> = body.split("now_").skip(1).filter_map(|part| part.split(|ch: char| !ch.is_ascii_alphanumeric()).next()).take(2).collect();
    if keys.len() != 2 {
        return None;
    }
    let encrypted = body.split("\"url\": \"").nth(1)?.split('"').next()?;
    decrypt(encrypted, keys[0], keys[1])
}

fn decrypt(src: &str, key1: &str, key2: &str) -> Option<String> {
    let mut prefix = vec!['\0'; key2.chars().count()];
    for (pos, ch) in key1.chars().zip(key2.chars()) {
        let index = pos.to_digit(10)? as usize;
        if index < prefix.len() {
            prefix[index] = ch;
        }
    }
    let text = format!("{}YLwJVbXw77pk2eOrAnFdBo2c3mWkLtodMni2wk81GCnP94ZltW", prefix.into_iter().collect::<String>());
    let digest = Md5::digest(text.as_bytes());
    let hex = format!("{digest:x}");
    let iv = &hex.as_bytes()[0..16];
    let key = &hex.as_bytes()[16..32];
    let bytes = general_purpose::STANDARD.decode(src).ok()?;
    let decrypted = Aes128CbcDec::new_from_slices(key, iv).ok()?.decrypt_padded_vec_mut::<Pkcs7>(&bytes).ok()?;
    String::from_utf8(decrypted).ok()
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                out.push(hex);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Default, Deserialize)]
struct VodResponse {
    list: Vec<VodInfo>,
}

#[derive(Default, Deserialize)]
struct VodInfo {
    #[serde(rename = "vod_id")]
    id: u64,
    #[serde(rename = "vod_name")]
    name: String,
    #[serde(rename = "vod_pic")]
    pic: String,
    #[serde(rename = "vod_actor")]
    actor: String,
}

const LIST_FIXTURE: &str = r#"<div class="public-list-box"><img data-src="/s.jpg"><div class="public-list-button"><a href="/bangumi/1.html">Sample</a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="slide-info-title">Sample</h1><div class="anthology-list-play"><a href="/play/1-1-1.html">第1集</a></div>"#;
const PLAYER_FIXTURE: &str = r#"<script>var player_aaaa={"url":"aHR0cHMlM0ElMkYlMkZtZWRpYS5leGFtcGxlJTJGc2FtcGxlLm0zdTg="}</script>"#;
const PARSER_FIXTURE: &str = r#"var now_01; var now_10; {"url": ""}"#;

export_video_source!(SOURCE);
