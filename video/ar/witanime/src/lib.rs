use std::{cmp::Ordering, collections::BTreeSet};

use base64::{
    engine::general_purpose::{STANDARD as BASE64, STANDARD_NO_PAD as BASE64_NO_PAD},
    Engine as _,
};
use manatan_sdk::{
    browser::{
        self, WebViewExtractRequest, WebViewExtractResponse, WebViewRequestCapture, WebViewSession,
        WebViewSessionPersistence, WebViewWaitUntil,
    },
    client::Client,
    CatalogItem, Error, FilterDefinition, ImageRequest, MediaResourceKind, OptionItem, Paged,
    Result, SegmentProcessing, SegmentRule, VideoEpisode, VideoSource, VideoStream,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::Value;
use url::Url;

const BASE_URL: &str = "https://witanime.you";
const PLAYER_SCRIPT_URL: &str =
    "https://witanime.you/wp-content/themes/Anime-Online-Theme/assets/js/qh100.js";

#[derive(Default)]
pub struct WitAnime;

#[derive(Clone, Debug)]
struct PlayerCandidate {
    name: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct PlayerConfig {
    d: Vec<usize>,
    k: String,
}

#[derive(Debug, Deserialize)]
struct EncodedEpisode {
    number: String,
    url: String,
    #[serde(rename = "type")]
    kind: String,
}

impl WitAnime {
    fn html(&self, url: &str) -> Result<String> {
        validate_site_url(url)?;
        self.remote_html(url, &format!("{BASE_URL}/"))
    }

    fn remote_html(&self, url: &str, referer: &str) -> Result<String> {
        Ok(Client::browser()
            .get(url)
            .header("Referer", referer)
            .timeout_ms(30_000)
            .max_body_bytes(8 * 1024 * 1024)
            .send()?
            .error_for_status()?
            .text()?
            .to_string())
    }

    fn listing_page(&self, url: &str) -> Result<Paged<CatalogItem>> {
        parse_catalog_page(&self.html(url)?)
    }

    fn real_html(&self, url: &str) -> Result<(String, String)> {
        let html = self.html(url)?;
        let document = Html::parse_document(&html);
        let anime_link = selector("div.anime-page-link a[href*=\"/anime/\"]")?;
        let Some(next) = document
            .select(&anime_link)
            .next()
            .and_then(|element| element.value().attr("href"))
            .and_then(absolute_site_url)
        else {
            return Ok((html, url.to_string()));
        };
        Ok((self.html(&next)?, next))
    }

    fn framework_hash(&self) -> Result<String> {
        let script = self.remote_html(PLAYER_SCRIPT_URL, &format!("{BASE_URL}/"))?;
        parse_framework_hash(&script)
    }

    fn expanded_candidates(
        &self,
        episode_html: &str,
        episode_url: &str,
    ) -> Result<Vec<PlayerCandidate>> {
        let framework_hash = self.framework_hash()?;
        let mut candidates = parse_player_candidates(episode_html, &framework_hash)?;
        let yonaplay: Vec<_> = candidates
            .iter()
            .filter(|candidate| is_yonaplay(&candidate.url))
            .cloned()
            .collect();
        for candidate in yonaplay {
            let html = self.remote_html(&candidate.url, episode_url)?;
            candidates.extend(parse_yonaplay_candidates(&html)?);
        }
        for candidate in &mut candidates {
            candidate.url = canonical_player_url(&candidate.url);
        }
        candidates.retain(|candidate| validate_player_url(&candidate.url).is_ok());
        candidates.sort_by_key(candidate_priority);
        let mut seen = BTreeSet::new();
        candidates.retain(|candidate| seen.insert(candidate.url.clone()));
        Ok(candidates)
    }

    fn capture_player(
        &self,
        candidate: &PlayerCandidate,
        episode_url: &str,
    ) -> Result<Vec<VideoStream>> {
        validate_player_url(&candidate.url)?;
        let response: WebViewExtractResponse = browser::extract(&WebViewExtractRequest {
            url: candidate.url.clone(),
            method: Default::default(),
            body: None,
            cookie_url: None,
            session: Some(WebViewSession {
                id: "witanime-player".to_string(),
                persistence: WebViewSessionPersistence::Ephemeral,
                ..WebViewSession::default()
            }),
            headers: vec![("Referer".to_string(), episode_url.to_string())],
            user_agent: None,
            wait_until: Some(WebViewWaitUntil::DomReady),
            wait_for_selector: None,
            wait_for_event: None,
            wait_for_script: Some(
                r#"(() => {
                    const media = /\.m3u8(?:\?|$)|\.mpd(?:\?|$)|\.mp4(?:\?|$)/i;
                    const visit = (target, depth) => {
                        if (depth > 3) return false;
                        try {
                            target.document.querySelector(
                                '#pl_but, #pl_but_background, .vjs-big-play-button, .jw-icon-playback, button[aria-label="Play"]'
                            )?.click();
                            target.document.querySelector('video')?.play?.().catch(() => {});
                            const video = target.document.querySelector('video');
                            if (media.test(video?.currentSrc || video?.src || '')) return true;
                            if (target.performance.getEntriesByType('resource').some(entry => media.test(entry.name))) return true;
                            for (const frame of target.document.querySelectorAll('iframe')) {
                                try {
                                    if (frame.contentWindow && visit(frame.contentWindow, depth + 1)) return true;
                                } catch (_) {}
                            }
                        } catch (_) {}
                        return false;
                    };
                    return visit(window, 0) || performance.now() >= 30000;
                })()"#
                    .to_string(),
            ),
            script: r#"(() => {
                const media = /\.m3u8(?:\?|$)|\.mpd(?:\?|$)|\.mp4(?:\?|$)/i;
                const urls = [];
                const visit = (target, depth) => {
                    if (depth > 3) return;
                    try {
                        const video = target.document.querySelector('video');
                        const current = video?.currentSrc || video?.src || '';
                        if (media.test(current)) urls.push(current);
                        for (const source of target.document.querySelectorAll('video source[src]')) {
                            if (media.test(source.src)) urls.push(source.src);
                        }
                        for (const entry of target.performance.getEntriesByType('resource')) {
                            if (media.test(entry.name)) urls.push(entry.name);
                        }
                        for (const frame of target.document.querySelectorAll('iframe')) {
                            try {
                                if (frame.contentWindow) visit(frame.contentWindow, depth + 1);
                            } catch (_) {}
                        }
                    } catch (_) {}
                };
                visit(window, 0);
                return { url: urls[0] || '', urls: [...new Set(urls)] };
            })()"#
                    .to_string(),
            timeout_ms: Some(60_000),
            headless: Some(true),
            capture_requests: vec![capture(".m3u8"), capture(".mpd"), capture(".mp4")],
            capture_events: Vec::new(),
            cookies: false,
            preload_scripts: Vec::new(),
        })?;
        streams_from_capture(&response, candidate)
    }
}

impl VideoSource for WitAnime {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing_page(&paged_archive_url("/قائمة-الانمي/", page))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing_page(&paged_archive_url("/episode/", page))
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        if query.trim().is_empty() {
            if let Some(genre) = filter_value(filters, "genre").filter(|value| !value.is_empty()) {
                return self
                    .listing_page(&paged_archive_url(&format!("/anime-genre/{genre}/"), page));
            }
            if let Some(kind) = filter_value(filters, "type").filter(|value| !value.is_empty()) {
                return self
                    .listing_page(&paged_archive_url(&format!("/anime-type/{kind}/"), page));
            }
            return self.popular(page);
        }
        let mut url = Url::parse(BASE_URL).map_err(url_error)?;
        url.query_pairs_mut()
            .append_pair("search_param", "animes")
            .append_pair("s", query.trim())
            .append_pair("paged", &page.max(1).to_string());
        self.listing_page(url.as_str())
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let item_url = site_url_from_key(&item.key)?;
        let (html, canonical_url) = self.real_html(&item_url)?;
        parse_details(&html, &item.key, &canonical_url)
    }

    fn episodes(&mut self, item: CatalogItem) -> Result<Vec<VideoEpisode>> {
        let item_url = site_url_from_key(&item.key)?;
        let (html, _) = self.real_html(&item_url)?;
        parse_episodes(&html)
    }

    fn streams(&mut self, item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoStream>> {
        let episode_url = site_url_from_key(&episode.key)?;
        if !episode.key.starts_with("/episode/") {
            return Err(Error::new("WIT ANIME episode key is not an episode"));
        }
        let allowed = self
            .episodes(item)?
            .iter()
            .any(|candidate| candidate.key == episode.key);
        if !allowed {
            return Err(Error::new(
                "episode does not belong to this WIT ANIME title",
            ));
        }
        let html = self.html(&episode_url)?;
        let candidates = self.expanded_candidates(&html, &episode_url)?;
        if candidates.is_empty() {
            return Err(Error::new(
                "WIT ANIME returned no supported playback servers",
            ));
        }
        let mut errors = Vec::new();
        for candidate in candidates.iter().take(4) {
            match self.capture_player(candidate, &episode_url) {
                Ok(streams) if !streams.is_empty() => return Ok(streams),
                Ok(_) => errors.push(format!("{} returned no media", candidate.name)),
                Err(error) => errors.push(format!("{}: {error}", candidate.name)),
            }
        }
        Err(Error::new(format!(
            "WIT ANIME playback failed: {}",
            errors.join("; ")
        )))
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![
            select(
                "type",
                "Type",
                &[
                    ("All", ""),
                    ("TV", "tv"),
                    ("Movie", "movie"),
                    ("OVA", "ova"),
                    ("ONA", "ona"),
                    ("Special", "special"),
                ],
            ),
            select(
                "genre",
                "Genre",
                &[
                    ("All", ""),
                    ("Action", "أكشن"),
                    ("Adventure", "مغامرات"),
                    ("Comedy", "كوميدي"),
                    ("Drama", "دراما"),
                    ("Fantasy", "خيال"),
                    ("Horror", "رعب"),
                    ("Mystery", "غموض"),
                    ("Romance", "رومانسي"),
                    ("Sports", "رياضي"),
                ],
            ),
        ])
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(site_url_from_key(&item.key)?))
    }

    fn episode_url(
        &mut self,
        _item: &CatalogItem,
        episode: &VideoEpisode,
    ) -> Result<Option<String>> {
        Ok(Some(site_url_from_key(&episode.key)?))
    }
}

fn parse_catalog_page(html: &str) -> Result<Paged<CatalogItem>> {
    let document = Html::parse_document(html);
    let cards =
        selector("div.anime-list-content div.anime-card-poster div.ehover6, div.anime-card-poster div.ehover6")?;
    let anchor = selector("a")?;
    let image = selector("img")?;
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    for card in document.select(&cards) {
        let Some(link) = card.select(&anchor).next() else {
            continue;
        };
        let Some(url) = element_target_url(link) else {
            continue;
        };
        let Some(key) = content_key(&url) else {
            continue;
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        let Some(image) = card.select(&image).next() else {
            continue;
        };
        let title = image
            .value()
            .attr("alt")
            .map(clean)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| clean(&card.text().collect::<String>()));
        if title.is_empty() {
            continue;
        }
        items.push(CatalogItem {
            key,
            title,
            url: Some(url),
            cover: image_request(image),
            language: Some("ar".to_string()),
            content_rating: Some("suggestive".to_string()),
            ..CatalogItem::default()
        });
    }
    let next = selector("ul.pagination a.next, a.next.page-numbers")?;
    Ok(Paged::new(items, document.select(&next).next().is_some()))
}

fn parse_details(html: &str, key: &str, canonical_url: &str) -> Result<CatalogItem> {
    let document = Html::parse_document(html);
    let title = required_text(&document, "h1.anime-details-title")?;
    let cover = document
        .select(&selector("img.thumbnail")?)
        .next()
        .and_then(image_request);
    let tags = document
        .select(&selector("ul.anime-genres > li > a")?)
        .map(|element| clean(&element.text().collect::<String>()))
        .filter(|value| !value.is_empty())
        .collect();
    let mut description = document
        .select(&selector("div.anime-info")?)
        .map(|element| clean(&element.text().collect::<String>()))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if let Some(story) = document
        .select(&selector("p.anime-story")?)
        .next()
        .map(|element| clean(&element.text().collect::<String>()))
        .filter(|value| !value.is_empty())
    {
        description.push(story);
    }
    Ok(CatalogItem {
        key: key.to_string(),
        title,
        url: Some(canonical_url.to_string()),
        cover,
        description: (!description.is_empty()).then(|| description.join("\n")),
        tags,
        initialized: true,
        language: Some("ar".to_string()),
        content_rating: Some("suggestive".to_string()),
        ..CatalogItem::default()
    })
}

fn parse_episodes(html: &str) -> Result<Vec<VideoEpisode>> {
    let mut episodes = parse_encoded_episodes(html)?;
    let document = Html::parse_document(html);
    let anchors = selector("div.ehover6 > div.episodes-card-title > h3 a")?;
    let number = Regex::new(r"(\d+(?:\.\d+)?)\s*$").map_err(regex_error)?;
    let mut seen: BTreeSet<String> = episodes.iter().map(|episode| episode.key.clone()).collect();
    for anchor in document.select(&anchors) {
        let Some(url) = element_target_url(anchor) else {
            continue;
        };
        let Some(key) = content_key(&url).filter(|key| key.starts_with("/episode/")) else {
            continue;
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        let title = clean(&anchor.text().collect::<String>());
        let episode_number = number
            .captures(&title)
            .and_then(|capture| capture.get(1))
            .and_then(|value| value.as_str().parse().ok());
        episodes.push(VideoEpisode {
            key,
            title: Some(title),
            episode_number,
            url: Some(url),
            language: Some("ar".to_string()),
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

fn parse_encoded_episodes(html: &str) -> Result<Vec<VideoEpisode>> {
    let Some(marker) = html.find("processedEpisodeData") else {
        return Ok(Vec::new());
    };
    let remainder = &html[marker + "processedEpisodeData".len()..];
    let Some(assignment) = remainder.find('=') else {
        return Err(Error::new(
            "WIT ANIME encoded episode assignment is malformed",
        ));
    };
    let remainder = remainder[assignment + 1..].trim_start();
    let Some(quote) = remainder
        .chars()
        .next()
        .filter(|quote| matches!(quote, '\'' | '"'))
    else {
        return Err(Error::new("WIT ANIME encoded episode payload is malformed"));
    };
    let remainder = &remainder[quote.len_utf8()..];
    let Some(end) = remainder.find(quote) else {
        return Err(Error::new(
            "WIT ANIME encoded episode payload is unterminated",
        ));
    };
    let payload = &remainder[..end];
    let (encoded_data, encoded_key) = payload
        .split_once('.')
        .ok_or_else(|| Error::new("WIT ANIME encoded episode payload has no key"))?;
    let data = decode_base64(encoded_data)?;
    let key = decode_base64(encoded_key)?;
    if key.is_empty() {
        return Err(Error::new("WIT ANIME encoded episode key is empty"));
    }
    let decoded: Vec<u8> = data
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ key[index % key.len()])
        .collect();
    let records: Vec<EncodedEpisode> = serde_json::from_slice(&decoded)
        .map_err(|error| Error::new(format!("invalid WIT ANIME episode payload: {error}")))?;
    let mut episodes = Vec::new();
    let mut seen = BTreeSet::new();
    for record in records {
        let Some(url) = absolute_site_url(&record.url) else {
            continue;
        };
        let Some(key) = content_key(&url).filter(|key| key.starts_with("/episode/")) else {
            continue;
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        let episode_number = record.number.trim().parse().ok();
        let title = clean(&format!("{} {}", record.kind, record.number));
        episodes.push(VideoEpisode {
            key,
            title: (!title.is_empty()).then_some(title),
            episode_number,
            url: Some(url),
            language: Some("ar".to_string()),
            ..VideoEpisode::default()
        });
    }
    Ok(episodes)
}

fn parse_player_candidates(html: &str, framework_hash: &str) -> Result<Vec<PlayerCandidate>> {
    let resources = script_value(html, "_zH")?;
    let configs = script_value(html, "_zW")?;
    let resources: Vec<String> =
        serde_json::from_slice(&decode_base64(&resources)?).map_err(json_error)?;
    let configs: Vec<PlayerConfig> =
        serde_json::from_slice(&decode_base64(&configs)?).map_err(json_error)?;
    if resources.len() != configs.len() {
        return Err(Error::new("WIT ANIME player registries do not match"));
    }
    let document = Html::parse_document(html);
    let names: Vec<String> = document
        .select(&selector("ul#episode-servers li a")?)
        .map(|element| clean(&element.text().collect::<String>()))
        .collect();
    let mut candidates = Vec::new();
    for (index, (resource, config)) in resources.into_iter().zip(configs).enumerate() {
        let key = String::from_utf8(decode_base64(&config.k)?)
            .map_err(|error| Error::new(format!("invalid WIT ANIME player key: {error}")))?;
        let offset_index: usize = key
            .parse()
            .map_err(|_| Error::new("invalid WIT ANIME player offset index"))?;
        let offset = *config
            .d
            .get(offset_index)
            .ok_or_else(|| Error::new("WIT ANIME player offset is unavailable"))?;
        let reversed: String = resource
            .chars()
            .rev()
            .filter(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '+' | '/' | '=')
            })
            .collect();
        let decoded = String::from_utf8(decode_base64(&reversed)?)
            .map_err(|error| Error::new(format!("invalid WIT ANIME player URL: {error}")))?;
        if decoded.len() < offset {
            return Err(Error::new("WIT ANIME player offset exceeds its payload"));
        }
        let mut url = decoded[..decoded.len() - offset].to_string();
        if is_yonaplay(&url) {
            let mut parsed = Url::parse(&url).map_err(url_error)?;
            parsed
                .query_pairs_mut()
                .append_pair("apiKey", framework_hash);
            url = parsed.to_string();
        }
        if validate_player_url(&url).is_err() {
            continue;
        }
        candidates.push(PlayerCandidate {
            name: names
                .get(index)
                .cloned()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("Server {}", index + 1)),
            url,
        });
    }
    Ok(candidates)
}

fn parse_yonaplay_candidates(html: &str) -> Result<Vec<PlayerCandidate>> {
    let document = Html::parse_document(html);
    let servers = selector(".OD li[onclick]")?;
    let encoded = Regex::new(r#"go_to_player\('([^']+)'\)"#).map_err(regex_error)?;
    let mut candidates = Vec::new();
    for server in document.select(&servers) {
        let Some(value) = server
            .value()
            .attr("onclick")
            .and_then(|value| encoded.captures(value))
            .and_then(|capture| capture.get(1))
            .map(|value| value.as_str())
        else {
            continue;
        };
        let Ok(bytes) = decode_base64(value) else {
            continue;
        };
        let Ok(url) = String::from_utf8(bytes) else {
            continue;
        };
        if validate_player_url(&url).is_err() {
            continue;
        }
        candidates.push(PlayerCandidate {
            name: clean(&server.text().collect::<String>()),
            url,
        });
    }
    Ok(candidates)
}

fn parse_framework_hash(script: &str) -> Result<String> {
    let parts = Regex::new(r#"_m[1-4]\s*=\s*"([^"]+)""#)
        .map_err(regex_error)?
        .captures_iter(script)
        .filter_map(|capture| capture.get(1))
        .map(|value| value.as_str())
        .take(4)
        .collect::<Vec<_>>();
    if parts.len() != 4 {
        return Err(Error::new("WIT ANIME player API key was not found"));
    }
    Ok(parts.concat())
}

fn streams_from_capture(
    response: &WebViewExtractResponse,
    candidate: &PlayerCandidate,
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
            .and_then(|value| value.get("url"))
            .and_then(Value::as_str)
            .filter(|url| media_url(url))
            .map(ToString::to_string),
    );
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
        return Err(Error::new(format!(
            "{} did not expose a playable stream",
            candidate.name
        )));
    }
    let origin = player_origin(&candidate.url)?;
    Ok(urls
        .into_iter()
        .map(|url| {
            let lower = url.to_ascii_lowercase();
            let is_hls = lower.contains(".m3u8");
            let is_dash = lower.contains(".mpd");
            VideoStream {
                url,
                name: Some(candidate.name.clone()),
                format: Some(
                    if is_hls {
                        "hls"
                    } else if is_dash {
                        "dash"
                    } else {
                        "mp4"
                    }
                    .to_string(),
                ),
                is_hls,
                is_dash,
                requires_proxy: true,
                initialized: true,
                headers: [
                    ("Referer".to_string(), candidate.url.clone()),
                    ("Origin".to_string(), origin.clone()),
                ]
                .into_iter()
                .collect(),
                segment_processing: Some(SegmentProcessing {
                    rewrite_playlist: true,
                    rules: vec![SegmentRule {
                        resource_types: vec![MediaResourceKind::Segment],
                        host_patterns: vec!["*.tiktokcdn.com".to_string()],
                        auto_detect_media_offset: true,
                        probe_bytes: Some(64 * 1024),
                        ..SegmentRule::default()
                    }],
                    ..SegmentProcessing::default()
                }),
                ..VideoStream::default()
            }
        })
        .collect())
}

fn element_target_url(element: ElementRef<'_>) -> Option<String> {
    let href = element.value().attr("href").unwrap_or_default();
    if !href.starts_with("javascript:") {
        return absolute_site_url(href);
    }
    let onclick = element.value().attr("onclick")?;
    let encoded = onclick.split('\'').nth(1)?;
    let decoded = String::from_utf8(decode_base64(encoded).ok()?).ok()?;
    absolute_site_url(&decoded)
}

fn script_value(html: &str, name: &str) -> Result<String> {
    let regex = Regex::new(&format!(r#"var\s+{}\s*=\s*"([^"]+)""#, regex::escape(name)))
        .map_err(regex_error)?;
    regex
        .captures(html)
        .and_then(|capture| capture.get(1))
        .map(|value| value.as_str().to_string())
        .ok_or_else(|| Error::new(format!("WIT ANIME is missing {name}")))
}

fn decode_base64(value: &str) -> Result<Vec<u8>> {
    BASE64
        .decode(value)
        .or_else(|_| BASE64_NO_PAD.decode(value))
        .map_err(|error| Error::new(format!("invalid base64: {error}")))
}

fn image_request(element: ElementRef<'_>) -> Option<ImageRequest> {
    ["data-original", "data-src", "src"]
        .into_iter()
        .find_map(|attribute| element.value().attr(attribute))
        .and_then(absolute_site_url)
        .map(|url| ImageRequest::get(url).header("Referer", format!("{BASE_URL}/")))
}

fn content_key(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    if url.scheme() != "https" || url.host_str()? != "witanime.you" {
        return None;
    }
    let segments: Vec<_> = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() != 2 || !matches!(segments[0], "anime" | "episode") {
        return None;
    }
    Some(format!("/{}/{}/", segments[0], segments[1]))
}

fn site_url_from_key(key: &str) -> Result<String> {
    if key.len() > 320 || !key.starts_with('/') || key.contains("..") {
        return Err(Error::new("invalid WIT ANIME content key"));
    }
    let url = absolute_site_url(key).ok_or_else(|| Error::new("invalid WIT ANIME content key"))?;
    if content_key(&url).as_deref() != Some(key) {
        return Err(Error::new("invalid WIT ANIME content key"));
    }
    Ok(url)
}

fn absolute_site_url(value: &str) -> Option<String> {
    let url = Url::parse(BASE_URL).ok()?.join(value).ok()?;
    validate_site_url(url.as_str()).ok()?;
    Some(url.to_string())
}

fn validate_site_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    if url.scheme() != "https" || url.host_str() != Some("witanime.you") {
        return Err(Error::new("unexpected WIT ANIME URL"));
    }
    Ok(())
}

fn validate_player_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || !matches!(
            host,
            "yonaplay.net"
                | "www.yonaplay.net"
                | "videa.hu"
                | "www.videa.hu"
                | "app.videas.fr"
                | "hgcloud.to"
                | "audinifer.com"
                | "mega.nz"
                | "www.4shared.com"
                | "my.mail.ru"
                | "vk.com"
        )
    {
        return Err(Error::new("unexpected WIT ANIME player URL"));
    }
    Ok(())
}

fn is_yonaplay(value: &str) -> bool {
    Url::parse(value)
        .ok()
        .and_then(|url| url.host_str().map(ToString::to_string))
        .is_some_and(|host| matches!(host.as_str(), "yonaplay.net" | "www.yonaplay.net"))
}

fn canonical_player_url(value: &str) -> String {
    let Ok(mut url) = Url::parse(value) else {
        return value.to_string();
    };
    if url.host_str() == Some("hgcloud.to") {
        let _ = url.set_host(Some("audinifer.com"));
    }
    url.to_string()
}

fn candidate_priority(candidate: &PlayerCandidate) -> (u8, u8) {
    let host = Url::parse(&candidate.url)
        .ok()
        .and_then(|url| url.host_str().map(ToString::to_string))
        .unwrap_or_default();
    let host_priority = match host.as_str() {
        "hgcloud.to" | "audinifer.com" => 0,
        "app.videas.fr" => 1,
        "videa.hu" | "www.videa.hu" => 2,
        "yonaplay.net" | "www.yonaplay.net" => 3,
        _ => 4,
    };
    let quality_priority = u8::from(!candidate.name.to_ascii_lowercase().contains("fhd"));
    (host_priority, quality_priority)
}

fn player_origin(value: &str) -> Result<String> {
    validate_player_url(value)?;
    let url = Url::parse(value).map_err(url_error)?;
    Ok(format!(
        "{}://{}",
        url.scheme(),
        url.host_str().unwrap_or_default()
    ))
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
    lower.contains(".m3u8") || lower.contains(".mpd") || lower.contains(".mp4")
}

fn paged_archive_url(route: &str, page: u32) -> String {
    let base = absolute_site_url(route).unwrap_or_else(|| format!("{BASE_URL}/"));
    if page <= 1 {
        base
    } else {
        format!("{}/page/{}/", base.trim_end_matches('/'), page.max(1))
    }
}

fn filter_value<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).and_then(Value::as_str).map(str::trim)
}

fn select(id: &str, name: &str, options: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Select {
        id: id.to_string(),
        name: name.to_string(),
        options: options
            .iter()
            .map(|(label, value)| OptionItem {
                label: (*label).to_string(),
                value: (*value).to_string(),
            })
            .collect(),
        default_index: 0,
    }
}

fn required_text(document: &Html, selector_text: &str) -> Result<String> {
    document
        .select(&selector(selector_text)?)
        .next()
        .map(|element| clean(&element.text().collect::<String>()))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new(format!("WIT ANIME is missing {selector_text}")))
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

fn json_error(error: serde_json::Error) -> Error {
    Error::new(format!("invalid WIT ANIME player registry: {error}"))
}

fn url_error(error: url::ParseError) -> Error {
    Error::new(format!("invalid URL: {error}"))
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(manatan_sdk::Extension::new().video("witanime", WitAnime));

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CATALOG: &str = r#"
      <div class="anime-list-content"><div class="anime-card-poster">
        <div class="hover ehover6">
          <a href="https://witanime.you/anime/one-piece/"></a>
          <img src="https://witanime.you/wp-content/one-piece.jpg" alt="One Piece">
        </div>
      </div></div>
      <ul class="pagination"><li><a class="next" href="/قائمة-الانمي/page/2/">Next</a></li></ul>
    "#;

    const DETAILS: &str = r#"
      <img src="https://witanime.you/wp-content/one-piece.jpg" class="thumbnail">
      <h1 class="anime-details-title">One Piece</h1>
      <ul class="anime-genres"><li><a>أكشن</a></li><li><a>مغامرات</a></li></ul>
      <div class="anime-info"><span>حالة الأنمي:</span> يعرض الان</div>
      <p class="anime-story">قراصنة يبحثون عن الكنز الأسطوري.</p>
      <div class="ehover6"><div class="episodes-card-title"><h3>
        <a href="javascript:void(0);" onclick="openEpisode('aHR0cHM6Ly93aXRhbmltZS55b3UvZXBpc29kZS9vbmUtcGllY2UtJUQ4JUE3JUQ5JTg0JUQ4JUFEJUQ5JTg0JUQ5JTgyJUQ4JUE5LTExNzAv')">الحلقة 1170</a>
      </h3></div></div>
    "#;

    #[test]
    fn parses_catalog_details_and_episodes() {
        let catalog = parse_catalog_page(CATALOG).unwrap();
        assert_eq!(catalog.entries.len(), 1);
        assert_eq!(catalog.entries[0].key, "/anime/one-piece/");
        assert!(catalog.has_next_page);
        assert_eq!(
            catalog.entries[0]
                .cover
                .as_ref()
                .and_then(|cover| cover.headers.get("Referer"))
                .map(String::as_str),
            Some("https://witanime.you/")
        );

        let details = parse_details(
            DETAILS,
            "/anime/one-piece/",
            "https://witanime.you/anime/one-piece/",
        )
        .unwrap();
        assert_eq!(details.title, "One Piece");
        assert_eq!(details.tags, vec!["أكشن", "مغامرات"]);
        assert!(details.description.unwrap().contains("الكنز"));

        let episodes = parse_episodes(DETAILS).unwrap();
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].episode_number, Some(1170.0));
        assert_eq!(
            episodes[0].key,
            "/episode/one-piece-%D8%A7%D9%84%D8%AD%D9%84%D9%82%D8%A9-1170/"
        );
    }

    #[test]
    fn decodes_current_episode_payload() {
        let records = serde_json::to_vec(&vec![json!({
            "number": "1171",
            "url": "https://witanime.you/episode/one-piece-1171/",
            "type": "الحلقة",
            "screenshot": "https://witanime.you/wp-content/one-piece-1171.jpg"
        })])
        .unwrap();
        let key = b"current-key";
        let encrypted: Vec<u8> = records
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ key[index % key.len()])
            .collect();
        let payload = format!("{}.{}", BASE64.encode(encrypted), BASE64.encode(key));
        let html = format!(r#"<script>var processedEpisodeData = '{payload}';</script>"#);
        let episodes = parse_episodes(&html).unwrap();
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].episode_number, Some(1171.0));
        assert_eq!(episodes[0].title.as_deref(), Some("الحلقة 1171"));
        assert_eq!(episodes[0].key, "/episode/one-piece-1171/");
    }

    #[test]
    fn builds_archive_pagination_urls() {
        assert_eq!(
            paged_archive_url("/episode/", 1),
            "https://witanime.you/episode/"
        );
        assert_eq!(
            paged_archive_url("/episode/", 2),
            "https://witanime.you/episode/page/2/"
        );
        assert_eq!(
            paged_archive_url("/anime-genre/action/", 3),
            "https://witanime.you/anime-genre/action/page/3/"
        );
    }

    #[test]
    fn decodes_current_player_registry() {
        let resource = "%=!=gY0&IzY%jN@DM5gz#N5cDO*4E#TPk%l2Pw^h*GcuQW%Z!i1WZ!vQX#Zu#5Sehx*Gch52*b5#9yL!6MHc%0*RHa";
        let unsupported = BASE64.encode("https://unsupported-player.example/embed/abc".as_bytes());
        let unsupported: String = unsupported.chars().rev().collect();
        let resources =
            BASE64.encode(serde_json::to_vec(&vec![resource.to_string(), unsupported]).unwrap());
        let configs = BASE64.encode(
            serde_json::to_vec(&vec![
                json!({
                    "d": [77,43,88,87,33,64,39,10,85,79],
                    "k": "Nw=="
                }),
                json!({
                    "d": [0],
                    "k": "MA=="
                }),
            ])
            .unwrap(),
        );
        let html = format!(
            r#"<script>var _zH="{resources}";var _zW="{configs}";</script>
               <ul id="episode-servers">
                 <li><a>yonaplay - FHD</a></li>
                 <li><a>Unsupported mirror</a></li>
               </ul>"#
        );
        let candidates =
            parse_player_candidates(&html, "23a97133-caf3-4eb4-9466-93d0a4ff8198").unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].url,
            "https://yonaplay.net/embed.php?id=18879&apiKey=23a97133-caf3-4eb4-9466-93d0a4ff8198"
        );
    }

    #[test]
    fn decodes_yonaplay_servers_and_rejects_unapproved_hosts() {
        let encoded = BASE64_NO_PAD.encode("https://hgcloud.to/e/abc123");
        let html = format!(
            r#"<div class="OD"><li onclick="go_to_player('{encoded}')"><span>StreamWish FHD</span></li></div>"#
        );
        let candidates = parse_yonaplay_candidates(&html).unwrap();
        assert_eq!(candidates[0].url, "https://hgcloud.to/e/abc123");
        assert!(validate_player_url("https://evil.example/embed/abc").is_err());
        assert!(content_key("https://evil.example/anime/one-piece/").is_none());
        assert_eq!(
            canonical_player_url("https://hgcloud.to/e/abc123"),
            "https://audinifer.com/e/abc123"
        );
    }

    #[test]
    fn maps_native_capture_to_proxied_streams() {
        let response = WebViewExtractResponse {
            value: Some(json!({
                "url": "https://cdn.hgcloud.to/video/master.m3u8",
                "urls": ["https://cdn.hgcloud.to/video/master.m3u8"]
            })),
            ..WebViewExtractResponse::default()
        };
        let streams = streams_from_capture(
            &response,
            &PlayerCandidate {
                name: "StreamWish FHD".to_string(),
                url: "https://hgcloud.to/e/abc123".to_string(),
            },
        )
        .unwrap();
        assert_eq!(streams.len(), 1);
        assert!(streams[0].is_hls);
        assert!(streams[0].requires_proxy);
        assert_eq!(
            streams[0].headers.get("Origin").map(String::as_str),
            Some("https://hgcloud.to")
        );
        let processing = streams[0].segment_processing.as_ref().unwrap();
        assert!(processing.rewrite_playlist);
        assert_eq!(
            processing.rules[0].resource_types,
            vec![MediaResourceKind::Segment]
        );
        assert!(processing.rules[0].auto_detect_media_offset);
    }

    #[test]
    fn reads_live_framework_hash_shape() {
        let script =
            r#"var _m1 = "23a9", _m2 = "7133-caf3-", _m3 = "4eb4-9466-", _m4 = "93d0a4ff8198";"#;
        assert_eq!(
            parse_framework_hash(script).unwrap(),
            "23a97133-caf3-4eb4-9466-93d0a4ff8198"
        );
    }
}
