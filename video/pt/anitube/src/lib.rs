use manatan_extension::{
    CatalogItem, Paged, VideoStream, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use regex::Regex;
use scraper::Html;
use serde_json::Value;

#[path = "../../_shared/pt_video_common.rs"]
mod pt_video_common;

use pt_video_common::{
    PtVideoConfig, PtVideoSource, absolute_remote, attr, client, fetch, filter, resolve_embed,
    selector, sort_streams, stream_for_url,
};

const SOURCE: Anitube = Anitube;
const BASE_URL: &str = "https://www.anitube.vip";

struct Anitube;

struct AnitubeConfig;

impl PtVideoConfig for AnitubeConfig {
    const NAME: &'static str = "Anitube";
    const BASE_URL: &'static str = BASE_URL;
    const POPULAR_TITLE: &'static str = "Animes";
    const LATEST_TITLE: &'static str = "Ultimos episodios";
    const LIST_SELECTOR: &'static str = "div.lista_de_animes div.ani_loop_item_img > a";
    const LATEST_SELECTOR: &'static str = "div.threeItensPerContent > div.epi_loop_item > a";
    const SEARCH_SELECTOR: &'static str = "div.lista_de_animes div.ani_loop_item_img > a";
    const DETAILS_TITLE_SELECTOR: &'static str = "div.anime_container_titulo, h1";
    const DETAILS_COVER_SELECTOR: &'static str = "div.anime_container_content img, img";
    const DETAILS_DESCRIPTION_SELECTOR: &'static str = "div.sinopse_container_content, p";
    const DETAILS_TAG_SELECTOR: &'static str = "div.anime_info:contains('Gêneros') a, div.anime_info:contains('Generos') a";
    const EPISODE_SELECTOR: &'static str = "div.animepag_episodios_item > a";
    const EPISODE_TITLE_SELECTOR: &'static str = "div.animepag_episodios_item_views";
    const EPISODE_NUMBER_SELECTOR: &'static str = "div.animepag_episodios_item_views";

    fn popular_url(page: u64) -> String {
        format!("{BASE_URL}/anime/page/{page}")
    }

    fn latest_url(page: u64) -> String {
        format!("{BASE_URL}/?page={page}")
    }

    fn search_url(page: u64, query: &str, request: &Value) -> String {
        if !query.is_empty() {
            return format!("{BASE_URL}/busca.php?s={}&submit=Buscar", manatan_shared::url::query_escape(query));
        }
        if let Some(season) = filter(request, "season").filter(|v| !v.is_empty()) {
            let year = filter(request, "year").unwrap_or_else(|| "todos".to_string());
            return format!("{BASE_URL}/temporada/{season}/{year}");
        }
        if let Some(genre) = filter(request, "genre").filter(|v| !v.is_empty()) {
            let letter = filter(request, "letter").unwrap_or_else(|| "todos".to_string());
            return format!("{BASE_URL}/genero/{genre}/page/{page}/{}", letter.replace("todos", ""));
        }
        let letter = filter(request, "letter").unwrap_or_else(|| "todos".to_string());
        format!("{BASE_URL}/anime/page/{page}/letra/{letter}")
    }
}

impl VideoSource for Anitube {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        PtVideoSource::<AnitubeConfig>::new().list(request)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        PtVideoSource::<AnitubeConfig>::new().search(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        PtVideoSource::<AnitubeConfig>::new().details(request)
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<manatan_extension::VideoEpisode>> {
        PtVideoSource::<AnitubeConfig>::new().episodes(request)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = pt_video_common::request_key::<AnitubeConfig>(&request, "episode")
            .unwrap_or_else(|| "/video/sample".to_string());
        let referer = pt_video_common::absolute_url::<AnitubeConfig>(&episode);
        let body = fetch::<AnitubeConfig>(&referer, PLAYER_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let qualities = ["SD", "HD", "FHD"];
        let mut streams = Vec::new();
        for (idx, link) in doc
            .select(&selector("div.video_container > a[href], div.playerContainer > a[href]"))
            .take(3)
            .enumerate()
        {
            let quality = qualities.get(idx).copied().unwrap_or("HD");
            streams.extend(resolve_anitube_link(&attr(&link, "href"), quality, &referer, &request));
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<manatan_extension::HomeSection<CatalogItem>>> {
        PtVideoSource::<AnitubeConfig>::new().home(request)
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        PtVideoSource::<AnitubeConfig>::new().item_url(request)
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        PtVideoSource::<AnitubeConfig>::new().episode_url(request)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<manatan_extension::UrlResolveResult>> {
        PtVideoSource::<AnitubeConfig>::new().handle_url(request)
    }
}

fn resolve_anitube_link(link: &str, quality: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let final_link = absolute_remote(link, referer);
    let body = fetch::<AnitubeConfig>(&final_link, "", referer);
    let doc = Html::parse_document(&body);
    if let Some(refresh) = doc.select(&selector("meta[http-equiv=refresh]")).next().map(|el| attr(&el, "content")) {
        if let Some(next) = refresh.split('=').nth(1) {
            return resolve_anitube_link(next, quality, &final_link, request);
        }
    }
    let iframe = doc
        .select(&selector("iframe[src]"))
        .next()
        .map(|el| absolute_remote(&attr(&el, "src"), &final_link));
    let Some(player_url) = iframe else {
        return resolve_embed::<AnitubeConfig>(&final_link, quality, referer, request, 0);
    };
    let Some(video_url) = query_param(&player_url, "url") else {
        return resolve_embed::<AnitubeConfig>(&player_url, quality, &final_link, request, 0);
    };
    let token = fetch_video_token(&player_url, &video_url, &final_link).unwrap_or_default();
    let final_url = format!("{video_url}{token}");
    vec![stream_for_url::<AnitubeConfig>(&final_url, quality, &player_url, request)]
}

fn fetch_video_token(player_url: &str, video_url: &str, referer: &str) -> Option<String> {
    let player_body = client::<AnitubeConfig>(referer)
        .get(player_url)
        .referer(referer)
        .send_text()
        .ok()?;
    let ads_url = Regex::new(r#"(?:urlToFetch|ADS_URL)\s*=\s*['"]([^'"]+)['"]"#)
        .ok()?
        .captures(&player_body)
        .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        .unwrap_or_else(|| "https://widgets.outbrain.com/outbrain.js".to_string());
    let adblock_url = player_body
        .split("$.post")
        .nth(1)
        .and_then(|part| part.split('\'').nth(1))
        .filter(|url| url.starts_with("http"))
        .unwrap_or("https://ads.anitube.vip/adblock2.php")
        .to_string();
    let ads = client::<AnitubeConfig>(referer).get(ads_url).send_text().unwrap_or_default();
    let response = client::<AnitubeConfig>(referer)
        .post(&adblock_url)
        .xhr()
        .form(&[
            ("category", "client"),
            ("type", "premium"),
            ("ad", ads.as_str()),
            ("url", video_url),
        ])
        .send_text()
        .ok()?;
    let token = publicidade(&response).unwrap_or_else(|| "undefined".to_string());
    let second = client::<AnitubeConfig>(referer)
        .get(format!("{adblock_url}?token={token}&url={video_url}"))
        .xhr()
        .send_text()
        .ok()?;
    publicidade(&second).filter(|value| value.starts_with('?'))
}

fn publicidade(input: &str) -> Option<String> {
    input
        .split("\"publicidade\"")
        .nth(1)?
        .split('"')
        .nth(2)
        .map(ToString::to_string)
}

fn query_param(input: &str, key: &str) -> Option<String> {
    input.split('?').nth(1)?.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

const PLAYER_FIXTURE: &str = r#"<div class="video_container"><a href="https://example.invalid/player?url=https://example.invalid/video.mp4"></a></div>"#;

export_video_source!(SOURCE);
