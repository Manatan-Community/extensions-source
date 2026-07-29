use std::collections::BTreeSet;

use manatan_sdk::{
    browser::{
        self, WebViewExtractRequest, WebViewExtractResponse, WebViewRequestCapture, WebViewSession,
        WebViewSessionPersistence, WebViewWaitUntil,
    },
    client::Client,
    CatalogItem, Error, FilterDefinition, ImageRequest, OptionItem, Paged, Result, VideoEpisode,
    VideoSource, VideoStream,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use url::Url;

const BASE_URL: &str = "https://asianctv.in";

#[derive(Default)]
pub struct AsianCtv;

impl AsianCtv {
    fn html(&self, url: &str) -> Result<String> {
        validate_site_url(url)?;
        Ok(Client::browser()
            .get(url)
            .header("Referer", format!("{BASE_URL}/"))
            .timeout_ms(30_000)
            .max_body_bytes(8 * 1024 * 1024)
            .send()?
            .error_for_status()?
            .text()?
            .to_string())
    }

    fn listing_page(&self, url: &str) -> Result<Paged<CatalogItem>> {
        let html = self.html(url)?;
        parse_catalog_page(&html)
    }

    fn capture_player(&self, player_url: &str, episode_url: &str) -> Result<Vec<VideoStream>> {
        validate_player_url(player_url)?;
        if let Ok(streams) = self.player_api_streams(player_url, episode_url) {
            return Ok(streams);
        }
        let response: WebViewExtractResponse = browser::extract(&WebViewExtractRequest {
            url: player_url.to_string(),
            method: Default::default(),
            body: None,
            cookie_url: None,
            session: Some(WebViewSession {
                id: "vidbasic-player".to_string(),
                persistence: WebViewSessionPersistence::Persistent,
                ..WebViewSession::default()
            }),
            headers: vec![("Referer".to_string(), episode_url.to_string())],
            user_agent: None,
            wait_until: Some(WebViewWaitUntil::DomReady),
            wait_for_selector: None,
            wait_for_event: None,
            wait_for_script: Some(
                "performance.getEntriesByType('resource').some(e => e.name.includes('.m3u8') || e.name.includes('.mpd')) || performance.now() >= 12000"
                    .to_string(),
            ),
            script: "(() => { const v = document.querySelector('video'); const resources = performance.getEntriesByType('resource').map(e => e.name); return { url: v?.currentSrc || v?.src || '', urls: Array.from(new Set(resources.filter(u => typeof u === 'string' && (u.includes('.m3u8') || u.includes('.mpd'))))) }; })()".to_string(),
            timeout_ms: Some(45_000),
            headless: Some(true),
            capture_requests: vec![capture(".m3u8"), capture(".mpd"), capture(".vtt")],
            capture_events: Vec::new(),
            cookies: false,
            preload_scripts: Vec::new(),
        })?;
        streams_from_capture(&response, &player_origin(player_url)?)
    }

    fn player_api_streams(&self, player_url: &str, episode_url: &str) -> Result<Vec<VideoStream>> {
        let id = player_id(player_url)?;
        let value: Value = Client::browser()
            .get(&format!(
                "https://vidbasic.live/stream/getSources?id={id}&id={id}"
            ))
            .header("Referer", episode_url)
            .header("Origin", "https://vidbasic.live")
            .timeout_ms(30_000)
            .max_body_bytes(2 * 1024 * 1024)
            .send()?
            .error_for_status()?
            .json()?;
        streams_from_player_api(&value, &player_origin(player_url)?)
    }
}

impl VideoSource for AsianCtv {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing_page(&paged_url("/most-popular-drama/", page))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing_page(&paged_url("/", page))
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        if let Some(genre) = filters
            .get("genre")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            return self.listing_page(&paged_url(&format!("/genres/{genre}/"), page));
        }
        let mut url = Url::parse(BASE_URL).map_err(url_error)?;
        url.query_pairs_mut()
            .append_pair("s", query.trim())
            .append_pair("post_type", "series")
            .append_pair("paged", &page.max(1).to_string());
        self.listing_page(url.as_str())
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let slug = validate_slug(&item.key)?;
        parse_details(&self.html(&format!("{BASE_URL}/series/{slug}/"))?, slug)
    }

    fn episodes(&mut self, item: CatalogItem) -> Result<Vec<VideoEpisode>> {
        let slug = validate_slug(&item.key)?;
        parse_episodes(&self.html(&format!("{BASE_URL}/series/{slug}/"))?)
    }

    fn streams(&mut self, item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoStream>> {
        validate_slug(&item.key)?;
        let episode_url = canonical_episode_url(&episode.key)?;
        let allowed = self
            .episodes(item)?
            .into_iter()
            .any(|candidate| candidate.key == episode.key);
        if !allowed {
            return Err(Error::new(
                "episode does not belong to this AsianCTV series",
            ));
        }
        let player = parse_player_url(&self.html(&episode_url)?)?;
        self.capture_player(&player, &episode_url)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![FilterDefinition::Select {
            id: "genre".to_string(),
            name: "Genre".to_string(),
            options: [
                ("All", ""),
                ("Action", "action"),
                ("Comedy", "comedy"),
                ("Drama", "drama"),
                ("Fantasy", "fantasy"),
                ("Historical", "historical"),
                ("Horror", "horror"),
                ("Mystery", "mystery"),
                ("Romance", "romance"),
                ("Thriller", "thriller"),
                ("Wuxia", "wuxia"),
            ]
            .into_iter()
            .map(|(label, value)| OptionItem {
                label: label.to_string(),
                value: value.to_string(),
            })
            .collect(),
            default_index: 0,
        }])
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(format!(
            "{BASE_URL}/series/{}/",
            validate_slug(&item.key)?
        )))
    }

    fn episode_url(
        &mut self,
        _item: &CatalogItem,
        episode: &VideoEpisode,
    ) -> Result<Option<String>> {
        Ok(episode.url.clone())
    }
}

fn parse_catalog_page(html: &str) -> Result<Paged<CatalogItem>> {
    let document = Html::parse_document(html);
    let series_anchor = selector("a[href*=\"/series/\"]")?;
    let episode_anchor = selector(
        ".list-episode-item a[href*=\"episode\"], .list-episode-item-2 a[href*=\"episode\"]",
    )?;
    let image_selector = selector("img")?;
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();

    for anchor in document.select(&series_anchor) {
        let Some(slug) = series_slug(anchor.value().attr("href").unwrap_or_default()) else {
            continue;
        };
        if !seen.insert(slug.clone()) {
            continue;
        }
        let title = anchor
            .value()
            .attr("title")
            .map(clean)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| clean(&anchor.text().collect::<String>()));
        if title.is_empty() {
            continue;
        }
        entries.push(CatalogItem {
            key: slug,
            title,
            url: absolute(anchor.value().attr("href").unwrap_or_default()),
            cover: anchor
                .select(&image_selector)
                .next()
                .and_then(image_from_element),
            content_rating: Some("suggestive".to_string()),
            ..CatalogItem::default()
        });
    }

    if entries.is_empty() {
        for anchor in document.select(&episode_anchor) {
            let href = anchor.value().attr("href").unwrap_or_default();
            let Some(slug) = episode_to_series_slug(href) else {
                continue;
            };
            if !seen.insert(slug.clone()) {
                continue;
            }
            let title = anchor
                .value()
                .attr("title")
                .map(clean)
                .or_else(|| {
                    anchor
                        .select(&selector("h2, h3").ok()?)
                        .next()
                        .map(|element| clean(&element.text().collect::<String>()))
                })
                .unwrap_or_else(|| title_from_slug(&slug));
            entries.push(CatalogItem {
                key: slug.clone(),
                title: strip_episode_suffix(&title),
                url: Some(format!("{BASE_URL}/series/{slug}/")),
                cover: anchor
                    .select(&image_selector)
                    .next()
                    .and_then(image_from_element),
                content_rating: Some("suggestive".to_string()),
                ..CatalogItem::default()
            });
        }
    }
    let next = selector("a.next, a.next.page-numbers")?;
    Ok(Paged::new(entries, document.select(&next).next().is_some()))
}

fn parse_details(html: &str, slug: &str) -> Result<CatalogItem> {
    let document = Html::parse_document(html);
    let root = document
        .select(&selector(".details")?)
        .next()
        .ok_or_else(|| Error::new("AsianCTV series details were not found"))?;
    let title = text(&root, "h1")?;
    let cover = root
        .select(&selector(".img img")?)
        .next()
        .and_then(image_from_element);
    let tags = root
        .select(&selector("a[href*=\"/genres/\"]")?)
        .map(|element| clean(&element.text().collect::<String>()))
        .filter(|value| !value.is_empty())
        .collect();
    let info = clean(&root.text().collect::<String>());
    Ok(CatalogItem {
        key: slug.to_string(),
        title,
        url: Some(format!("{BASE_URL}/series/{slug}/")),
        cover,
        description: Some(info),
        tags,
        initialized: true,
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        ..CatalogItem::default()
    })
}

fn parse_episodes(html: &str) -> Result<Vec<VideoEpisode>> {
    let document = Html::parse_document(html);
    let anchors = selector(".all-episode a[href], .list-episode-item-2 a[href]")?;
    let number = Regex::new(r"(?i)episode[- ]+(\d+(?:\.\d+)?)").map_err(regex_error)?;
    let mut episodes = Vec::new();
    let mut seen = BTreeSet::new();
    for anchor in document.select(&anchors) {
        let href = anchor.value().attr("href").unwrap_or_default();
        let Some(url) = absolute(href) else {
            continue;
        };
        let Some(key) = episode_slug(&url) else {
            continue;
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        let title = anchor
            .select(&selector("h3, h2")?)
            .next()
            .map(|element| clean(&element.text().collect::<String>()))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_slug(&key));
        let episode_number = number
            .captures(&title)
            .and_then(|capture| capture.get(1))
            .and_then(|value| value.as_str().parse().ok());
        episodes.push(VideoEpisode {
            key,
            title: Some(title),
            episode_number,
            url: Some(url),
            language: Some("en".to_string()),
            ..VideoEpisode::default()
        });
    }
    episodes.sort_by(|left, right| {
        right
            .episode_number
            .partial_cmp(&left.episode_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(episodes)
}

fn parse_player_url(html: &str) -> Result<String> {
    let document = Html::parse_document(html);
    for selector_text in ["#block-tab-video iframe[src]", "[data-video]"] {
        let selector = selector(selector_text)?;
        for element in document.select(&selector) {
            if let Some(value) = element
                .value()
                .attr("src")
                .or_else(|| element.value().attr("data-video"))
            {
                validate_player_url(value)?;
                return Ok(value.to_string());
            }
        }
    }
    Err(Error::new("AsianCTV episode returned no video server"))
}

fn streams_from_capture(
    response: &WebViewExtractResponse,
    referer: &str,
) -> Result<Vec<VideoStream>> {
    let script_url = response
        .value
        .as_ref()
        .and_then(|value| value.get("url"))
        .and_then(Value::as_str);
    let mut urls: Vec<String> = response
        .captured_requests
        .iter()
        .map(|request| request.url.clone())
        .filter(|url| media_url(url))
        .collect();
    if let Some(value) = script_url.filter(|url| media_url(url)) {
        urls.push(value.to_string());
    }
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
        return Err(Error::new(
            "AsianCTV player did not expose a playable stream",
        ));
    }
    Ok(urls
        .into_iter()
        .map(|url| {
            let lower = url.to_ascii_lowercase();
            let is_hls = lower.contains(".m3u8");
            let is_dash = lower.contains(".mpd");
            VideoStream {
                url,
                name: Some("VidBasic".to_string()),
                format: Some(if is_dash { "dash" } else { "hls" }.to_string()),
                is_hls,
                is_dash,
                requires_proxy: true,
                initialized: true,
                headers: [("Referer".to_string(), referer.to_string())]
                    .into_iter()
                    .chain([("Origin".to_string(), "https://vidbasic.live".to_string())])
                    .into_iter()
                    .collect(),
                ..VideoStream::default()
            }
        })
        .collect())
}

fn streams_from_player_api(value: &Value, referer: &str) -> Result<Vec<VideoStream>> {
    let sources = value
        .get("sources")
        .ok_or_else(|| Error::new("VidBasic returned no sources"))?;
    let urls: Vec<&str> = if let Some(items) = sources.as_array() {
        items
            .iter()
            .filter_map(|item| item.get("file").and_then(Value::as_str))
            .collect()
    } else {
        sources
            .get("file")
            .and_then(Value::as_str)
            .into_iter()
            .collect()
    };
    if urls.is_empty() {
        return Err(Error::new("VidBasic returned no playable streams"));
    }
    Ok(urls
        .into_iter()
        .map(|url| {
            let lower = url.to_ascii_lowercase();
            let is_hls = lower.contains(".m3u8");
            let is_dash = lower.contains(".mpd");
            VideoStream {
                url: url.to_string(),
                name: Some("VidBasic".to_string()),
                format: Some(if is_dash { "dash" } else { "hls" }.to_string()),
                is_hls,
                is_dash,
                requires_proxy: true,
                initialized: true,
                headers: [
                    ("Referer".to_string(), referer.to_string()),
                    ("Origin".to_string(), "https://vidbasic.live".to_string()),
                ]
                .into_iter()
                .collect(),
                ..VideoStream::default()
            }
        })
        .collect())
}

fn player_id(value: &str) -> Result<String> {
    let url = Url::parse(value).map_err(url_error)?;
    let id = url
        .path_segments()
        .and_then(Iterator::last)
        .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| Error::new("invalid VidBasic player id"))?;
    Ok(id.to_string())
}

fn player_origin(value: &str) -> Result<String> {
    validate_player_url(value)?;
    let url = Url::parse(value).map_err(url_error)?;
    Ok(format!(
        "{}://{}/",
        url.scheme(),
        url.host_str().unwrap_or_default()
    ))
}

fn capture(needle: &str) -> WebViewRequestCapture {
    WebViewRequestCapture {
        url_contains: Some(needle.to_string()),
        limit: Some(8),
        ..WebViewRequestCapture::default()
    }
}

fn media_url(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains(".m3u8") || value.contains(".mpd")
}

fn validate_site_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    if url.scheme() != "https" || url.host_str() != Some("asianctv.in") {
        return Err(Error::new("unexpected AsianCTV URL"));
    }
    Ok(())
}

fn validate_player_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https" || !matches!(host, "vidbasic.live" | "megaplay.buzz") {
        return Err(Error::new("unexpected AsianCTV player URL"));
    }
    Ok(())
}

fn validate_slug(value: &str) -> Result<&str> {
    if value.is_empty()
        || value.len() > 180
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(Error::new("invalid AsianCTV slug"));
    }
    Ok(value)
}

fn canonical_episode_url(key: &str) -> Result<String> {
    Ok(format!("{BASE_URL}/{}/", validate_slug(key)?))
}

fn series_slug(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    if url.host_str()? != "asianctv.in" {
        return None;
    }
    let parts: Vec<_> = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect();
    (parts.len() == 2 && parts[0] == "series")
        .then(|| parts[1].to_string())
        .filter(|slug| validate_slug(slug).is_ok())
}

fn episode_slug(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    if url.host_str()? != "asianctv.in" {
        return None;
    }
    let slug = url.path_segments()?.find(|part| !part.is_empty())?;
    validate_slug(slug).ok().map(str::to_string)
}

fn episode_to_series_slug(value: &str) -> Option<String> {
    let slug = episode_slug(value)?;
    let regex = Regex::new(r"(?i)^(.+?)(?:-\d{4})?-episode-\d+(?:-\d+)?$").ok()?;
    let captures = regex.captures(&slug)?;
    let base = captures.get(1)?.as_str();
    let year = Regex::new(r"-(\d{4})-episode-")
        .ok()?
        .captures(&slug)
        .and_then(|capture| capture.get(1))
        .map(|value| value.as_str());
    Some(match year {
        Some(year) => format!("{base}-{year}"),
        None => base.to_string(),
    })
}

fn image_from_element(element: ElementRef<'_>) -> Option<ImageRequest> {
    ["data-original", "data-src", "src"]
        .into_iter()
        .find_map(|attribute| element.value().attr(attribute))
        .and_then(absolute)
        .map(|url| ImageRequest::get(url).header("Referer", format!("{BASE_URL}/")))
}

fn absolute(value: &str) -> Option<String> {
    Url::parse(BASE_URL)
        .ok()?
        .join(value)
        .ok()
        .map(|url| url.to_string())
}

fn paged_url(route: &str, page: u32) -> String {
    if page <= 1 {
        format!("{BASE_URL}{route}")
    } else {
        format!("{BASE_URL}{route}page/{page}/")
    }
}

fn text(root: &ElementRef<'_>, selector_text: &str) -> Result<String> {
    root.select(&selector(selector_text)?)
        .next()
        .map(|element| clean(&element.text().collect::<String>()))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new(format!("AsianCTV is missing {selector_text}")))
}

fn clean(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_episode_suffix(value: &str) -> String {
    Regex::new(r"(?i)\s+Episode\s+\d+.*$")
        .ok()
        .map(|regex| regex.replace(value, "").into_owned())
        .unwrap_or_else(|| value.to_string())
}

fn title_from_slug(slug: &str) -> String {
    slug.split('-')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
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
manatan_sdk::export_extension!(manatan_sdk::Extension::new().video("asianctv", AsianCtv));

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_series_details_episodes_and_player() {
        let series = r#"
          <div class="details"><div class="img"><img src="/cover.jpg"></div>
          <div class="info"><h1>Test Drama (2026)</h1><a href="/genres/romance/">Romance</a></div></div>
          <ul class="all-episode"><li><a href="https://asianctv.in/test-drama-2026-episode-2/">
          <h3>Test Drama (2026) Episode 2</h3></a></li></ul>
        "#;
        let details = parse_details(series, "test-drama-2026").unwrap();
        assert_eq!(details.title, "Test Drama (2026)");
        assert_eq!(details.tags, vec!["Romance"]);
        let episodes = parse_episodes(series).unwrap();
        assert_eq!(episodes[0].episode_number, Some(2.0));

        let episode = r#"<div id="block-tab-video"><iframe src="https://vidbasic.live/stream/s-1/123"></iframe></div>"#;
        assert_eq!(
            parse_player_url(episode).unwrap(),
            "https://vidbasic.live/stream/s-1/123"
        );
    }

    #[test]
    fn derives_series_from_episode_urls() {
        assert_eq!(
            episode_to_series_slug("https://asianctv.in/dream-to-you-2026-episode-6/").as_deref(),
            Some("dream-to-you-2026")
        );
        assert!(validate_player_url("https://evil.example/video").is_err());
        assert_eq!(
            canonical_episode_url("vincenzo-2021-episode-1").unwrap(),
            "https://asianctv.in/vincenzo-2021-episode-1/"
        );
        assert!(canonical_episode_url("https://evil.example/video").is_err());
    }

    #[test]
    fn reads_streams_from_webkit_resource_timing() {
        let response = WebViewExtractResponse {
            value: Some(json!({
                "url": "blob:https://vidbasic.live/player",
                "urls": [
                    "https://media.example/master.m3u8",
                    "https://media.example/poster.jpg"
                ]
            })),
            ..WebViewExtractResponse::default()
        };
        let streams = streams_from_capture(&response, "https://vidbasic.live/stream/s-1/123")
            .expect("resource timing should expose the native-WebKit HLS URL");
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].url, "https://media.example/master.m3u8");
        assert!(streams[0].is_hls);
        assert!(streams[0].requires_proxy);
    }

    #[test]
    fn parses_vidbasic_source_api() {
        assert_eq!(
            player_id("https://vidbasic.live/stream/s-1/107754").unwrap(),
            "107754"
        );
        assert!(player_id("https://vidbasic.live/stream/s-1/not-a-number").is_err());
        let streams = streams_from_player_api(
            &json!({
                "sources": {
                    "file": "https://n9f2k.newdramaplay.buzz/series/id/master.m3u8"
                }
            }),
            "https://vidbasic.live/",
        )
        .unwrap();
        assert_eq!(streams.len(), 1);
        assert!(streams[0].is_hls);
        assert!(streams[0].requires_proxy);
        assert_eq!(
            streams[0].headers.get("Referer").map(String::as_str),
            Some("https://vidbasic.live/")
        );
        assert_eq!(
            player_origin("https://vidbasic.live/stream/s-1/107754").unwrap(),
            "https://vidbasic.live/"
        );
    }
}
