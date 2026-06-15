use manatan_extension::{
    CatalogItem, Paged, VideoEpisode, VideoStream, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use scraper::Html;
use serde_json::Value;

#[path = "../../_shared/pt_video_common.rs"]
mod pt_video_common;

use pt_video_common::{
    PtVideoConfig, PtVideoSource, attr, fetch, first_number, request_key, selector, sort_streams,
    stream_for_url, text,
};

const SOURCE: DarkMahou = DarkMahou;
const BASE_URL: &str = "https://darkmahou.org";

struct DarkMahou;
struct DarkMahouConfig;

impl PtVideoConfig for DarkMahouConfig {
    const NAME: &'static str = "DarkMahou (Torrent)";
    const BASE_URL: &'static str = BASE_URL;
    const POPULAR_TITLE: &'static str = "Animes";
    const LATEST_TITLE: &'static str = "Recentes";
    const LIST_SELECTOR: &'static str = "div.listupd article a, div.bsx a, div.bs a, article a";
    const LATEST_SELECTOR: &'static str = Self::LIST_SELECTOR;
    const SEARCH_SELECTOR: &'static str = Self::LIST_SELECTOR;
    const DETAILS_TITLE_SELECTOR: &'static str = "h1.entry-title, h1";
    const DETAILS_COVER_SELECTOR: &'static str = "div.thumb img, div.poster img, img";
    const DETAILS_DESCRIPTION_SELECTOR: &'static str = "div.entry-content, div.desc, p";
    const DETAILS_TAG_SELECTOR: &'static str = "a[rel='tag'], .genres a";
    const EPISODE_SELECTOR: &'static str = "div.mctnx div.soraddl";

    fn popular_url(page: u64) -> String {
        format!("{BASE_URL}/animes/page/{page}")
    }

    fn latest_url(page: u64) -> String {
        format!("{BASE_URL}/page/{page}")
    }

    fn search_url(page: u64, query: &str, _request: &Value) -> String {
        if query.is_empty() {
            format!("{BASE_URL}/animes/page/{page}")
        } else {
            format!("{BASE_URL}/page/{page}/?s={}", manatan_shared::url::query_escape(query))
        }
    }

}

impl VideoSource for DarkMahou {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        PtVideoSource::<DarkMahouConfig>::new().list(request)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        PtVideoSource::<DarkMahouConfig>::new().search(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        PtVideoSource::<DarkMahouConfig>::new().details(request)
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key::<DarkMahouConfig>(&request, "item").unwrap_or_else(|| "/sample".to_string());
        let url = pt_video_common::absolute_url::<DarkMahouConfig>(&path);
        let body = fetch::<DarkMahouConfig>(&url, DETAILS_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        Ok(doc
            .select(&selector("div.mctnx div.soraddl"))
            .filter_map(|el| {
                let title = pt_video_common::select_text(el, ".sorattl h3, h3").unwrap_or_else(|| text(el));
                if title.is_empty() {
                    return None;
                }
                let key = format!("{path}#{}", title.replace(' ', "%20"));
                Some(VideoEpisode {
                    key: key.clone(),
                    title: Some(title.clone()),
                    episode_number: first_number(&title),
                    url: Some(format!("{url}#{}", title.replace(' ', "%20"))),
                    language: Some("pt-BR".to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect())
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let raw = request
            .get("episode")
            .and_then(|v| v.get("key").or_else(|| v.get("url")).and_then(Value::as_str).or_else(|| v.as_str()))
            .or_else(|| request.get("key").and_then(Value::as_str))
            .unwrap_or("/sample#Episode%201");
        let (path, fragment) = raw.split_once('#').unwrap_or((raw, ""));
        let page_url = pt_video_common::absolute_url::<DarkMahouConfig>(path);
        let wanted = fragment.replace("%20", " ");
        let body = fetch::<DarkMahouConfig>(&page_url, DETAILS_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for block in doc.select(&selector("div.mctnx div.soraddl")) {
            let title = pt_video_common::select_text(block, ".sorattl h3, h3").unwrap_or_default();
            if !wanted.is_empty() && title != wanted {
                continue;
            }
            for group in block.select(&selector(".soraurl")) {
                let prefix = if text(group).to_lowercase().contains("dublado") {
                    "Dublado"
                } else {
                    "Legendado"
                };
                for link in group.select(&selector(".slink a[href], a[href]")) {
                    let href = attr(&link, "href");
                    let quality = text(link);
                    streams.push(stream_for_url::<DarkMahouConfig>(&href, &format!("{prefix} - {quality}"), &page_url, &request));
                }
            }
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<manatan_extension::HomeSection<CatalogItem>>> {
        PtVideoSource::<DarkMahouConfig>::new().home(request)
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        PtVideoSource::<DarkMahouConfig>::new().item_url(request)
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        PtVideoSource::<DarkMahouConfig>::new().episode_url(request)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<manatan_extension::UrlResolveResult>> {
        PtVideoSource::<DarkMahouConfig>::new().handle_url(request)
    }
}

const DETAILS_FIXTURE: &str = r#"<div class="mctnx"><div class="soraddl"><div class="sorattl"><h3>Episode 1</h3></div><div class="soraurl"><div class="slink"><a href="magnet:?xt=urn:btih:sample">1080p</a></div></div></div></div>"#;

export_video_source!(SOURCE);
