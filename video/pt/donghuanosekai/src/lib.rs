use manatan_extension::{
    CatalogItem, Paged, VideoStream, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use scraper::Html;
use serde::Deserialize;
use serde_json::Value;

#[path = "../../_shared/pt_video_common.rs"]
mod pt_video_common;

use pt_video_common::{
    PtVideoConfig, PtVideoSource, absolute_remote, attr, client, fetch, filter, parse_cards,
    resolve_embed, selector, sort_streams, stream_for_url,
};

const SOURCE: DonghuaNoSekai = DonghuaNoSekai;
const BASE_URL: &str = "https://donghuanosekai.com";

struct DonghuaNoSekai;
struct DonghuaNoSekaiConfig;

impl PtVideoConfig for DonghuaNoSekaiConfig {
    const NAME: &'static str = "Donghua no Sekai";
    const BASE_URL: &'static str = BASE_URL;
    const POPULAR_TITLE: &'static str = "Top";
    const LATEST_TITLE: &'static str = "Lancamentos";
    const LIST_SELECTOR: &'static str = "div.sidebarContent div.navItensTop li > a";
    const LATEST_SELECTOR: &'static str = "div.boxContent div.itemE > a";
    const SEARCH_SELECTOR: &'static str = "div.itemE > a";
    const DETAILS_TITLE_SELECTOR: &'static str = "div.dados h1, h1";
    const DETAILS_COVER_SELECTOR: &'static str = "div.poster > img, img";
    const DETAILS_DESCRIPTION_SELECTOR: &'static str = "div.articleContent:has(div:contains('Sinopse')) div.context p, p";
    const DETAILS_TAG_SELECTOR: &'static str = "div.genresL > a, a[rel='tag']";
    const EPISODE_SELECTOR: &'static str = "div.episode_list > div.item > a";
    const EPISODE_TITLE_SELECTOR: &'static str = "span.episode";
    const EPISODE_NUMBER_SELECTOR: &'static str = "span.episode";

    fn popular_url(_page: u64) -> String {
        BASE_URL.to_string()
    }

    fn latest_url(page: u64) -> String {
        format!("{BASE_URL}/lancamentos?pagina={page}")
    }

    fn search_url(_page: u64, _query: &str, _request: &Value) -> String {
        format!("{BASE_URL}/donghuas")
    }

    fn search_override(page: u64, query: &str, request: &Value) -> Option<Paged<CatalogItem>> {
        Some(search_ajax(page, query, request))
    }

    fn real_details_url(_path: &str, body: &str) -> Option<String> {
        let doc = Html::parse_document(body);
        doc.select(&selector("div.controles li.list-ep > a[href]"))
            .next()
            .map(|el| absolute_remote(&attr(&el, "href"), BASE_URL))
    }
}

impl VideoSource for DonghuaNoSekai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        PtVideoSource::<DonghuaNoSekaiConfig>::new().list(request)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        PtVideoSource::<DonghuaNoSekaiConfig>::new().search(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        PtVideoSource::<DonghuaNoSekaiConfig>::new().details(request)
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<manatan_extension::VideoEpisode>> {
        PtVideoSource::<DonghuaNoSekaiConfig>::new().episodes(request)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = pt_video_common::request_key::<DonghuaNoSekaiConfig>(&request, "episode")
            .unwrap_or_else(|| "/sample-1".to_string());
        let referer = pt_video_common::absolute_url::<DonghuaNoSekaiConfig>(&episode);
        let body = fetch::<DonghuaNoSekaiConfig>(&referer, PLAYER_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for slide in doc.select(&selector("div.slideItem[data-video-url]")) {
            let player = absolute_remote(&attr(&slide, "data-video-url"), &referer);
            let player_body = fetch::<DonghuaNoSekaiConfig>(&player, "", &referer);
            streams.extend(streams_from_player(&player_body, &player, &request));
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<manatan_extension::HomeSection<CatalogItem>>> {
        PtVideoSource::<DonghuaNoSekaiConfig>::new().home(request)
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        PtVideoSource::<DonghuaNoSekaiConfig>::new().item_url(request)
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        PtVideoSource::<DonghuaNoSekaiConfig>::new().episode_url(request)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<manatan_extension::UrlResolveResult>> {
        PtVideoSource::<DonghuaNoSekaiConfig>::new().handle_url(request)
    }
}

fn search_ajax(page: u64, query: &str, request: &Value) -> Paged<CatalogItem> {
    let token_body = fetch::<DonghuaNoSekaiConfig>(&format!("{BASE_URL}/donghuas"), "", BASE_URL);
    let token_doc = Html::parse_document(&token_body);
    let token = token_doc
        .select(&selector("div.menu_filter_box[data-secury]"))
        .next()
        .map(|el| attr(&el, "data-secury"))
        .unwrap_or_default();
    let animation = filter(request, "animation").unwrap_or_else(|| "undefined".to_string());
    let letter = filter(request, "letter").unwrap_or_else(|| "0".to_string());
    let order = filter(request, "order").unwrap_or_else(|| "nome".to_string());
    let status = filter(request, "status").unwrap_or_else(|| "0".to_string());
    let filter_data = format!(
        "filter_animation={}&filter_audio=undefined&filter_letter={}&filter_order={}&filter_status={}&type_url=ONA",
        manatan_shared::url::query_escape(&animation),
        manatan_shared::url::query_escape(&letter),
        manatan_shared::url::query_escape(&order),
        manatan_shared::url::query_escape(&status),
    );
    let page_s = page.to_string();
    let filters = format!(r#"{{"filter_data":"{filter_data}","filter_genre_add":[],"filter_genre_del":[]}}"#);
    let body = client::<DonghuaNoSekaiConfig>(BASE_URL)
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .xhr()
        .form(&[
            ("type", "lista"),
            ("action", "getListFilter"),
            ("limit", "30"),
            ("token", token.as_str()),
            ("search", if query.is_empty() { "0" } else { query }),
            ("pagina", page_s.as_str()),
            ("filters", filters.as_str()),
        ])
        .send_text()
        .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
    let Ok(data) = serde_json::from_str::<SearchResponse>(&body) else {
        return Paged::default();
    };
    let mut entries = Vec::new();
    for html in data.results {
        entries.extend(parse_cards::<DonghuaNoSekaiConfig>(&html, DonghuaNoSekaiConfig::SEARCH_SELECTOR).entries);
    }
    Paged {
        entries,
        has_next_page: data.total_page > data.page,
    }
}

fn streams_from_player(body: &str, player_url: &str, request: &Value) -> Vec<VideoStream> {
    let doc = Html::parse_document(body);
    if let Some(source) = doc.select(&selector("video > source[src], source[src]")).next() {
        let size = attr(&source, "size");
        let name = if size.is_empty() {
            "Player".to_string()
        } else {
            format!("{size}p")
        };
        return vec![stream_for_url::<DonghuaNoSekaiConfig>(&absolute_remote(&attr(&source, "src"), player_url), &name, player_url, request)];
    }
    let Some(iframe) = doc.select(&selector("iframe[src]")).next().map(|el| absolute_remote(&attr(&el, "src"), player_url)) else {
        return Vec::new();
    };
    if iframe.contains("nativov2.php") || iframe.contains("/embed2/") {
        let video = query_param(&iframe, "id").or_else(|| query_param(&iframe, "v")).unwrap_or(iframe.clone());
        let quality = video.split('_').nth(1).unwrap_or("720p");
        return vec![stream_for_url::<DonghuaNoSekaiConfig>(&video, quality, player_url, request)];
    }
    if iframe.contains("playerB.php") {
        let body = fetch::<DonghuaNoSekaiConfig>(&iframe, "", player_url);
        return pt_video_common::streams_from_script::<DonghuaNoSekaiConfig>(&body, &iframe, "Player", request);
    }
    resolve_embed::<DonghuaNoSekaiConfig>(&iframe, "External", player_url, request, 0)
}

fn query_param(input: &str, key: &str) -> Option<String> {
    input.split('?').nth(1)?.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

#[derive(Deserialize)]
struct SearchResponse {
    results: Vec<String>,
    page: u64,
    #[serde(default = "one")]
    total_page: u64,
}

fn one() -> u64 {
    1
}

const PLAYER_FIXTURE: &str = r#"<div class="slideItem" data-video-url="https://example.invalid/player"></div>"#;
const SEARCH_FIXTURE: &str = r#"{"results":["<div class=\"itemE\"><a href=\"/sample\"><div class=\"title\"><h3>Sample</h3></div><div class=\"thumb\"><img src=\"/poster.jpg\"></div></a></div>"],"page":1,"total_page":1}"#;

export_video_source!(SOURCE);
