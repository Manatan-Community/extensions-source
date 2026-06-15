use manatan_extension::{
    CatalogItem, Paged, VideoStream, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use scraper::Html;
use serde_json::Value;

#[path = "../../_shared/pt_video_common.rs"]
mod pt_video_common;

use pt_video_common::{
    PtVideoConfig, PtVideoSource, absolute_remote, attr, fetch, filter, request_key, resolve_embed,
    selector, sort_streams, stream_for_url,
};

const SOURCE: Doramogo = Doramogo;
const BASE_URL: &str = "https://doramogo.com";

struct Doramogo;
struct DoramogoConfig;

impl PtVideoConfig for DoramogoConfig {
    const NAME: &'static str = "Doramogo";
    const BASE_URL: &'static str = BASE_URL;
    const POPULAR_TITLE: &'static str = "Populares";
    const LATEST_TITLE: &'static str = "Recentes";
    const LIST_SELECTOR: &'static str = "div.item-drm";
    const LATEST_SELECTOR: &'static str = Self::LIST_SELECTOR;
    const SEARCH_SELECTOR: &'static str = Self::LIST_SELECTOR;
    const DETAILS_TITLE_SELECTOR: &'static str = "div.dados h1, h1";
    const DETAILS_COVER_SELECTOR: &'static str = "div.image--cover, img";
    const DETAILS_DESCRIPTION_SELECTOR: &'static str = "p.readMor, p";
    const DETAILS_TAG_SELECTOR: &'static str = "a[rel='tag'], .genres a";
    const EPISODE_SELECTOR: &'static str = "li.episode--content";
    const EPISODE_TITLE_SELECTOR: &'static str = "div.title-episode a, a";

    fn popular_url(_page: u64) -> String {
        format!("{BASE_URL}/doramas/?filter_order=popular")
    }

    fn latest_url(_page: u64) -> String {
        format!("{BASE_URL}/doramas/?filter_orderby=date")
    }

    fn search_url(_page: u64, query: &str, request: &Value) -> String {
        let mut target = format!("{BASE_URL}/search/{}", manatan_shared::url::query_escape(query));
        let mut params = Vec::new();
        if let Some(audio) = filter(request, "audio").filter(|v| !v.is_empty()) {
            params.push(format!("filter_audio={}", manatan_shared::url::query_escape(&audio)));
        }
        if let Some(genre) = filter(request, "genre").filter(|v| !v.is_empty()) {
            params.push(format!("filter_genre={}", manatan_shared::url::query_escape(&genre)));
        }
        if !params.is_empty() {
            target.push('?');
            target.push_str(&params.join("&"));
        }
        target
    }
}

impl VideoSource for Doramogo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        PtVideoSource::<DoramogoConfig>::new().list(request)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        PtVideoSource::<DoramogoConfig>::new().search(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        PtVideoSource::<DoramogoConfig>::new().details(request)
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<manatan_extension::VideoEpisode>> {
        let mut episodes = PtVideoSource::<DoramogoConfig>::new().episodes(request)?;
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key::<DoramogoConfig>(&request, "episode").unwrap_or_else(|| "/sample".to_string());
        let referer = pt_video_common::absolute_url::<DoramogoConfig>(&episode);
        let body = fetch::<DoramogoConfig>(&referer, PLAYER_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for iframe in doc.select(&selector("div.source-box iframe[src], iframe[src]")) {
            streams.extend(resolve_doramogo_url(&absolute_remote(&attr(&iframe, "src"), &referer), &referer, &request));
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<manatan_extension::HomeSection<CatalogItem>>> {
        PtVideoSource::<DoramogoConfig>::new().home(request)
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        PtVideoSource::<DoramogoConfig>::new().item_url(request)
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        PtVideoSource::<DoramogoConfig>::new().episode_url(request)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<manatan_extension::UrlResolveResult>> {
        PtVideoSource::<DoramogoConfig>::new().handle_url(request)
    }
}

fn resolve_doramogo_url(url: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    if url.contains("dailymotion") || url.contains("ok.ru") || url.contains("drive.google.com") {
        return vec![pt_video_common::external_stream(url, "External", referer)];
    }
    if url.contains("embedrise.com") || url.contains("streamable.com") {
        let body = fetch::<DoramogoConfig>(url, "", referer);
        let doc = Html::parse_document(&body);
        if let Some(src) = doc
            .select(&selector("video source[src], video[src], source[src]"))
            .next()
            .map(|el| attr(&el, "src"))
        {
            return vec![stream_for_url::<DoramogoConfig>(&absolute_remote(&src, url), "External", url, request)];
        }
    }
    if url.contains("/player/") {
        let body = fetch::<DoramogoConfig>(url, "", referer);
        let mut streams = pt_video_common::streams_from_script::<DoramogoConfig>(&body, url, "Doramogo", request);
        if !streams.is_empty() {
            return streams.drain(..).collect();
        }
    }
    resolve_embed::<DoramogoConfig>(url, "External", referer, request, 0)
}

const PLAYER_FIXTURE: &str = r#"<div class="source-box"><iframe src="https://example.invalid/embed"></iframe></div>"#;

export_video_source!(SOURCE);
