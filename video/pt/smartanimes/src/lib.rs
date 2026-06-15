use manatan_extension::{
    CatalogItem, HomeSection, Paged, UrlResolveResult, VideoEpisode, VideoStream,
    abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::http::HttpClient,
    video::animestream::{AnimeStreamConfig, AnimeStreamSource},
};
use scraper::Html;
use serde::Deserialize;
use serde_json::Value;

#[path = "../../_shared/pt_video_common.rs"]
mod pt_video_common;

use pt_video_common::{
    PtVideoConfig, absolute_remote, attr, external_stream, preference, selector, sort_streams,
    stream_for_url,
};

const SOURCE: SmartAnimes = SmartAnimes;
const BASE_URL: &str = "https://smartanimes.net";

struct SmartAnimes;
struct SmartAnimesConfig;

impl AnimeStreamConfig for SmartAnimesConfig {
    const NAME: &'static str = "SmartAnimes";
    const BASE_URL: &'static str = BASE_URL;
    const LANG: &'static str = "pt-BR";
    const CONTENT_RATING: &'static str = "adult";
    const QUALITY_DEFAULT: &'static str = "1080p";
}

impl PtVideoConfig for SmartAnimesConfig {
    const NAME: &'static str = "SmartAnimes";
    const BASE_URL: &'static str = BASE_URL;
    const LANG: &'static str = "pt-BR";
    const CONTENT_RATING: &'static str = "adult";
    const LIST_SELECTOR: &'static str = "article";
    const EPISODE_SELECTOR: &'static str = "a";

    fn popular_url(page: u64) -> String {
        format!("{BASE_URL}/anime/?page={page}&order=popular")
    }

    fn latest_url(page: u64) -> String {
        format!("{BASE_URL}/anime/?page={page}&order=update")
    }

    fn search_url(page: u64, query: &str, _request: &Value) -> String {
        format!("{BASE_URL}/page/{page}/?s={}", manatan_shared::url::query_escape(query))
    }
}

impl VideoSource for SmartAnimes {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        AnimeStreamSource::<SmartAnimesConfig>::new().list(request)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        AnimeStreamSource::<SmartAnimesConfig>::new().search(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        AnimeStreamSource::<SmartAnimesConfig>::new().details(request)
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        AnimeStreamSource::<SmartAnimesConfig>::new().episodes(request)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "episode").unwrap_or_else(|| "/sample".to_string());
        let referer = absolute_url(&episode);
        let body = fetch(&referer, PLAYER_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for item in doc.select(&selector(".dlbox li:not(.head)")) {
            let name = item
                .select(&selector(".q"))
                .next()
                .map(pt_video_common::text)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Player".to_string());
            let quality = item
                .select(&selector(".w"))
                .next()
                .map(pt_video_common::text)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| preference(&request, "preferred_quality", "1080p"));
            let Some(link) = item.select(&selector("a[href]")).next().map(|a| attr(&a, "href")) else {
                continue;
            };
            let player = absolute_remote(&link, BASE_URL);
            if player.contains(host(BASE_URL)) {
                streams.extend(resolve_smart_player(&player, &format!("{name} - {quality}"), &request));
            }
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        AnimeStreamSource::<SmartAnimesConfig>::new().home(request)
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        AnimeStreamSource::<SmartAnimesConfig>::new().item_url(request)
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        AnimeStreamSource::<SmartAnimesConfig>::new().episode_url(request)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        AnimeStreamSource::<SmartAnimesConfig>::new().handle_url(request)
    }
}

fn resolve_smart_player(url: &str, name: &str, request: &Value) -> Vec<VideoStream> {
    let content = fetch(url, "", BASE_URL);
    let Some(item) = json_after::<ItemDto>(&content, "var item = ") else {
        return Vec::new();
    };
    let Some(options) = json_after::<OptionsDto>(&content, "var options = ") else {
        return Vec::new();
    };
    let response = client(&item.post)
        .post(options.soralink_ajaxurl)
        .referer(&item.post)
        .form(&[
            ("token", item.token.as_str()),
            ("id", item.id.to_string().as_str()),
            ("time", item.time.to_string().as_str()),
            ("post", item.post.as_str()),
            ("redirect", item.redirect.as_str()),
            ("cacha", item.cacha.as_str()),
            ("new", "false"),
            ("link", item.link.as_str()),
            ("action", options.soralink_z.as_str()),
        ])
        .send()
        .ok();
    let source_url = response
        .as_ref()
        .and_then(|response| header_value(&response.headers, "location"))
        .or_else(|| response.as_ref().map(|response| response.final_url.clone()))
        .unwrap_or_default();
    if source_url.is_empty() {
        return Vec::new();
    }
    if source_url.contains("send.now") {
        return resolve_send_now(&source_url, name, request);
    }
    if source_url.contains(".m3u8") || source_url.contains(".mp4") {
        return vec![stream_for_url::<SmartAnimesConfig>(&source_url, name, url, request)];
    }
    vec![external_stream(&source_url, name, url)]
}

fn resolve_send_now(url: &str, name: &str, request: &Value) -> Vec<VideoStream> {
    let body = fetch(url, "", BASE_URL);
    let doc = Html::parse_document(&body);
    if let Some(source) = doc.select(&selector("source[src], video[src]")).next() {
        let src = attr(&source, "src");
        if !src.is_empty() {
            return vec![stream_for_url::<SmartAnimesConfig>(&absolute_remote(&src, url), name, url, request)];
        }
    }
    vec![external_stream(url, name, url)]
}

fn client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8")
        .with_header("Accept-Language", "pt-BR,pt;q=0.9,en-US;q=0.8,en;q=0.7")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .or_else(|| value.get("url"))
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(path_key)
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(BASE_URL) {
        return input.to_string();
    }
    let without_base = input.strip_prefix(BASE_URL).unwrap_or(input);
    format!("/{}", without_base.split('#').next().unwrap_or(without_base).trim_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        manatan_shared::url::join_url(BASE_URL, input)
    }
}

fn json_after<T: for<'de> Deserialize<'de>>(body: &str, marker: &str) -> Option<T> {
    let raw = body.split(marker).nth(1)?.split(';').next()?.trim();
    serde_json::from_str(raw).ok()
}

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.to_string())
}

fn host(input: &str) -> &str {
    input.split("://").nth(1).unwrap_or(input).split('/').next().unwrap_or(input)
}

#[derive(Deserialize)]
struct ItemDto {
    token: String,
    id: i64,
    time: i64,
    post: String,
    redirect: String,
    cacha: String,
    link: String,
}

#[derive(Deserialize)]
struct OptionsDto {
    soralink_z: String,
    soralink_ajaxurl: String,
}

const PLAYER_FIXTURE: &str =
    r#"<ul class="dlbox"><li><span class="q">Server</span><span class="w">720p</span><a href="/sample"></a></li></ul>"#;

export_video_source!(SOURCE);
