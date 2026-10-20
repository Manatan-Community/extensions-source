use std::{cmp::Ordering, collections::BTreeSet};

use manatan_sdk::{
    browser::{
        self, WebViewExtractRequest, WebViewExtractResponse, WebViewRequestCapture, WebViewSession,
        WebViewSessionPersistence, WebViewWaitUntil,
    },
    client::Client,
    CatalogItem, Error, ImageRequest, Paged, Result, VideoEpisode, VideoSource, VideoStream,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use url::Url;

const BASE_URL: &str = "https://www.anime-jl.net";
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

#[derive(Default)]
pub struct AnimeJl;

impl AnimeJl {
    fn html(&self, url: &str) -> Result<String> {
        validate_site_url(url)?;
        self.remote_html(url, &format!("{BASE_URL}/"))
    }

    fn remote_html(&self, url: &str, referer: &str) -> Result<String> {
        Ok(Client::browser()
            .get(url)
            .header("Referer", referer)
            .header("User-Agent", BROWSER_USER_AGENT)
            .timeout_ms(30_000)
            .max_body_bytes(8 * 1024 * 1024)
            .send()?
            .error_for_status()?
            .text()?
            .to_string())
    }

    fn browse(&self, page: u32, order: &str, query: Option<&str>) -> Result<Paged<CatalogItem>> {
        let mut url = Url::parse(&format!("{BASE_URL}/animes")).map_err(url_error)?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("order", order);
            pairs.append_pair("page", &page.max(1).to_string());
            if let Some(query) = query.filter(|value| !value.trim().is_empty()) {
                pairs.append_pair("q", query.trim());
            }
        }
        parse_catalog(&self.html(url.as_str())?, page.max(1))
    }

    fn okru_stream(&self, embed_url: &str, episode_url: &str) -> Result<VideoStream> {
        validate_player_url(embed_url)?;
        let body = self.remote_html(embed_url, episode_url)?;
        parse_okru_stream(&body, embed_url)
    }

    fn capture_player(&self, embed_url: &str, episode_url: &str) -> Result<Vec<VideoStream>> {
        validate_player_url(embed_url)?;
        let response: WebViewExtractResponse = browser::extract(&WebViewExtractRequest {
            url: embed_url.to_string(),
            method: Default::default(),
            body: None,
            cookie_url: None,
            session: Some(WebViewSession {
                id: "animejl-player".to_string(),
                persistence: WebViewSessionPersistence::Ephemeral,
                ..WebViewSession::default()
            }),
            headers: vec![("Referer".to_string(), episode_url.to_string())],
            user_agent: Some(BROWSER_USER_AGENT.to_string()),
            wait_until: Some(WebViewWaitUntil::DomReady),
            wait_for_selector: None,
            wait_for_event: None,
            wait_for_script: Some(
                "(() => { const media = /\\.(m3u8|mp4)(?:\\?|$)/i; document.querySelector('video')?.play?.().catch(() => {}); document.querySelector('.vjs-big-play-button, .jw-icon-playback, button[aria-label=\"Play\"]')?.click(); return performance.getEntriesByType('resource').some(e => media.test(e.name)) || performance.now() >= 25000; })()".to_string(),
            ),
            script: "(() => { const media = /\\.(m3u8|mp4)(?:\\?|$)/i; const urls = performance.getEntriesByType('resource').map(e => e.name).filter(url => media.test(url)); const video = document.querySelector('video'); const direct = video?.currentSrc || video?.src || ''; if (media.test(direct)) urls.unshift(direct); return { urls: [...new Set(urls)] }; })()".to_string(),
            timeout_ms: Some(60_000),
            headless: Some(true),
            capture_requests: vec![capture(".m3u8"), capture(".mp4")],
            capture_events: Vec::new(),
            cookies: false,
            preload_scripts: Vec::new(),
        })?;
        captured_streams(&response, embed_url)
    }
}

impl VideoSource for AnimeJl {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse(page, "rating", None)
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse(page, "updated", None)
    }

    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        self.browse(page, "rating", Some(query))
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = item_url(&item.key)?;
        parse_details(&self.html(&url)?, &item.key, &url)
    }

    fn episodes(&mut self, item: CatalogItem) -> Result<Vec<VideoEpisode>> {
        let url = item_url(&item.key)?;
        parse_episodes(&self.html(&url)?, &item.key)
    }

    fn streams(&mut self, item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoStream>> {
        let episode_url = episode_url(&episode.key)?;
        if !episode
            .key
            .starts_with(&format!("{}/", item.key.trim_end_matches('/')))
        {
            return Err(Error::new("episode does not belong to this AnimeJL title"));
        }
        let candidates = parse_player_candidates(&self.html(&episode_url)?)?;
        if candidates.is_empty() {
            return Err(Error::new("AnimeJL returned no supported playback mirrors"));
        }
        let mut errors = Vec::new();
        for candidate in &candidates {
            let host = Url::parse(candidate)
                .ok()
                .and_then(|url| url.host_str().map(ToString::to_string))
                .unwrap_or_default();
            let result = if matches!(host.as_str(), "ok.ru" | "www.ok.ru") {
                self.okru_stream(candidate, &episode_url)
                    .map(|stream| vec![stream])
            } else {
                self.capture_player(candidate, &episode_url)
            };
            match result {
                Ok(streams) if !streams.is_empty() => return Ok(streams),
                Ok(_) => errors.push(format!("{host} returned no media")),
                Err(error) => errors.push(format!("{host}: {error}")),
            }
        }
        Err(Error::new(format!(
            "AnimeJL playback mirrors failed: {}",
            errors.join("; ")
        )))
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(item_url(&item.key)?))
    }

    fn episode_url(
        &mut self,
        _item: &CatalogItem,
        episode: &VideoEpisode,
    ) -> Result<Option<String>> {
        Ok(Some(episode_url(&episode.key)?))
    }
}

fn parse_catalog(html: &str, page: u32) -> Result<Paged<CatalogItem>> {
    let document = Html::parse_document(html);
    let cards = selector("ul.ListAnimes li article.Anime")?;
    let link = selector("a[href*='/anime/']")?;
    let title = selector("h3.Title")?;
    let image = selector("div.Image figure img")?;
    if document.select(&cards).next().is_none() {
        return Err(Error::new("AnimeJL returned an unsupported catalog page"));
    }
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    for card in document.select(&cards) {
        let Some(href) = card
            .select(&link)
            .next()
            .and_then(|element| element.value().attr("href"))
        else {
            continue;
        };
        let Some(key) = item_key(href) else { continue };
        if !seen.insert(key.clone()) {
            continue;
        }
        let Some(title) = card
            .select(&title)
            .next()
            .map(|element| clean(&element.text().collect::<String>()))
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        items.push(CatalogItem {
            key: key.clone(),
            title,
            url: Some(format!("{BASE_URL}{key}")),
            cover: card.select(&image).next().and_then(image_request),
            language: Some("es".to_string()),
            content_rating: Some("suggestive".to_string()),
            ..CatalogItem::default()
        });
    }
    let next = format!("page={}", page + 1);
    Ok(Paged::new(items, html.contains(&next)))
}

fn parse_details(html: &str, key: &str, url: &str) -> Result<CatalogItem> {
    let document = Html::parse_document(html);
    let title = required_text(&document, ".Ficha.fchlt .Container > h1.Title")?;
    let cover = document
        .select(&selector(".AnimeCover .Image figure img")?)
        .next()
        .and_then(image_request);
    let description = document
        .select(&selector("main.Main .Description")?)
        .next()
        .map(|element| clean(&element.text().collect::<String>()))
        .filter(|value| !value.is_empty());
    let tags = document
        .select(&selector("nav.Nvgnrs a")?)
        .map(|element| clean(&element.text().collect::<String>()))
        .filter(|value| !value.is_empty())
        .collect();
    Ok(CatalogItem {
        key: key.to_string(),
        title,
        url: Some(url.to_string()),
        cover,
        description,
        tags,
        initialized: true,
        language: Some("es".to_string()),
        content_rating: Some("suggestive".to_string()),
        ..CatalogItem::default()
    })
}

fn parse_episodes(html: &str, item_key: &str) -> Result<Vec<VideoEpisode>> {
    let payload = Regex::new(r#"var\s+episodes\s*=\s*\[(.*?)\];"#)
        .map_err(regex_error)?
        .captures(html)
        .and_then(|capture| capture.get(1))
        .map(|value| value.as_str())
        .ok_or_else(|| Error::new("AnimeJL episode registry was not found"))?;
    let record = Regex::new(r#"\[([0-9]+(?:\.[0-9]+)?),\"([^\"]+)\""#).map_err(regex_error)?;
    let mut episodes = Vec::new();
    for capture in record.captures_iter(payload) {
        let Some(number) = capture.get(1).and_then(|value| value.as_str().parse().ok()) else {
            continue;
        };
        let Some(slug) = capture.get(2).map(|value| value.as_str()) else {
            continue;
        };
        if slug.contains('/') || !slug.starts_with("episodio-") {
            continue;
        }
        let key = format!("{}/{}", item_key.trim_end_matches('/'), slug);
        episodes.push(VideoEpisode {
            key: key.clone(),
            title: Some(format!("Episode {number}")),
            episode_number: Some(number),
            url: Some(format!("{BASE_URL}{key}")),
            language: Some("es".to_string()),
            ..VideoEpisode::default()
        });
    }
    episodes.sort_by(|left, right| {
        right
            .episode_number
            .partial_cmp(&left.episode_number)
            .unwrap_or(Ordering::Equal)
    });
    Ok(episodes)
}

fn parse_player_candidates(html: &str) -> Result<Vec<String>> {
    let iframe =
        Regex::new(r#"video\[\d+\]\s*=\s*'<iframe\s+src=\"([^\"]+)\""#).map_err(regex_error)?;
    let mut candidates: Vec<_> = iframe
        .captures_iter(html)
        .filter_map(|capture| capture.get(1).map(|value| value.as_str().to_string()))
        .filter(|url| !url.contains("hqq."))
        .filter(|url| validate_player_url(url).is_ok())
        .collect();
    candidates.sort_by_key(|url| player_priority(url));
    candidates.dedup();
    Ok(candidates)
}

fn parse_okru_stream(html: &str, embed_url: &str) -> Result<VideoStream> {
    let encoded = Regex::new(r#"hlsManifestUrl&quot;:&quot;([^&]+)&quot;"#)
        .map_err(regex_error)?
        .captures(html)
        .and_then(|capture| capture.get(1))
        .map(|value| value.as_str())
        .ok_or_else(|| Error::new("OK.ru did not publish an HLS manifest"))?;
    let url: String = serde_json::from_str(&format!("\"{encoded}\""))
        .map_err(|error| Error::new(format!("invalid OK.ru manifest URL: {error}")))?;
    validate_okcdn_url(&url)?;
    Ok(VideoStream {
        url,
        name: Some("AnimeJL OK.ru".to_string()),
        format: Some("hls".to_string()),
        is_hls: true,
        requires_proxy: true,
        preferred: true,
        initialized: true,
        headers: [
            ("Referer".to_string(), embed_url.to_string()),
            ("User-Agent".to_string(), BROWSER_USER_AGENT.to_string()),
        ]
        .into_iter()
        .collect(),
        ..VideoStream::default()
    })
}

fn captured_streams(
    response: &WebViewExtractResponse,
    embed_url: &str,
) -> Result<Vec<VideoStream>> {
    let mut urls: Vec<String> = response
        .captured_requests
        .iter()
        .map(|request| request.url.clone())
        .filter(|url| media_url(url))
        .collect();
    urls.extend(
        response
            .value
            .as_ref()
            .and_then(|value| value.get("urls"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|url| media_url(url))
            .map(ToString::to_string),
    );
    urls.sort();
    urls.dedup();
    if urls.is_empty() {
        return Err(Error::new("player did not expose a playable stream"));
    }
    Ok(urls
        .into_iter()
        .map(|url| {
            let is_hls = url.to_ascii_lowercase().contains(".m3u8");
            VideoStream {
                url,
                name: Some("AnimeJL mirror".to_string()),
                format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
                is_hls,
                requires_proxy: true,
                initialized: true,
                headers: [
                    ("Referer".to_string(), embed_url.to_string()),
                    ("User-Agent".to_string(), BROWSER_USER_AGENT.to_string()),
                ]
                .into_iter()
                .collect(),
                ..VideoStream::default()
            }
        })
        .collect())
}

fn image_request(element: ElementRef<'_>) -> Option<ImageRequest> {
    element
        .value()
        .attr("src")
        .and_then(absolute_site_url)
        .map(|url| ImageRequest::get(url).header("Referer", format!("{BASE_URL}/")))
}

fn item_key(value: &str) -> Option<String> {
    let url = Url::parse(BASE_URL).ok()?.join(value).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("www.anime-jl.net") {
        return None;
    }
    let parts: Vec<_> = url
        .path()
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    (parts.len() == 3 && parts[0] == "anime" && parts[1].parse::<u64>().is_ok())
        .then(|| format!("/anime/{}/{}", parts[1], parts[2]))
}

fn item_url(key: &str) -> Result<String> {
    if item_key(key).as_deref() != Some(key) {
        return Err(Error::new("invalid AnimeJL item key"));
    }
    Ok(format!("{BASE_URL}{key}"))
}

fn episode_url(key: &str) -> Result<String> {
    let parts: Vec<_> = key.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() != 4
        || parts[0] != "anime"
        || parts[1].parse::<u64>().is_err()
        || !parts[3].starts_with("episodio-")
        || parts[3]["episodio-".len()..].parse::<f32>().is_err()
    {
        return Err(Error::new("invalid AnimeJL episode key"));
    }
    Ok(format!("{BASE_URL}{key}"))
}

fn absolute_site_url(value: &str) -> Option<String> {
    let url = Url::parse(BASE_URL).ok()?.join(value).ok()?;
    (url.scheme() == "https" && url.host_str() == Some("www.anime-jl.net")).then(|| url.to_string())
}

fn validate_site_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    if url.scheme() != "https" || url.host_str() != Some("www.anime-jl.net") {
        return Err(Error::new("unexpected AnimeJL URL"));
    }
    Ok(())
}

fn validate_player_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || !matches!(
            host,
            "ok.ru"
                | "www.ok.ru"
                | "uqload.com"
                | "www.uqload.com"
                | "videovard.sx"
                | "www.videovard.sx"
                | "sbfull.com"
                | "www.sbfull.com"
                | "animejl.top"
                | "www.animejl.top"
        )
    {
        return Err(Error::new("unexpected AnimeJL player URL"));
    }
    Ok(())
}

fn validate_okcdn_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https" || !(host == "okcdn.ru" || host.ends_with(".okcdn.ru")) {
        return Err(Error::new("OK.ru returned an unexpected media URL"));
    }
    Ok(())
}

fn player_priority(value: &str) -> u8 {
    Url::parse(value)
        .ok()
        .and_then(|url| url.host_str().map(ToString::to_string))
        .map(|host| match host.as_str() {
            "ok.ru" | "www.ok.ru" => 0,
            "uqload.com" | "www.uqload.com" => 1,
            "videovard.sx" | "www.videovard.sx" => 2,
            "sbfull.com" | "www.sbfull.com" => 3,
            _ => 4,
        })
        .unwrap_or(9)
}

fn capture(needle: &str) -> WebViewRequestCapture {
    WebViewRequestCapture {
        url_contains: Some(needle.to_string()),
        limit: Some(12),
        ..WebViewRequestCapture::default()
    }
}

fn media_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains(".m3u8") || lower.contains(".mp4")
}

fn required_text(document: &Html, selector_text: &str) -> Result<String> {
    document
        .select(&selector(selector_text)?)
        .next()
        .map(|element| clean(&element.text().collect::<String>()))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new(format!("AnimeJL is missing {selector_text}")))
}

fn clean(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn selector(value: &str) -> Result<Selector> {
    Selector::parse(value).map_err(|_| Error::new(format!("invalid selector {value:?}")))
}

fn regex_error(error: regex::Error) -> Error {
    Error::new(format!("invalid regex: {error}"))
}

fn url_error(error: url::ParseError) -> Error {
    Error::new(format!("invalid URL: {error}"))
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(manatan_sdk::Extension::new().video("animejl", AnimeJl));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_catalog_details_and_episodes() {
        let catalog = r#"<ul class="ListAnimes"><li><article class="Anime"><a href="https://www.anime-jl.net/anime/512/example"><div class="Image"><figure><img src="/storage/cover.jpg"></figure></div><h3 class="Title">Example</h3></a></article></li></ul><a href="?page=2">2</a>"#;
        let page = parse_catalog(catalog, 1).unwrap();
        assert_eq!(page.entries[0].title, "Example");
        assert!(page.has_next_page);

        let details = r#"<div class="Ficha fchlt"><div class="Container"><h1 class="Title">Example</h1></div></div><div class="AnimeCover"><div class="Image"><figure><img src="/storage/cover.jpg"></figure></div></div><main class="Main"><nav class="Nvgnrs"><a>Drama</a></nav><div class="Description">A story.</div></main><script>var episodes = [[2,"episodio-2","x",""] ,[1,"episodio-1","x",""]];</script>"#;
        let item = parse_details(
            details,
            "/anime/512/example",
            "https://www.anime-jl.net/anime/512/example",
        )
        .unwrap();
        assert_eq!(item.description.as_deref(), Some("A story."));
        let episodes = parse_episodes(details, &item.key).unwrap();
        assert_eq!(episodes.len(), 2);
        assert_eq!(episodes[0].episode_number, Some(2.0));
    }

    #[test]
    fn prefers_okru_and_decodes_its_hls_manifest() {
        let html = r#"video[0] = '<iframe src="https://hqq.to/e/nope" frameborder="0"></iframe>'; video[1] = '<iframe src="https://uqload.com/embed-id.html" frameborder="0"></iframe>'; video[2] = '<iframe src="https://ok.ru/videoembed/123" frameborder="0"></iframe>';"#;
        let candidates = parse_player_candidates(html).unwrap();
        assert_eq!(candidates[0], "https://ok.ru/videoembed/123");
        assert!(!candidates.iter().any(|value| value.contains("hqq")));

        let ok = r#"hlsManifestUrl&quot;:&quot;https://vd296.okcdn.ru/video.m3u8?cmd=videoPlayerCdn\u0026id=123&quot;"#;
        let stream = parse_okru_stream(ok, &candidates[0]).unwrap();
        assert_eq!(
            stream.url,
            "https://vd296.okcdn.ru/video.m3u8?cmd=videoPlayerCdn&id=123"
        );
        assert!(stream.is_hls);
        assert_eq!(
            stream.headers.get("User-Agent").map(String::as_str),
            Some(BROWSER_USER_AGENT)
        );
    }

    #[test]
    fn rejects_external_content_and_media_urls() {
        assert!(item_url("https://evil.example/anime/1/nope").is_err());
        assert!(validate_okcdn_url("https://okcdn.ru.evil.example/video.m3u8").is_err());
    }
}
