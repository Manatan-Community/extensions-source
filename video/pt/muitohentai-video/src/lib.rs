use manatan_extension::{
    CatalogItem, Paged, VideoEpisode, VideoStream, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use scraper::{ElementRef, Html};
use serde_json::Value;

#[path = "../../_shared/pt_video_common.rs"]
mod pt_video_common;

use pt_video_common::{
    PtVideoConfig, PtVideoSource, absolute_remote, attr, fetch, first_number, image_from,
    request_key, selector, sort_streams, stream_for_url, text,
};

const SOURCE: MuitoHentai = MuitoHentai;
const BASE_URL: &str = "https://www.muitohentai.com";

struct MuitoHentai;
struct MuitoHentaiConfig;

impl PtVideoConfig for MuitoHentaiConfig {
    const NAME: &'static str = "Muito Hentai";
    const BASE_URL: &'static str = BASE_URL;
    const CONTENT_RATING: &'static str = "adult";
    const POPULAR_TITLE: &'static str = "Ranking";
    const LATEST_TITLE: &'static str = "Lancamentos";
    const LIST_SELECTOR: &'static str = "ul.ul_sidebar > li";
    const LATEST_SELECTOR: &'static str = "div.animation-2 > article";
    const SEARCH_SELECTOR: &'static str = "div#archive-content > article > div.poster";
    const DETAILS_TITLE_SELECTOR: &'static str = "div.sheader div.data h1, h1";
    const DETAILS_COVER_SELECTOR: &'static str = "div.sheader div.poster img, img";
    const DETAILS_DESCRIPTION_SELECTOR: &'static str = "div#info1 div.wp-content p, div.wp-content p, p";
    const DETAILS_TAG_SELECTOR: &'static str = "div.sgeneros a, div.sgeneros span";
    const EPISODE_SELECTOR: &'static str = "article.item";
    const EPISODE_TITLE_SELECTOR: &'static str = "div.data h3";

    fn popular_url(page: u64) -> String {
        format!("{BASE_URL}/ranking-hentais/?paginacao={page}")
    }

    fn latest_url(_page: u64) -> String {
        BASE_URL.to_string()
    }

    fn search_url(_page: u64, query: &str, _request: &Value) -> String {
        format!("{BASE_URL}/buscar/{}", manatan_shared::url::query_escape(query))
    }

    fn normalize_item_path(href: &str) -> String {
        if href.contains("/episodios/") {
            let slug = href
                .split("/episodios/")
                .nth(1)
                .unwrap_or_default()
                .split("-episodio")
                .next()
                .unwrap_or_default();
            if !slug.is_empty() {
                return format!("/info/{slug}");
            }
        }
        pt_video_common::path_key::<Self>(href)
    }

    fn card_title(el: ElementRef<'_>, path: &str) -> String {
        el.select(&selector("a.series"))
            .next()
            .map(text)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                el.select(&selector("img"))
                    .next()
                    .map(|img| attr(&img, "alt"))
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| pt_video_common::title_from_path::<Self>(path))
    }

    fn card_cover(el: ElementRef<'_>) -> Option<String> {
        image_from(el).map(|src| pt_video_common::absolute_url::<Self>(&src))
    }

    fn episode_from_element(el: ElementRef<'_>) -> Option<VideoEpisode> {
        let href = el
            .select(&selector("div.poster div.season_m a[href], a[href]"))
            .next()
            .map(|a| attr(&a, "href"))?;
        let title = el
            .select(&selector("div.data h3"))
            .next()
            .map(text)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Episode 1".to_string());
        let number = first_number(&title).unwrap_or(1.0);
        let key = pt_video_common::path_key::<Self>(&href);
        Some(VideoEpisode {
            key: key.clone(),
            title: Some(title),
            episode_number: Some(number),
            url: Some(pt_video_common::absolute_url::<Self>(&key)),
            language: Some(Self::LANG.to_string()),
            ..VideoEpisode::default()
        })
    }
}

impl VideoSource for MuitoHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        PtVideoSource::<MuitoHentaiConfig>::new().list(request)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        PtVideoSource::<MuitoHentaiConfig>::new().search(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        PtVideoSource::<MuitoHentaiConfig>::new().details(request)
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let mut episodes = PtVideoSource::<MuitoHentaiConfig>::new().episodes(request)?;
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key::<MuitoHentaiConfig>(&request, "episode")
            .unwrap_or_else(|| "/episodios/sample".to_string());
        let referer = pt_video_common::absolute_url::<MuitoHentaiConfig>(&episode);
        let body = fetch::<MuitoHentaiConfig>(&referer, PLAYER_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for iframe in doc.select(&selector("div.playex div#option-0 iframe[src], iframe[src]")) {
            let src = attr(&iframe, "src");
            let idplay = src.split("?idplay=").nth(1).unwrap_or_default();
            if idplay.is_empty() {
                continue;
            }
            let player = format!("https://www.hentaitube.online/players_sites/mt/index.php?idplay={idplay}");
            let player_body = fetch::<MuitoHentaiConfig>(&player, "", &referer);
            let player_doc = Html::parse_document(&player_body);
            for source in player_doc.select(&selector("source[src]")) {
                let url = absolute_remote(&attr(&source, "src"), &player);
                let label = attr(&source, "label");
                streams.push(stream_for_url::<MuitoHentaiConfig>(&url, &label, &player, &request));
            }
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<manatan_extension::HomeSection<CatalogItem>>> {
        PtVideoSource::<MuitoHentaiConfig>::new().home(request)
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        PtVideoSource::<MuitoHentaiConfig>::new().item_url(request)
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        PtVideoSource::<MuitoHentaiConfig>::new().episode_url(request)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<manatan_extension::UrlResolveResult>> {
        PtVideoSource::<MuitoHentaiConfig>::new().handle_url(request)
    }
}

const PLAYER_FIXTURE: &str =
    r#"<div class="playex"><div id="option-0"><iframe src="/player?idplay=1"></iframe></div></div>"#;

export_video_source!(SOURCE);
