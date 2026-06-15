use manatan_extension::{
    CatalogItem, Paged, VideoStream, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use scraper::{ElementRef, Html};
use serde_json::Value;

#[path = "../../_shared/pt_video_common.rs"]
mod pt_video_common;

use pt_video_common::{
    PtVideoConfig, PtVideoSource, absolute_remote, attr, fetch, image_from, request_key,
    resolve_embed, selector, sort_streams,
};

const SOURCE: PiFansubs = PiFansubs;
const BASE_URL: &str = "https://pifansubs.club";

struct PiFansubs;
struct PiFansubsConfig;

impl PtVideoConfig for PiFansubsConfig {
    const NAME: &'static str = "Pi Fansubs";
    const BASE_URL: &'static str = BASE_URL;
    const CONTENT_RATING: &'static str = "adult";
    const POPULAR_TITLE: &'static str = "Popular";
    const LATEST_TITLE: &'static str = "Episodios";
    const LIST_SELECTOR: &'static str = "div#featured-titles div.poster, article, div.poster";
    const LATEST_SELECTOR: &'static str = "article, div.items article, div.episodes article";
    const SEARCH_SELECTOR: &'static str = "article, div.items article, div.poster";
    const DETAILS_TITLE_SELECTOR: &'static str = "div.data h1, h1";
    const DETAILS_COVER_SELECTOR: &'static str = "div.poster img, img";
    const DETAILS_DESCRIPTION_SELECTOR: &'static str = "div#info p, div.wp-content p, p";
    const DETAILS_TAG_SELECTOR: &'static str = "div.sgeneros a, a[rel='tag']";
    const EPISODE_SELECTOR: &'static str = "ul.episodios li a, div#seasons li a, a[href*='/episodio']";

    fn popular_url(_page: u64) -> String {
        BASE_URL.to_string()
    }

    fn latest_url(page: u64) -> String {
        format!("{BASE_URL}/episodios/page/{page}")
    }

    fn search_url(page: u64, query: &str, _request: &Value) -> String {
        format!("{BASE_URL}/page/{page}/?s={}", manatan_shared::url::query_escape(query))
    }

    fn card_cover(el: ElementRef<'_>) -> Option<String> {
        image_from(el).map(|src| pt_video_common::absolute_url::<Self>(&src))
    }
}

impl VideoSource for PiFansubs {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        PtVideoSource::<PiFansubsConfig>::new().list(request)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        PtVideoSource::<PiFansubsConfig>::new().search(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        PtVideoSource::<PiFansubsConfig>::new().details(request)
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<manatan_extension::VideoEpisode>> {
        PtVideoSource::<PiFansubsConfig>::new().episodes(request)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key::<PiFansubsConfig>(&request, "episode")
            .unwrap_or_else(|| "/sample".to_string());
        let referer = pt_video_common::absolute_url::<PiFansubsConfig>(&episode);
        let body = fetch::<PiFansubsConfig>(&referer, PLAYER_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for iframe in doc.select(&selector("div.source-box:not(#source-player-trailer) iframe, iframe[src], iframe[data-src]")) {
            let raw = attr(&iframe, "data-src").if_empty(&attr(&iframe, "src"));
            let player = absolute_remote(&raw, &referer);
            streams.extend(resolve_embed::<PiFansubsConfig>(&player, "Player", &referer, &request, 0));
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<manatan_extension::HomeSection<CatalogItem>>> {
        PtVideoSource::<PiFansubsConfig>::new().home(request)
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        PtVideoSource::<PiFansubsConfig>::new().item_url(request)
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        PtVideoSource::<PiFansubsConfig>::new().episode_url(request)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<manatan_extension::UrlResolveResult>> {
        PtVideoSource::<PiFansubsConfig>::new().handle_url(request)
    }
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.trim().is_empty() { fallback.to_string() } else { self }
    }
}

const PLAYER_FIXTURE: &str = r#"<div class="source-box"><iframe src="https://example.invalid/embed"></iframe></div>"#;

export_video_source!(SOURCE);
