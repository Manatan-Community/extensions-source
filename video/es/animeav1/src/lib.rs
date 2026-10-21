use std::{cmp::Ordering, collections::BTreeSet};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
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

const BASE_URL: &str = "https://animeav1.com";
const STREAMTAPE_REFERER: &str = "https://streamtape.com/";
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

#[derive(Default)]
pub struct AnimeAv1;

impl AnimeAv1 {
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

    fn browse(&self, page: u32, order: &str, query: Option<&str>) -> Result<Paged<CatalogItem>> {
        let mut url = Url::parse(&format!("{BASE_URL}/catalogo")).map_err(url_error)?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("order", order);
            pairs.append_pair("page", &page.max(1).to_string());
            if let Some(query) = query.filter(|value| !value.trim().is_empty()) {
                pairs.append_pair("search", query.trim());
            }
        }
        parse_catalog(&self.html(url.as_str())?, page.max(1))
    }

    fn extract_streamtape(&self, mirror: &PlayerMirror) -> Result<Vec<VideoStream>> {
        validate_streamtape_url(&mirror.url)?;
        let html = Client::browser()
            .get(&mirror.url)
            .header("Referer", STREAMTAPE_REFERER)
            .header("User-Agent", BROWSER_USER_AGENT)
            .timeout_ms(30_000)
            .max_body_bytes(4 * 1024 * 1024)
            .send()?
            .error_for_status()?
            .text()?
            .to_string();
        let media = parse_streamtape_media_url(&html)?;
        Ok(vec![VideoStream {
            url: media,
            name: Some(format!("AnimeAv1 {} · StreamTape", mirror.language)),
            format: Some("mp4".to_string()),
            requires_proxy: true,
            preferred: mirror.language == "SUB",
            initialized: true,
            headers: [
                ("Referer".to_string(), STREAMTAPE_REFERER.to_string()),
                ("User-Agent".to_string(), BROWSER_USER_AGENT.to_string()),
            ]
            .into_iter()
            .collect(),
            ..VideoStream::default()
        }])
    }

    fn extract_mp4upload(
        &self,
        mirror: &PlayerMirror,
        episode_url: &str,
    ) -> Result<Vec<VideoStream>> {
        validate_mp4upload_url(&mirror.url)?;
        let html = Client::browser()
            .get(&mirror.url)
            .header("Referer", episode_url)
            .header("User-Agent", BROWSER_USER_AGENT)
            .timeout_ms(30_000)
            .max_body_bytes(4 * 1024 * 1024)
            .send()?
            .error_for_status()?
            .text()?
            .to_string();
        let media = parse_mp4upload_media_url(&html)?;
        ensure_full_length(&media, &mirror.url)?;
        Ok(vec![VideoStream {
            url: media,
            name: Some(format!("AnimeAv1 {} · MP4Upload", mirror.language)),
            format: Some("mp4".to_string()),
            requires_proxy: true,
            preferred: mirror.language == "SUB",
            initialized: true,
            headers: [
                ("Referer".to_string(), mirror.url.clone()),
                ("User-Agent".to_string(), BROWSER_USER_AGENT.to_string()),
            ]
            .into_iter()
            .collect(),
            ..VideoStream::default()
        }])
    }

    fn capture_dynamic_player(
        &self,
        mirror: &PlayerMirror,
        episode_url: &str,
    ) -> Result<Vec<VideoStream>> {
        validate_dynamic_player_url(&mirror.url)?;
        let response: WebViewExtractResponse = browser::extract(&WebViewExtractRequest {
            url: mirror.url.clone(),
            method: Default::default(),
            body: None,
            cookie_url: None,
            session: Some(WebViewSession {
                id: "animeav1-player".to_string(),
                persistence: WebViewSessionPersistence::Persistent,
                ..WebViewSession::default()
            }),
            headers: vec![("Referer".to_string(), episode_url.to_string())],
            user_agent: Some(BROWSER_USER_AGENT.to_string()),
            wait_until: Some(WebViewWaitUntil::DomReady),
            wait_for_selector: None,
            wait_for_event: None,
            wait_for_script: Some(
                "(() => { const media = /\\.(m3u8|mp4)(?:\\?|$)/i; document.querySelector('video')?.play?.().catch(() => {}); document.querySelector('.vjs-big-play-button, .jw-icon-playback, button[aria-label=\"Play\"]')?.click(); return performance.getEntriesByType('resource').some(e => media.test(e.name)) || performance.now() >= 12000; })()".to_string(),
            ),
            script: "(() => { const media = /\\.(m3u8|mp4)(?:\\?|$)/i; const urls = performance.getEntriesByType('resource').map(e => e.name).filter(url => media.test(url)); const video = document.querySelector('video'); const direct = video?.currentSrc || video?.src || ''; if (media.test(direct)) urls.unshift(direct); return { urls: [...new Set(urls)] }; })()".to_string(),
            timeout_ms: Some(20_000),
            cookies: true,
            headless: Some(true),
            preload_scripts: Vec::new(),
            capture_requests: vec![capture(".m3u8"), capture(".mp4")],
            capture_events: Vec::new(),
        })?;
        captured_dynamic_streams(&response, mirror)
    }

    fn extract_voe(&self, mirror: &PlayerMirror, episode_url: &str) -> Result<Vec<VideoStream>> {
        validate_dynamic_player_url(&mirror.url)?;
        let redirect_page = self.remote_player_html(&mirror.url, episode_url)?;
        let redirect_url = parse_voe_redirect_url(&redirect_page)?;
        validate_voe_redirect_url(&redirect_url)?;
        let player_page = self.remote_player_html(&redirect_url, episode_url)?;
        let media_url = parse_voe_media_url(&player_page)?;
        validate_standard_media_url(&media_url)?;
        Client::browser()
            .get(&media_url)
            .header("Referer", redirect_url.clone())
            .header("User-Agent", BROWSER_USER_AGENT)
            .timeout_ms(20_000)
            .max_body_bytes(2 * 1024 * 1024)
            .send()?
            .error_for_status()?;
        let is_hls = media_url.to_ascii_lowercase().contains(".m3u8");
        Ok(vec![VideoStream {
            url: media_url,
            name: Some(format!("AnimeAv1 {} · Voe", mirror.language)),
            format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
            is_hls,
            requires_proxy: true,
            preferred: mirror.language == "SUB",
            initialized: true,
            headers: [
                ("Referer".to_string(), redirect_url),
                ("User-Agent".to_string(), BROWSER_USER_AGENT.to_string()),
            ]
            .into_iter()
            .collect(),
            ..VideoStream::default()
        }])
    }

    fn remote_player_html(&self, url: &str, referer: &str) -> Result<String> {
        Ok(Client::browser()
            .get(url)
            .header("Referer", referer)
            .header("User-Agent", BROWSER_USER_AGENT)
            .timeout_ms(30_000)
            .max_body_bytes(4 * 1024 * 1024)
            .send()?
            .error_for_status()?
            .text()?
            .to_string())
    }
}

fn parse_streamtape_media_url(html: &str) -> Result<String> {
    let expression = Regex::new(
            r#"document\.getElementById\('robotlink'\)\.innerHTML\s*=\s*'([^']+)'\s*\+\s*\('([^']+)'\)\.substring\((\d+)\)\.substring\((\d+)\)"#,
        )
        .map_err(regex_error)?
        .captures(html)
        .ok_or_else(|| Error::new("StreamTape did not expose its media expression"))?;
    let prefix = expression
        .get(1)
        .map(|value| value.as_str())
        .unwrap_or_default();
    let encoded = expression
        .get(2)
        .map(|value| value.as_str())
        .unwrap_or_default();
    let first = expression
        .get(3)
        .and_then(|value| value.as_str().parse::<usize>().ok())
        .ok_or_else(|| Error::new("StreamTape returned an invalid media expression"))?;
    let second = expression
        .get(4)
        .and_then(|value| value.as_str().parse::<usize>().ok())
        .ok_or_else(|| Error::new("StreamTape returned an invalid media expression"))?;
    let suffix = encoded
        .get(first..)
        .and_then(|value| value.get(second..))
        .ok_or_else(|| Error::new("StreamTape returned an invalid media expression"))?;
    let media = format!("https:{prefix}{suffix}");
    validate_streamtape_media_url(&media)?;
    Ok(media)
}

impl VideoSource for AnimeAv1 {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse(page, "popular", None)
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse(page, "latest_released", None)
    }

    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        self.browse(page, "popular", Some(query))
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
            return Err(Error::new("episode does not belong to this AnimeAv1 title"));
        }
        let html = self.html(&episode_url)?;
        let candidates = parse_player_candidates(&html)?;
        let mut errors = Vec::new();
        for candidate in &candidates {
            match self.extract_streamtape(candidate) {
                Ok(streams) if !streams.is_empty() => return Ok(streams),
                Ok(_) => errors.push(format!("{} returned no media", candidate.url)),
                Err(error) => errors.push(format!("{}: {error}", candidate.url)),
            }
        }

        let dynamic_candidates = parse_dynamic_candidates(&html)?;
        for candidate in &dynamic_candidates {
            let result = if Url::parse(&candidate.url)
                .ok()
                .and_then(|url| url.host_str().map(ToString::to_string))
                .as_deref()
                == Some("voe.sx")
            {
                self.extract_voe(candidate, &episode_url)
            } else {
                self.capture_dynamic_player(candidate, &episode_url)
            };
            match result {
                Ok(streams) if !streams.is_empty() => return Ok(streams),
                Ok(_) => errors.push(format!("{} returned no media", candidate.url)),
                Err(error) => errors.push(format!("{}: {error}", candidate.url)),
            }
        }

        let mp4upload_candidates = parse_mp4upload_candidates(&html)?;
        for candidate in &mp4upload_candidates {
            match self.extract_mp4upload(candidate, &episode_url) {
                Ok(streams) if !streams.is_empty() => return Ok(streams),
                Ok(_) => errors.push(format!("{} returned no media", candidate.url)),
                Err(error) => errors.push(format!("{}: {error}", candidate.url)),
            }
        }

        if !candidates.is_empty()
            || !dynamic_candidates.is_empty()
            || !mp4upload_candidates.is_empty()
        {
            return Err(Error::new(format!(
                "AnimeAv1 playback mirrors failed: {}",
                errors.join("; ")
            )));
        }
        Err(Error::new("AnimeAv1 returned no playable streams"))
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
    let cards = selector("article.group\\/item")?;
    let link = selector("a[href^='/media/']")?;
    let title = selector("header h3")?;
    let image = selector("figure img")?;
    if document.select(&cards).next().is_none() {
        return Err(Error::new("AnimeAv1 returned an unsupported catalog page"));
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
        let title = card
            .select(&title)
            .next()
            .map(|element| clean(&element.text().collect::<String>()))
            .filter(|value| !value.is_empty());
        let Some(title) = title else { continue };
        let cover = card.select(&image).next().and_then(image_request);
        items.push(CatalogItem {
            key: key.clone(),
            title,
            url: Some(format!("{BASE_URL}{key}")),
            cover,
            language: Some("es".to_string()),
            content_rating: Some("suggestive".to_string()),
            ..CatalogItem::default()
        });
    }
    let next_page = format!("page={}", page + 1);
    Ok(Paged::new(items, html.contains(&next_page)))
}

fn parse_details(html: &str, key: &str, url: &str) -> Result<CatalogItem> {
    let document = Html::parse_document(html);
    let title = required_text(&document, "h1")?;
    let cover = document
        .select(&selector(
            "figure img[alt$='Poster'], article img.object-cover",
        )?)
        .next()
        .and_then(image_request);
    let description = document
        .select(&selector(".entry p")?)
        .next()
        .map(|element| clean(&element.text().collect::<String>()))
        .filter(|value| !value.is_empty());
    let tags = document
        .select(&selector("a[href^='/catalogo?genre=']")?)
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
    let document = Html::parse_document(html);
    let pattern = format!("a[href^='{}/']", item_key.trim_end_matches('/'));
    let anchors = selector(&pattern)?;
    let mut episodes = Vec::new();
    let mut seen = BTreeSet::new();
    for anchor in document.select(&anchors) {
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        let Some(key) = episode_key(href, item_key) else {
            continue;
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        let number = key.rsplit('/').next().and_then(|value| value.parse().ok());
        episodes.push(VideoEpisode {
            key: key.clone(),
            title: number.map(|value: f32| format!("Episode {value}")),
            episode_number: number,
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct PlayerMirror {
    language: String,
    url: String,
}

fn parse_player_candidates(html: &str) -> Result<Vec<PlayerMirror>> {
    let embeds = html
        .split("embeds:")
        .nth(1)
        .and_then(|value| value.split(",downloads:").next())
        .ok_or_else(|| Error::new("AnimeAv1 episode embeds were not found"))?;
    let block = Regex::new(r#"(DUB|SUB):\[([^]]*)\]"#).map_err(regex_error)?;
    let streamtape = Regex::new(
        r#"\{server:\"StreamTape\",url:\"(https://streamtape\.com/e/[A-Za-z0-9_-]+/?)\"\}"#,
    )
    .map_err(regex_error)?;
    let mut candidates = Vec::new();
    for group in block.captures_iter(embeds) {
        let language = group.get(1).map(|value| value.as_str()).unwrap_or("ES");
        let priority = usize::from(language != "SUB");
        let body = group.get(2).map(|value| value.as_str()).unwrap_or_default();
        candidates.extend(
            streamtape
                .captures_iter(body)
                .filter_map(|capture| capture.get(1).map(|value| value.as_str().to_string()))
                .filter(|url| validate_streamtape_url(url).is_ok())
                .map(|url| {
                    (
                        priority,
                        PlayerMirror {
                            language: language.to_string(),
                            url,
                        },
                    )
                }),
        );
    }
    candidates.sort();
    candidates.dedup_by(|left, right| left.1.url == right.1.url);
    Ok(candidates.into_iter().map(|(_, mirror)| mirror).collect())
}

fn parse_dynamic_candidates(html: &str) -> Result<Vec<PlayerMirror>> {
    let embeds = html
        .split("embeds:")
        .nth(1)
        .and_then(|value| value.split(",downloads:").next())
        .ok_or_else(|| Error::new("AnimeAv1 episode embeds were not found"))?;
    let block = Regex::new(r#"(DUB|SUB):\[([^]]*)\]"#).map_err(regex_error)?;
    let player = Regex::new(r#"\{server:\"(?:Voe|Byse|UPNShare)\",url:\"(https://[^\"]+)\"\}"#)
        .map_err(regex_error)?;
    let mut candidates = Vec::new();
    for group in block.captures_iter(embeds) {
        let language = group.get(1).map(|value| value.as_str()).unwrap_or("ES");
        let body = group.get(2).map(|value| value.as_str()).unwrap_or_default();
        candidates.extend(player.captures_iter(body).filter_map(|capture| {
            let url = capture.get(1)?.as_str().to_string();
            validate_dynamic_player_url(&url).ok()?;
            Some(PlayerMirror {
                language: language.to_string(),
                url,
            })
        }));
    }
    candidates.sort_by_key(|mirror| {
        let priority = Url::parse(&mirror.url)
            .ok()
            .and_then(|url| url.host_str().map(ToString::to_string))
            .map_or(9, |host| match host.as_str() {
                "voe.sx" => 0,
                "byselapuix.com" => 1,
                "animeav1.uns.bio" => 2,
                _ => 9,
            });
        (mirror.language != "SUB", priority)
    });
    candidates.dedup_by(|left, right| left.url == right.url);
    candidates.truncate(6);
    Ok(candidates)
}

fn parse_mp4upload_candidates(html: &str) -> Result<Vec<PlayerMirror>> {
    let embeds = html
        .split("embeds:")
        .nth(1)
        .and_then(|value| value.split(",downloads:").next())
        .ok_or_else(|| Error::new("AnimeAv1 episode embeds were not found"))?;
    let block = Regex::new(r#"(DUB|SUB):\[([^]]*)\]"#).map_err(regex_error)?;
    let mp4upload = Regex::new(
        r#"\{server:\"MP4Upload\",url:\"(https://(?:www\.)?mp4upload\.com/embed-[A-Za-z0-9_-]+\.html)\"\}"#,
    )
    .map_err(regex_error)?;
    let mut candidates = Vec::new();
    for group in block.captures_iter(embeds) {
        let language = group.get(1).map(|value| value.as_str()).unwrap_or("ES");
        let body = group.get(2).map(|value| value.as_str()).unwrap_or_default();
        candidates.extend(mp4upload.captures_iter(body).filter_map(|capture| {
            let url = capture.get(1)?.as_str().to_string();
            validate_mp4upload_url(&url).ok()?;
            Some(PlayerMirror {
                language: language.to_string(),
                url,
            })
        }));
    }
    candidates.sort_by_key(|mirror| mirror.language != "SUB");
    candidates.dedup_by(|left, right| left.url == right.url);
    Ok(candidates)
}

fn parse_mp4upload_media_url(html: &str) -> Result<String> {
    let url = Regex::new(r#"src:\s*\"(https://[^\"]+/video\.mp4(?:\?[^\"]*)?)\""#)
        .map_err(regex_error)?
        .captures(html)
        .and_then(|capture| capture.get(1))
        .map(|value| value.as_str().to_string())
        .ok_or_else(|| Error::new("MP4Upload did not publish an MP4 source"))?;
    validate_mp4upload_media_url(&url)?;
    Ok(url)
}

fn parse_voe_redirect_url(html: &str) -> Result<String> {
    Regex::new(r#"window\.location\.href\s*=\s*'([^']+)'"#)
        .map_err(regex_error)?
        .captures(html)
        .and_then(|capture| capture.get(1))
        .map(|value| value.as_str().to_string())
        .ok_or_else(|| Error::new("Voe did not publish its player redirect"))
}

fn parse_voe_media_url(html: &str) -> Result<String> {
    let encoded = Regex::new(
        r#"<script[^>]+type=[\"']application/json[\"'][^>]*>\s*\[\"([^\"]+)\"\]\s*</script>"#,
    )
    .map_err(regex_error)?
    .captures(html)
    .and_then(|capture| capture.get(1))
    .map(|value| value.as_str())
    .ok_or_else(|| Error::new("Voe did not publish its encoded playback data"))?;
    let rotated: String = encoded
        .chars()
        .map(|character| match character {
            'A'..='Z' => (((character as u8 - b'A' + 13) % 26) + b'A') as char,
            'a'..='z' => (((character as u8 - b'a' + 13) % 26) + b'a') as char,
            _ => character,
        })
        .collect();
    let mut normalized = rotated;
    for pattern in ["@$", "^^", "~@", "%?", "*~", "!!", "#&"] {
        normalized = normalized.replace(pattern, "_");
    }
    normalized.retain(|character| character != '_');
    let shifted = BASE64
        .decode(normalized)
        .map_err(|error| Error::new(format!("Voe playback data was not base64: {error}")))?;
    let reversed: Vec<u8> = shifted
        .into_iter()
        .map(|byte| byte.wrapping_sub(3))
        .rev()
        .collect();
    let json = BASE64
        .decode(reversed)
        .map_err(|error| Error::new(format!("Voe playback payload was not base64: {error}")))?;
    let value: Value = serde_json::from_slice(&json)
        .map_err(|error| Error::new(format!("Voe playback payload was invalid: {error}")))?;
    value
        .get("source")
        .or_else(|| value.get("direct_access_url"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Error::new("Voe playback payload did not contain media"))
}

fn ensure_full_length(media_url: &str, referer: &str) -> Result<()> {
    let response = Client::browser()
        .get(media_url)
        .header("Referer", referer)
        .header("User-Agent", BROWSER_USER_AGENT)
        .header("Range", "bytes=0-0")
        .timeout_ms(20_000)
        .max_body_bytes(1024 * 1024)
        .send()?
        .error_for_status()?;
    let total = response
        .header("Content-Range")
        .and_then(|value| value.rsplit('/').next())
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| {
            response
                .header("Content-Length")
                .and_then(|value| value.parse::<u64>().ok())
        })
        .ok_or_else(|| Error::new("video host did not report the media size"))?;
    if total < 5 * 1024 * 1024 {
        return Err(Error::new("video host returned a truncated placeholder"));
    }
    Ok(())
}

fn captured_dynamic_streams(
    response: &WebViewExtractResponse,
    mirror: &PlayerMirror,
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
            let mut headers = response
                .captured_requests
                .iter()
                .find(|request| request.url == url)
                .map(|request| forwarded_headers(&request.headers))
                .unwrap_or_default();
            headers.insert("Referer".to_string(), mirror.url.clone());
            headers
                .entry("User-Agent".to_string())
                .or_insert_with(|| BROWSER_USER_AGENT.to_string());
            if let Some(cookie) = cookie_header(response, &url) {
                headers.insert("Cookie".to_string(), cookie);
            }
            VideoStream {
                url,
                name: Some(format!("AnimeAv1 {}", mirror.language)),
                format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
                is_hls,
                requires_proxy: true,
                preferred: mirror.language == "SUB",
                initialized: true,
                headers,
                ..VideoStream::default()
            }
        })
        .collect())
}

fn image_request(element: ElementRef<'_>) -> Option<ImageRequest> {
    element
        .value()
        .attr("src")
        .and_then(absolute_site_or_cdn_url)
        .map(|url| ImageRequest::get(url).header("Referer", format!("{BASE_URL}/")))
}

fn item_key(value: &str) -> Option<String> {
    let path = Url::parse(BASE_URL)
        .ok()?
        .join(value)
        .ok()?
        .path()
        .to_string();
    let parts: Vec<_> = path.split('/').filter(|part| !part.is_empty()).collect();
    (parts.len() == 2 && parts[0] == "media").then(|| format!("/media/{}", parts[1]))
}

fn episode_key(value: &str, item: &str) -> Option<String> {
    let path = Url::parse(BASE_URL)
        .ok()?
        .join(value)
        .ok()?
        .path()
        .to_string();
    let prefix = format!("{}/", item.trim_end_matches('/'));
    let suffix = path.strip_prefix(&prefix)?;
    (!suffix.is_empty() && !suffix.contains('/') && suffix.parse::<f32>().is_ok()).then_some(path)
}

fn item_url(key: &str) -> Result<String> {
    if item_key(key).as_deref() != Some(key) {
        return Err(Error::new("invalid AnimeAv1 item key"));
    }
    Ok(format!("{BASE_URL}{key}"))
}

fn episode_url(key: &str) -> Result<String> {
    let parts: Vec<_> = key.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() != 3 || parts[0] != "media" || parts[2].parse::<f32>().is_err() {
        return Err(Error::new("invalid AnimeAv1 episode key"));
    }
    Ok(format!("{BASE_URL}{key}"))
}

fn absolute_site_or_cdn_url(value: &str) -> Option<String> {
    let url = Url::parse(BASE_URL).ok()?.join(value).ok()?;
    let host = url.host_str()?;
    (url.scheme() == "https" && matches!(host, "animeav1.com" | "cdn.animeav1.com"))
        .then(|| url.to_string())
}

fn validate_site_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    if url.scheme() != "https" || url.host_str() != Some("animeav1.com") {
        return Err(Error::new("unexpected AnimeAv1 URL"));
    }
    Ok(())
}

fn validate_streamtape_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    if url.scheme() != "https"
        || url.host_str() != Some("streamtape.com")
        || !url.path().starts_with("/e/")
    {
        return Err(Error::new("unexpected AnimeAv1 player URL"));
    }
    Ok(())
}

fn validate_streamtape_media_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    if url.scheme() != "https"
        || url.host_str() != Some("streamtape.com")
        || url.path() != "/get_video"
        || url.query_pairs().all(|(key, _)| key != "id")
    {
        return Err(Error::new("StreamTape returned an unexpected media URL"));
    }
    Ok(())
}

fn validate_mp4upload_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || !matches!(host, "mp4upload.com" | "www.mp4upload.com")
        || !url.path().starts_with("/embed-")
        || !url.path().ends_with(".html")
    {
        return Err(Error::new("unexpected AnimeAv1 MP4Upload URL"));
    }
    Ok(())
}

fn validate_mp4upload_media_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || !(host == "mp4upload.com" || host.ends_with(".mp4upload.com"))
        || !url.path().ends_with("/video.mp4")
    {
        return Err(Error::new("MP4Upload returned an unexpected media URL"));
    }
    Ok(())
}

fn validate_dynamic_player_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("voe.sx" | "byselapuix.com" | "animeav1.uns.bio")
        )
    {
        return Err(Error::new("unexpected AnimeAv1 player URL"));
    }
    Ok(())
}

fn validate_voe_redirect_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    if url.scheme() != "https"
        || url.host_str() != Some("derricknationtop.com")
        || !url.path().starts_with("/e/")
    {
        return Err(Error::new("Voe returned an unexpected player redirect"));
    }
    Ok(())
}

fn validate_standard_media_url(value: &str) -> Result<()> {
    if !media_url(value) {
        return Err(Error::new("player returned an unexpected media URL"));
    }
    Ok(())
}

fn media_url(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    let host = url.host_str().unwrap_or_default();
    let lower = value.to_ascii_lowercase();
    url.scheme() == "https"
        && host.contains('.')
        && host != "localhost"
        && url.port_or_known_default() == Some(443)
        && (lower.contains(".m3u8") || lower.contains(".mp4"))
}

fn forwarded_headers(values: &[(String, String)]) -> std::collections::BTreeMap<String, String> {
    values
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                "cookie" | "origin" | "referer" | "user-agent"
            )
        })
        .cloned()
        .collect()
}

fn cookie_header(response: &WebViewExtractResponse, target: &str) -> Option<String> {
    let target = Url::parse(target).ok()?;
    let host = target.host_str()?;
    let path = target.path();
    let values: Vec<_> = response
        .cookies
        .iter()
        .filter(|cookie| {
            let domain = cookie.domain.trim_start_matches('.');
            (host == domain || host.ends_with(&format!(".{domain}")))
                && cookie
                    .path
                    .as_deref()
                    .is_none_or(|prefix| path.starts_with(prefix))
        })
        .map(|cookie| format!("{}={}", cookie.name, cookie.value))
        .collect();
    (!values.is_empty()).then(|| values.join("; "))
}

fn capture(needle: &str) -> WebViewRequestCapture {
    WebViewRequestCapture {
        url_contains: Some(needle.to_string()),
        limit: Some(16),
        ..WebViewRequestCapture::default()
    }
}

fn required_text(document: &Html, selector_text: &str) -> Result<String> {
    document
        .select(&selector(selector_text)?)
        .next()
        .map(|element| clean(&element.text().collect::<String>()))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new(format!("AnimeAv1 is missing {selector_text}")))
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
manatan_sdk::export_extension!(manatan_sdk::Extension::new().video("animeav1", AnimeAv1));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_catalog_details_episodes_and_direct_players() {
        let catalog = r#"<article class="group/item"><figure><img src="https://cdn.animeav1.com/covers/171.jpg"></figure><header><h3>Shingeki no Kyojin</h3></header><a href="/media/shingeki-no-kyojin"></a></article><a href="?page=2">Next</a>"#;
        let page = parse_catalog(catalog, 1).unwrap();
        assert_eq!(page.entries[0].title, "Shingeki no Kyojin");
        assert!(page.has_next_page);

        let details = r#"<h1>Shingeki no Kyojin</h1><figure><img alt="Shingeki Poster" src="https://cdn.animeav1.com/covers/171.jpg"></figure><div class="entry"><p>Humanity fights titans.</p></div><a href="/catalogo?genre=accion">Acción</a><a href="/media/shingeki-no-kyojin/1">Episode 1</a>"#;
        let item = parse_details(
            details,
            "/media/shingeki-no-kyojin",
            "https://animeav1.com/media/shingeki-no-kyojin",
        )
        .unwrap();
        assert_eq!(item.description.as_deref(), Some("Humanity fights titans."));
        let episodes = parse_episodes(details, &item.key).unwrap();
        assert_eq!(episodes[0].episode_number, Some(1.0));

        let episode = r#"embeds:{DUB:[{server:"HLS",url:"https://player.zilla-networks.com/play/dub123"},{server:"StreamTape",url:"https://streamtape.com/e/dub/"}],SUB:[{server:"HLS",url:"https://player.zilla-networks.com/play/sub123"},{server:"StreamTape",url:"https://streamtape.com/e/sub/"}]},downloads:{}"#;
        assert_eq!(
            parse_player_candidates(episode).unwrap(),
            vec![
                PlayerMirror {
                    language: "SUB".to_string(),
                    url: "https://streamtape.com/e/sub/".to_string(),
                },
                PlayerMirror {
                    language: "DUB".to_string(),
                    url: "https://streamtape.com/e/dub/".to_string(),
                }
            ]
        );
        let dynamic = parse_dynamic_candidates(
            r#"embeds:{DUB:[{server:"Byse",url:"https://byselapuix.com/e/dub"}],SUB:[{server:"Voe",url:"https://voe.sx/e/sub"},{server:"UPNShare",url:"https://animeav1.uns.bio/#sub"}]},downloads:{}"#,
        )
        .unwrap();
        assert_eq!(dynamic[0].language, "SUB");
        assert_eq!(dynamic[0].url, "https://voe.sx/e/sub");
        assert_eq!(
            parse_voe_redirect_url(
                "<script>window.location.href = 'https://derricknationtop.com/e/sub';</script>"
            )
            .unwrap(),
            "https://derricknationtop.com/e/sub"
        );
        let payload = r#"{"source":"https://media.example.com/master.m3u8"}"#;
        let inner = BASE64.encode(payload);
        let shifted: Vec<u8> = inner
            .bytes()
            .rev()
            .map(|byte| byte.wrapping_add(3))
            .collect();
        let outer = BASE64.encode(shifted);
        let encoded: String = outer
            .chars()
            .map(|character| match character {
                'A'..='Z' => (((character as u8 - b'A' + 13) % 26) + b'A') as char,
                'a'..='z' => (((character as u8 - b'a' + 13) % 26) + b'a') as char,
                _ => character,
            })
            .collect();
        assert_eq!(
            parse_voe_media_url(&format!(
                r#"<script type="application/json">["{encoded}"]</script>"#
            ))
            .unwrap(),
            "https://media.example.com/master.m3u8"
        );
        let mp4 = r#"embeds:{DUB:[{server:"MP4Upload",url:"https://www.mp4upload.com/embed-dub123.html"}],SUB:[{server:"MP4Upload",url:"https://www.mp4upload.com/embed-sub123.html"}]},downloads:{}"#;
        let mirrors = parse_mp4upload_candidates(mp4).unwrap();
        assert_eq!(mirrors[0].language, "SUB");
        assert_eq!(
            mirrors[0].url,
            "https://www.mp4upload.com/embed-sub123.html"
        );
        assert_eq!(
            parse_mp4upload_media_url(
                r#"player.setup({src: "https://a1.mp4upload.com:183/d/token/video.mp4"});"#
            )
            .unwrap(),
            "https://a1.mp4upload.com:183/d/token/video.mp4"
        );
        let player = r#"<script>document.getElementById('robotlink').innerHTML = '//streamtape.com/get_video?id'+ ('xcd=sub&expires=1&token=test').substring(2).substring(1);</script>"#;
        assert_eq!(
            parse_streamtape_media_url(player).unwrap(),
            "https://streamtape.com/get_video?id=sub&expires=1&token=test"
        );
    }

    #[test]
    fn rejects_cross_title_and_external_keys() {
        assert!(item_url("https://evil.example/media/foo").is_err());
        assert!(episode_url("/media/foo/not-a-number").is_err());
        assert!(validate_streamtape_url("https://streamtape.com.evil.example/e/id").is_err());
        assert!(
            validate_mp4upload_url("https://mp4upload.com.evil.example/embed-id.html").is_err()
        );
        assert!(validate_mp4upload_media_url("https://a1.mp4upload.com/d/id/video.mp4").is_ok());
        assert!(
            validate_streamtape_media_url("https://streamtape.com/get_video?id=abc&token=def")
                .is_ok()
        );
    }
}
