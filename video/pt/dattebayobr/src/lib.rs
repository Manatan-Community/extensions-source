use manatan_extension::{
    CatalogItem, Paged, VideoEpisode, VideoStream, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use regex::Regex;
use scraper::Html;
use serde_json::Value;

#[path = "../../_shared/pt_video_common.rs"]
mod pt_video_common;

use pt_video_common::{
    PtVideoConfig, PtVideoSource, attr, client, fetch, request_key, selector, sort_streams,
    stream_for_url, text,
};

const SOURCE: DattebayoBr = DattebayoBr;
const BASE_URL: &str = "https://www.dattebayo-br.com";

struct DattebayoBr;
struct DattebayoBrConfig;

impl PtVideoConfig for DattebayoBrConfig {
    const NAME: &'static str = "Dattebayo BR";
    const BASE_URL: &'static str = BASE_URL;
    const POPULAR_TITLE: &'static str = "Animes";
    const LATEST_TITLE: &'static str = "Lancamentos";
    const LIST_SELECTOR: &'static str = "div.ultimosAnimesHomeItem";
    const LATEST_SELECTOR: &'static str = Self::LIST_SELECTOR;
    const SEARCH_SELECTOR: &'static str = Self::LIST_SELECTOR;
    const DETAILS_TITLE_SELECTOR: &'static str = ".tituloPage h1, h1";
    const DETAILS_COVER_SELECTOR: &'static str = ".aniInfosSingleCapa img, img";
    const DETAILS_DESCRIPTION_SELECTOR: &'static str = ".aniInfosSingleSinopse p, p";
    const DETAILS_TAG_SELECTOR: &'static str = ".aniInfosSingleGeneros span, a[rel='tag']";
    const EPISODE_SELECTOR: &'static str = "div.ultimosEpisodiosHomeItem";
    const EPISODE_TITLE_SELECTOR: &'static str = ".ultimosEpisodiosHomeItemInfosNome";
    const EPISODE_NUMBER_SELECTOR: &'static str = ".ultimosEpisodiosHomeItemInfosNum";

    fn popular_url(_page: u64) -> String {
        BASE_URL.to_string()
    }

    fn latest_url(_page: u64) -> String {
        BASE_URL.to_string()
    }

    fn search_url(page: u64, query: &str, _request: &Value) -> String {
        format!("{BASE_URL}/busca?busca={}&page={page}", manatan_shared::url::query_escape(query))
    }
}

impl VideoSource for DattebayoBr {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        PtVideoSource::<DattebayoBrConfig>::new().list(request)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        PtVideoSource::<DattebayoBrConfig>::new().search(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        PtVideoSource::<DattebayoBrConfig>::new().details(request)
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key::<DattebayoBrConfig>(&request, "item").unwrap_or_else(|| "/sample".to_string());
        let base = pt_video_common::absolute_url::<DattebayoBrConfig>(&path);
        let mut episodes = Vec::new();
        for page in 1..=20 {
            let url = if page == 1 { base.clone() } else { format!("{base}/page/{page}") };
            let body = fetch::<DattebayoBrConfig>(&url, DETAILS_FIXTURE, BASE_URL);
            let doc = Html::parse_document(&body);
            let before = episodes.len();
            for el in doc.select(&selector("div.ultimosEpisodiosHomeItem")) {
                if let Some(ep) = pt_video_common::default_episode_from_element::<DattebayoBrConfig>(el) {
                    if !episodes.iter().any(|existing: &VideoEpisode| existing.key == ep.key) {
                        episodes.push(ep);
                    }
                }
            }
            if episodes.len() == before {
                break;
            }
        }
        episodes.sort_by(|a, b| b.episode_number.partial_cmp(&a.episode_number).unwrap_or(std::cmp::Ordering::Equal));
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key::<DattebayoBrConfig>(&request, "episode")
            .unwrap_or_else(|| "/sample-1".to_string());
        let referer = pt_video_common::absolute_url::<DattebayoBrConfig>(&episode);
        let body = fetch::<DattebayoBrConfig>(&referer, PLAYER_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let mut streams = Vec::new();
        for tab in doc.select(&selector("div.AbasBox div.Aba")) {
            let aba_type = attr(&tab, "aba-type");
            let quality = text(tab);
            let Some(container) = doc.select(&selector(&format!("#{aba_type}"))).next() else {
                continue;
            };
            let script = pt_video_common::select_text(container, "script").unwrap_or_else(|| container.html());
            let Some(caps) = Regex::new(r#"var\s+vid\s*=\s*['"]([^'"]+)['"]"#).ok().and_then(|re| re.captures(&script)) else {
                continue;
            };
            let base_video = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            if base_video.is_empty() {
                continue;
            }
            let final_url = sign_video_url(base_video, &referer).unwrap_or_else(|| base_video.to_string());
            streams.push(stream_for_url::<DattebayoBrConfig>(&final_url, &quality, &referer, &request));
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<manatan_extension::HomeSection<CatalogItem>>> {
        PtVideoSource::<DattebayoBrConfig>::new().home(request)
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        PtVideoSource::<DattebayoBrConfig>::new().item_url(request)
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        PtVideoSource::<DattebayoBrConfig>::new().episode_url(request)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<manatan_extension::UrlResolveResult>> {
        PtVideoSource::<DattebayoBrConfig>::new().handle_url(request)
    }
}

fn sign_video_url(base_video: &str, referer: &str) -> Option<String> {
    let target = format!("https://ads.animeyabu.net?url={}", manatan_shared::url::query_escape(base_video));
    let body = client::<DattebayoBrConfig>(referer)
        .get(target)
        .referer(referer)
        .send_text()
        .ok()?;
    let value: Value = serde_json::from_str(&body).ok()?;
    let signature = value
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item.get("publicidade"))
        .and_then(Value::as_str)?;
    Some(format!("{base_video}{signature}"))
}

const DETAILS_FIXTURE: &str = r#"<div class="ultimosEpisodiosHomeItem"><a href="/anime/sample-1"><div class="ultimosEpisodiosHomeItemInfosNum">Episodio 1</div><div class="ultimosEpisodiosHomeItemInfosNome">Episode 1</div></a></div>"#;
const PLAYER_FIXTURE: &str = r#"<div class="AbasBox"><div class="Aba" aba-type="p1">720p</div></div><div id="p1"><script>var vid = 'https://example.invalid/video.mp4'</script></div>"#;

export_video_source!(SOURCE);
