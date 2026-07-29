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

const BASE_URL: &str = "https://asianctv.cc";

#[derive(Default)]
pub struct AsiancTvCc;

impl AsiancTvCc {
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
        parse_catalog_page(&self.html(url)?)
    }

    fn filtered_listing(&self, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let kind = filter_value(filters, "type").unwrap_or("movie");
        let route = if kind == "tv" { "/tv-shows" } else { "/movies" };
        let sort = filter_value(filters, "sort").unwrap_or("popular");
        let sort = match (kind, sort) {
            ("movie", "latest") => "now_playing",
            ("tv", "latest") => "on_the_air",
            (_, value) => value,
        };
        let mut url = Url::parse(&format!("{BASE_URL}{route}")).map_err(url_error)?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("sort", sort);
            pairs.append_pair("page", &page.max(1).to_string());
            if let Some(genre) = filter_value(filters, "genre").filter(|value| !value.is_empty()) {
                pairs.append_pair("genre", genre);
            }
            if let Some(year) = filter_value(filters, "year").filter(|value| !value.is_empty()) {
                pairs.append_pair("year", year);
            }
        }
        self.listing_page(url.as_str())
    }

    fn player_html(&self, url: &str, referer: &str) -> Result<String> {
        validate_player_url(url)?;
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

    fn resolve_player(&self, player_url: &str, episode_url: &str) -> Result<(String, String)> {
        let mut current = player_url.to_string();
        let mut referer = episode_url.to_string();
        for _ in 0..5 {
            validate_player_url(&current)?;
            if is_protected_player_url(&current) {
                return Ok((current, referer));
            }
            let html = self.player_html(&current, &referer)?;
            let next = parse_nested_player_url(&html, &current)?;
            referer = current;
            current = next;
        }
        Err(Error::new(
            "AsiancTV (.cc) player chain did not reach the video host",
        ))
    }

    fn capture_player(&self, player_url: &str, referer_url: &str) -> Result<Vec<VideoStream>> {
        validate_player_url(player_url)?;
        let response: WebViewExtractResponse = browser::extract(&WebViewExtractRequest {
            url: player_url.to_string(),
            method: Default::default(),
            body: None,
            cookie_url: None,
            session: Some(WebViewSession {
                id: "asianctvcc-player".to_string(),
                persistence: WebViewSessionPersistence::Persistent,
                ..WebViewSession::default()
            }),
            headers: vec![("Referer".to_string(), referer_url.to_string())],
            user_agent: None,
            wait_until: Some(WebViewWaitUntil::DomReady),
            wait_for_selector: None,
            wait_for_event: None,
            wait_for_script: Some(
                r#"(() => {
                    const media = /\.m3u8(?:\?|$)|\.mpd(?:\?|$)|\.mp4(?:\?|$)/i;
                    const windows = [window];
                    for (const frame of document.querySelectorAll('iframe')) {
                        try { if (frame.contentWindow) windows.push(frame.contentWindow); } catch (_) {}
                    }
                    for (const target of windows) {
                        try {
                            target.document.querySelector('#pl_but, #pl_but_background, .vjs-big-play-button, button[aria-label="Play"]')?.click();
                            target.document.querySelector('video')?.play?.().catch(() => {});
                            const video = target.document.querySelector('video');
                            if (media.test(video?.currentSrc || video?.src || '')) return true;
                            if (target.performance.getEntriesByType('resource').some(entry => media.test(entry.name))) return true;
                        } catch (_) {}
                    }
                    return performance.now() >= 30000;
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
                        const source = video?.currentSrc || video?.src || '';
                        if (media.test(source)) urls.push(source);
                        for (const element of target.document.querySelectorAll('video source[src]')) {
                            if (media.test(element.src)) urls.push(element.src);
                        }
                        for (const entry of target.performance.getEntriesByType('resource')) {
                            if (media.test(entry.name)) urls.push(entry.name);
                        }
                        for (const frame of target.document.querySelectorAll('iframe')) {
                            try { if (frame.contentWindow) visit(frame.contentWindow, depth + 1); } catch (_) {}
                        }
                    } catch (_) {}
                };
                visit(window, 0);
                return {
                    url: urls[0] || '',
                    urls: [...new Set(urls)]
                };
            })()"#
            .to_string(),
            timeout_ms: Some(60_000),
            headless: Some(true),
            capture_requests: vec![
                capture(".m3u8"),
                capture(".mpd"),
                capture(".mp4"),
            ],
            capture_events: Vec::new(),
            cookies: false,
            preload_scripts: Vec::new(),
        })?;
        streams_from_capture(&response, player_url)
    }
}

impl VideoSource for AsiancTvCc {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing_page(&listing_url("/movies", "popular", page))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing_page(&listing_url("/movies", "now_playing", page))
    }

    fn listing(&mut self, listing: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        match listing {
            "popular" if filters_empty(filters) => self.popular(page),
            "latest" if filters_empty(filters) => self.latest(page),
            "tv" if filters_empty(filters) => {
                self.listing_page(&listing_url("/tv-shows", "popular", page))
            }
            "popular" | "latest" | "tv" => self.filtered_listing(page, filters),
            other => Err(Error::new(format!(
                "unknown AsiancTV (.cc) listing {other:?}"
            ))),
        }
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        if query.trim().is_empty() {
            return self.filtered_listing(page, filters);
        }
        if page > 1 {
            return Ok(Paged::new(Vec::new(), false));
        }
        let mut url = Url::parse(&format!("{BASE_URL}/search-suggest")).map_err(url_error)?;
        url.query_pairs_mut().append_pair("q", query.trim());
        let value: Value = Client::browser()
            .get(url.as_str())
            .header("Accept", "application/json")
            .header("Referer", format!("{BASE_URL}/"))
            .timeout_ms(30_000)
            .max_body_bytes(2 * 1024 * 1024)
            .send()?
            .error_for_status()?
            .json()?;
        parse_search_results(&value)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let (kind, id) = parse_item_key(&item.key)?;
        let url = preferred_item_url(&item, kind, id);
        parse_details(&self.html(&url)?, kind, id, &url)
    }

    fn episodes(&mut self, item: CatalogItem) -> Result<Vec<VideoEpisode>> {
        let (kind, id) = parse_item_key(&item.key)?;
        if kind == "movie" {
            return Ok(vec![movie_episode(id)]);
        }
        let details_url = preferred_item_url(&item, kind, id);
        let details_html = self.html(&details_url)?;
        let season_urls = parse_season_urls(&details_html, id)?;
        let mut episodes = Vec::new();
        let mut seen = BTreeSet::new();
        for url in season_urls {
            for episode in parse_tv_episodes(&self.html(&url)?, id)? {
                if seen.insert(episode.key.clone()) {
                    episodes.push(episode);
                }
            }
        }
        episodes.sort_by(|left, right| {
            left.season_number
                .partial_cmp(&right.season_number)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    left.episode_number
                        .partial_cmp(&right.episode_number)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        Ok(episodes)
    }

    fn streams(&mut self, item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoStream>> {
        let (kind, id) = parse_item_key(&item.key)?;
        validate_episode_key(&episode.key, kind, id)?;
        let episode_url = episode
            .url
            .as_deref()
            .filter(|url| validate_watch_url(url, kind, id).is_ok())
            .map(ToString::to_string)
            .unwrap_or_else(|| canonical_watch_url(kind, id, &episode));
        validate_watch_url(&episode_url, kind, id)?;
        let player_url = parse_player_url(&self.html(&episode_url)?)?;
        let (resolved_player, player_referer) = self.resolve_player(&player_url, &episode_url)?;
        self.capture_player(&resolved_player, &player_referer)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![
            select("type", "Type", &[("Movies", "movie"), ("TV Shows", "tv")]),
            select(
                "sort",
                "Sort",
                &[
                    ("Popular", "popular"),
                    ("Top Rated", "top_rated"),
                    ("Latest", "latest"),
                ],
            ),
            select(
                "genre",
                "Genre",
                &[
                    ("All", ""),
                    ("Action", "28"),
                    ("Adventure", "12"),
                    ("Animation", "16"),
                    ("Comedy", "35"),
                    ("Crime", "80"),
                    ("Documentary", "99"),
                    ("Drama", "18"),
                    ("Family", "10751"),
                    ("Fantasy", "14"),
                    ("History", "36"),
                    ("Horror", "27"),
                    ("Music", "10402"),
                    ("Mystery", "9648"),
                    ("Romance", "10749"),
                    ("Science Fiction", "878"),
                    ("Thriller", "53"),
                    ("War", "10752"),
                ],
            ),
            FilterDefinition::Text {
                id: "year".to_string(),
                name: "Year".to_string(),
                default: String::new(),
            },
        ])
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        let (kind, id) = parse_item_key(&item.key)?;
        Ok(Some(preferred_item_url(item, kind, id)))
    }

    fn episode_url(
        &mut self,
        item: &CatalogItem,
        episode: &VideoEpisode,
    ) -> Result<Option<String>> {
        let (kind, id) = parse_item_key(&item.key)?;
        validate_episode_key(&episode.key, kind, id)?;
        Ok(episode.url.clone())
    }
}

fn parse_catalog_page(html: &str) -> Result<Paged<CatalogItem>> {
    let document = Html::parse_document(html);
    let cards = selector("a.poster-card[href]")?;
    let image = selector("img")?;
    let title = selector(".card-title")?;
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    for card in document.select(&cards) {
        let Some(url) = absolute(card.value().attr("href").unwrap_or_default()) else {
            continue;
        };
        let Some((kind, id)) = item_from_url(&url) else {
            continue;
        };
        let key = format!("{kind}:{id}");
        if !seen.insert(key.clone()) {
            continue;
        }
        let title = card
            .select(&title)
            .next()
            .map(element_text)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("{kind} {id}"));
        items.push(CatalogItem {
            key,
            title,
            url: Some(url),
            cover: card.select(&image).next().and_then(image_from_element),
            language: Some("en".to_string()),
            content_rating: Some("suggestive".to_string()),
            ..CatalogItem::default()
        });
    }
    let next = selector("a[href*=\"page=\"]")?;
    let has_more = document
        .select(&next)
        .any(|element| element_text(element).to_ascii_lowercase().contains("next"));
    Ok(Paged::new(items, has_more))
}

fn parse_search_results(value: &Value) -> Result<Paged<CatalogItem>> {
    let results = value
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::new("AsiancTV (.cc) search returned no results array"))?;
    let items = results
        .iter()
        .filter_map(|entry| {
            let kind = entry.get("type")?.as_str()?;
            let id = entry.get("id")?.as_u64()?;
            if !matches!(kind, "movie" | "tv") {
                return None;
            }
            let title = clean(entry.get("title")?.as_str()?);
            let cover = entry
                .get("poster")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(image_request);
            Some(CatalogItem {
                key: format!("{kind}:{id}"),
                title,
                url: Some(format!("{BASE_URL}/{kind}/{id}")),
                cover,
                language: Some("en".to_string()),
                content_rating: Some("suggestive".to_string()),
                ..CatalogItem::default()
            })
        })
        .collect();
    Ok(Paged::new(items, false))
}

fn parse_details(html: &str, kind: &str, id: u64, url: &str) -> Result<CatalogItem> {
    let document = Html::parse_document(html);
    let root = document
        .select(&selector(".details")?)
        .next()
        .ok_or_else(|| Error::new("AsiancTV (.cc) details were not found"))?;
    let title = root
        .select(&selector("h1")?)
        .next()
        .map(element_text)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new("AsiancTV (.cc) title was not found"))?;
    let cover = document
        .select(&selector("img.details-poster")?)
        .next()
        .and_then(image_from_element);
    let description = root
        .select(&selector(".overview")?)
        .next()
        .map(element_text)
        .filter(|value| !value.is_empty());
    let facts: Vec<String> = root
        .select(&selector(".facts > div")?)
        .map(element_text)
        .collect();
    let tags = facts
        .iter()
        .find_map(|fact| fact.strip_prefix("Genre:"))
        .map(|genres| {
            genres
                .split(',')
                .map(clean)
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let rating = root
        .select(&selector(".chip.rating")?)
        .next()
        .and_then(|element| {
            Regex::new(r"(\d+(?:\.\d+)?)")
                .ok()?
                .captures(&element_text(element))?
                .get(1)?
                .as_str()
                .parse()
                .ok()
        });
    let banner = parse_banner(&document);
    Ok(CatalogItem {
        key: format!("{kind}:{id}"),
        title,
        url: Some(url.to_string()),
        cover,
        banner,
        description,
        tags,
        initialized: true,
        language: Some("en".to_string()),
        rating,
        content_rating: Some("suggestive".to_string()),
        ..CatalogItem::default()
    })
}

fn parse_banner(document: &Html) -> Option<ImageRequest> {
    let stage = document.select(&selector(".details-stage").ok()?).next()?;
    let style = stage.value().attr("style")?;
    let capture = Regex::new(r#"--hero-bg\s*:\s*url\(['"]?([^'")]+)"#)
        .ok()?
        .captures(style)?;
    Some(image_request(capture.get(1)?.as_str()))
}

fn parse_season_urls(html: &str, id: u64) -> Result<Vec<String>> {
    let document = Html::parse_document(html);
    let links = selector("a.season-card[href]")?;
    let mut urls = Vec::new();
    for link in document.select(&links) {
        let Some(url) = absolute(link.value().attr("href").unwrap_or_default()) else {
            continue;
        };
        if validate_watch_url(&url, "tv", id).is_ok() {
            urls.push(url);
        }
    }
    urls.sort();
    urls.dedup();
    if urls.is_empty() {
        return Err(Error::new("AsiancTV (.cc) returned no seasons"));
    }
    Ok(urls)
}

fn parse_tv_episodes(html: &str, id: u64) -> Result<Vec<VideoEpisode>> {
    let document = Html::parse_document(html);
    let links = selector("a.episode-card[href]")?;
    let number_selector = selector(".episode-num")?;
    let title_selector = selector("strong")?;
    let mut episodes = Vec::new();
    for link in document.select(&links) {
        let Some(url) = absolute(link.value().attr("href").unwrap_or_default()) else {
            continue;
        };
        let Some((item_id, season, episode)) = tv_episode_from_url(&url) else {
            continue;
        };
        if item_id != id {
            continue;
        }
        let title = link
            .select(&title_selector)
            .next()
            .map(element_text)
            .filter(|value| !value.is_empty())
            .or_else(|| link.select(&number_selector).next().map(element_text));
        episodes.push(VideoEpisode {
            key: format!("tv:{id}:{season}:{episode}"),
            title,
            season_number: Some(season as f32),
            episode_number: Some(episode as f32),
            url: Some(url),
            language: Some("en".to_string()),
            ..VideoEpisode::default()
        });
    }
    Ok(episodes)
}

fn movie_episode(id: u64) -> VideoEpisode {
    VideoEpisode {
        key: format!("movie:{id}"),
        title: Some("Movie".to_string()),
        season_number: Some(1.0),
        episode_number: Some(1.0),
        url: Some(format!("{BASE_URL}/watch/movie/{id}")),
        language: Some("en".to_string()),
        ..VideoEpisode::default()
    }
}

fn parse_player_url(html: &str) -> Result<String> {
    let document = Html::parse_document(html);
    for frame in document.select(&selector("iframe[src]")?) {
        let Some(url) = absolute(frame.value().attr("src").unwrap_or_default()) else {
            continue;
        };
        if validate_player_url(&url).is_ok() {
            return Ok(url);
        }
    }
    Err(Error::new(
        "AsiancTV (.cc) episode returned no supported video server",
    ))
}

fn parse_nested_player_url(html: &str, base_url: &str) -> Result<String> {
    validate_player_url(base_url)?;
    let base = Url::parse(base_url).map_err(url_error)?;
    let document = Html::parse_document(html);
    for frame in document.select(&selector("iframe[src]")?) {
        let Ok(url) = base.join(frame.value().attr("src").unwrap_or_default()) else {
            continue;
        };
        if validate_player_url(url.as_str()).is_ok() {
            return Ok(url.to_string());
        }
    }
    let lazy_player = Regex::new(r#"src\s*:\s*['"]([^'"]*/prorcp/[^'"]+)['"]"#)
        .map_err(|_| Error::new("invalid AsiancTV (.cc) player expression"))?;
    if let Some(value) = lazy_player
        .captures(html)
        .and_then(|captures| captures.get(1))
        .and_then(|value| base.join(value.as_str()).ok())
        .filter(|url| validate_player_url(url.as_str()).is_ok())
    {
        return Ok(value.to_string());
    }
    Err(Error::new(
        "AsiancTV (.cc) player returned no supported nested video host",
    ))
}

fn streams_from_capture(
    response: &WebViewExtractResponse,
    player_url: &str,
) -> Result<Vec<VideoStream>> {
    let mut urls: Vec<String> = response
        .captured_requests
        .iter()
        .map(|request| request.url.clone())
        .filter(|url| media_url(url))
        .collect();
    if let Some(url) = response
        .value
        .as_ref()
        .and_then(|value| value.get("url"))
        .and_then(Value::as_str)
        .filter(|url| media_url(url))
    {
        urls.push(url.to_string());
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
            "AsiancTV (.cc) player did not expose a playable stream",
        ));
    }
    let origin = player_origin(player_url)?;
    Ok(urls
        .into_iter()
        .map(|url| {
            let lower = url.to_ascii_lowercase();
            let is_hls = lower.contains(".m3u8");
            let is_dash = lower.contains(".mpd");
            VideoStream {
                url,
                name: Some("AsiancTV".to_string()),
                format: Some(
                    if is_dash {
                        "dash"
                    } else if is_hls {
                        "hls"
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
                    ("Referer".to_string(), player_url.to_string()),
                    ("Origin".to_string(), origin.clone()),
                ]
                .into_iter()
                .collect(),
                ..VideoStream::default()
            }
        })
        .collect())
}

fn listing_url(route: &str, sort: &str, page: u32) -> String {
    format!("{BASE_URL}{route}?sort={sort}&page={}", page.max(1))
}

fn filter_value<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).and_then(Value::as_str)
}

fn filters_empty(filters: &Value) -> bool {
    filters.as_object().is_none_or(|values| values.is_empty())
}

fn select(id: &str, name: &str, values: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Select {
        id: id.to_string(),
        name: name.to_string(),
        options: values
            .iter()
            .map(|(label, value)| OptionItem {
                label: (*label).to_string(),
                value: (*value).to_string(),
            })
            .collect(),
        default_index: 0,
    }
}

fn preferred_item_url(item: &CatalogItem, kind: &str, id: u64) -> String {
    item.url
        .as_deref()
        .filter(|url| item_from_url(url) == Some((kind, id)))
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("{BASE_URL}/{kind}/{id}"))
}

fn parse_item_key(value: &str) -> Result<(&str, u64)> {
    let (kind, id) = value
        .split_once(':')
        .ok_or_else(|| Error::new("invalid AsiancTV (.cc) item key"))?;
    if !matches!(kind, "movie" | "tv") {
        return Err(Error::new("invalid AsiancTV (.cc) item type"));
    }
    let id = id
        .parse()
        .map_err(|_| Error::new("invalid AsiancTV (.cc) item id"))?;
    Ok((kind, id))
}

fn validate_episode_key(value: &str, kind: &str, id: u64) -> Result<()> {
    let parts: Vec<_> = value.split(':').collect();
    let valid = match kind {
        "movie" => {
            parts.len() == 2 && parts[0] == "movie" && parts[1].parse::<u64>().ok() == Some(id)
        }
        "tv" => {
            parts.len() == 4
                && parts[0] == "tv"
                && parts[1].parse::<u64>().ok() == Some(id)
                && parts[2].parse::<u32>().is_ok()
                && parts[3].parse::<u32>().is_ok()
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::new(
            "episode does not belong to this AsiancTV (.cc) title",
        ))
    }
}

fn canonical_watch_url(kind: &str, id: u64, episode: &VideoEpisode) -> String {
    if kind == "movie" {
        format!("{BASE_URL}/watch/movie/{id}")
    } else {
        format!(
            "{BASE_URL}/watch/tv/{id}/{}/{}",
            episode.season_number.unwrap_or(1.0) as u32,
            episode.episode_number.unwrap_or(1.0) as u32
        )
    }
}

fn validate_watch_url(value: &str, kind: &str, id: u64) -> Result<()> {
    validate_site_url(value)?;
    let url = Url::parse(value).map_err(url_error)?;
    let parts: Vec<_> = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect();
    let valid = match kind {
        "movie" => {
            parts.len() >= 3
                && parts[0] == "watch"
                && parts[1] == "movie"
                && numeric_prefix(parts[2]) == Some(id)
        }
        "tv" => {
            parts.len() >= 5
                && parts[0] == "watch"
                && parts[1] == "tv"
                && numeric_prefix(parts[2]) == Some(id)
                && numeric_prefix(parts[3]).is_some()
                && numeric_prefix(parts[4]).is_some()
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::new("unexpected AsiancTV (.cc) watch URL"))
    }
}

fn item_from_url(value: &str) -> Option<(&'static str, u64)> {
    let url = Url::parse(value).ok()?;
    if url.scheme() != "https" || url.host_str()? != "asianctv.cc" {
        return None;
    }
    let parts: Vec<_> = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() < 2 || !matches!(parts[0], "movie" | "tv") {
        return None;
    }
    let kind = if parts[0] == "movie" { "movie" } else { "tv" };
    Some((kind, numeric_prefix(parts[1])?))
}

fn tv_episode_from_url(value: &str) -> Option<(u64, u32, u32)> {
    let url = Url::parse(value).ok()?;
    if url.scheme() != "https" || url.host_str()? != "asianctv.cc" {
        return None;
    }
    let parts: Vec<_> = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() < 5 || parts[0] != "watch" || parts[1] != "tv" {
        return None;
    }
    Some((
        numeric_prefix(parts[2])?,
        numeric_prefix(parts[3])? as u32,
        numeric_prefix(parts[4])? as u32,
    ))
}

fn numeric_prefix(value: &str) -> Option<u64> {
    value
        .split('-')
        .next()
        .filter(|value| !value.is_empty())?
        .parse()
        .ok()
}

fn validate_site_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    if url.scheme() != "https" || url.host_str() != Some("asianctv.cc") {
        return Err(Error::new("unexpected AsiancTV (.cc) URL"));
    }
    Ok(())
}

fn validate_player_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    let host = url.host_str().unwrap_or_default();
    let allowed = match host {
        "megaplay.lol" => url.path().starts_with("/movie/") || url.path().starts_with("/tv/"),
        "vidsrc.mov" | "vsembed.ru" => url.path().starts_with("/embed/"),
        "cloudorchestranova.com" => {
            url.path().starts_with("/rcp/") || url.path().starts_with("/prorcp/")
        }
        _ => false,
    };
    if url.scheme() != "https" || !allowed {
        return Err(Error::new("unexpected AsiancTV (.cc) player URL"));
    }
    Ok(())
}

fn is_protected_player_url(value: &str) -> bool {
    Url::parse(value).ok().is_some_and(|url| {
        url.host_str() == Some("cloudorchestranova.com") && url.path().starts_with("/prorcp/")
    })
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

fn image_from_element(element: ElementRef<'_>) -> Option<ImageRequest> {
    ["data-src", "data-original", "src"]
        .into_iter()
        .find_map(|attribute| element.value().attr(attribute))
        .and_then(absolute)
        .map(image_request)
}

fn image_request(value: impl Into<String>) -> ImageRequest {
    ImageRequest::get(value.into()).header("Referer", format!("{BASE_URL}/"))
}

fn absolute(value: &str) -> Option<String> {
    Url::parse(BASE_URL)
        .ok()?
        .join(value)
        .ok()
        .map(|url| url.to_string())
}

fn element_text(element: ElementRef<'_>) -> String {
    clean(&element.text().collect::<String>())
}

fn clean(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn selector(value: &str) -> Result<Selector> {
    Selector::parse(value).map_err(|_| Error::new(format!("invalid selector {value:?}")))
}

fn url_error(error: url::ParseError) -> Error {
    Error::new(format!("invalid URL: {error}"))
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(manatan_sdk::Extension::new().video("asianctvcc", AsiancTvCc));

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_mixed_catalog_and_search_results() {
        let html = r#"
          <a class="poster-card" href="/movie/1375646-test-movie">
            <img src="https://image.tmdb.org/t/p/w500/movie.jpg"><div class="card-title">Test Movie</div>
          </a>
          <a class="poster-card" href="/tv/296206-test-show">
            <img src="https://image.tmdb.org/t/p/w500/show.jpg"><div class="card-title">Test Show</div>
          </a>
          <a href="/movies?sort=popular&page=2">Next ›</a>
        "#;
        let page = parse_catalog_page(html).unwrap();
        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.entries[0].key, "movie:1375646");
        assert_eq!(page.entries[1].key, "tv:296206");
        assert!(page.has_next_page);

        let search = parse_search_results(&json!({
            "results": [{
                "id": 93405,
                "type": "tv",
                "title": "Squid Game",
                "poster": "https://image.tmdb.org/t/p/w154/test.jpg"
            }]
        }))
        .unwrap();
        assert_eq!(search.entries[0].key, "tv:93405");
        assert_eq!(search.entries[0].title, "Squid Game");
    }

    #[test]
    fn parses_details_seasons_and_episodes() {
        let details_html = r#"
          <div class="details-stage" style="--hero-bg:url('https://image.tmdb.org/t/p/original/banner.jpg')">
            <img class="details-poster" src="https://image.tmdb.org/t/p/w500/cover.jpg">
            <div class="details"><h1>Agent Kim Reactivated</h1>
              <div class="overview">A retired agent returns.</div>
              <span class="chip rating">IMDB: 7.8</span>
              <div class="facts"><div>Released: 2026</div><div>Genre: Action &amp; Adventure, Crime, Drama</div></div>
            </div>
          </div>
          <a class="season-card" href="/watch/tv/296206/1/1-agent-kim-reactivated">Season 1</a>
        "#;
        let details = parse_details(
            details_html,
            "tv",
            296206,
            "https://asianctv.cc/tv/296206-agent-kim-reactivated",
        )
        .unwrap();
        assert_eq!(details.title, "Agent Kim Reactivated");
        assert_eq!(details.tags, vec!["Action & Adventure", "Crime", "Drama"]);
        assert_eq!(details.rating, Some(7.8));
        assert!(details.banner.is_some());
        assert_eq!(parse_season_urls(details_html, 296206).unwrap().len(), 1);

        let episode_html = r#"
          <a class="episode-card" href="/watch/tv/296206/1/1-agent-kim-reactivated">
            <span class="episode-num">1</span><strong>We Meet Again</strong>
          </a>
          <a class="episode-card" href="/watch/tv/296206/1/2-agent-kim-reactivated">
            <span class="episode-num">2</span><strong>The Target</strong>
          </a>
        "#;
        let episodes = parse_tv_episodes(episode_html, 296206).unwrap();
        assert_eq!(episodes.len(), 2);
        assert_eq!(episodes[1].key, "tv:296206:1:2");
        assert_eq!(episodes[1].title.as_deref(), Some("The Target"));
    }

    #[test]
    fn validates_player_and_converts_captured_hls() {
        let page = r#"<iframe src="https://megaplay.lol/tv/296206/1/1"></iframe>"#;
        assert_eq!(
            parse_player_url(page).unwrap(),
            "https://megaplay.lol/tv/296206/1/1"
        );
        assert!(parse_player_url(r#"<iframe src="https://evil.example/video"></iframe>"#).is_err());

        assert_eq!(
            parse_nested_player_url(
                r#"<iframe src="https://vidsrc.mov/embed/movie/1630409"></iframe>"#,
                "https://megaplay.lol/movie/1630409",
            )
            .unwrap(),
            "https://vidsrc.mov/embed/movie/1630409"
        );
        assert_eq!(
            parse_nested_player_url(
                r#"<iframe src="//cloudorchestranova.com/rcp/token"></iframe>"#,
                "https://vsembed.ru/embed/movie/1630409/",
            )
            .unwrap(),
            "https://cloudorchestranova.com/rcp/token"
        );
        assert_eq!(
            parse_nested_player_url(
                r#"function loadIframe() { $('<iframe>', { src: '/prorcp/protected-token' }); }"#,
                "https://cloudorchestranova.com/rcp/token",
            )
            .unwrap(),
            "https://cloudorchestranova.com/prorcp/protected-token"
        );
        assert!(parse_nested_player_url(
            r#"<iframe src="https://evil.example/video"></iframe>"#,
            "https://megaplay.lol/movie/1630409",
        )
        .is_err());

        let response = WebViewExtractResponse {
            value: Some(json!({
                "url": "",
                "urls": ["https://cdn.example/video/master.m3u8"]
            })),
            ..WebViewExtractResponse::default()
        };
        let streams =
            streams_from_capture(&response, "https://cloudorchestranova.com/rcp/player-token")
                .unwrap();
        assert_eq!(streams.len(), 1);
        assert!(streams[0].is_hls);
        assert!(streams[0].requires_proxy);
        assert_eq!(
            streams[0].headers.get("Origin").map(String::as_str),
            Some("https://cloudorchestranova.com")
        );
    }

    #[test]
    fn rejects_cross_title_episode_urls_and_keys() {
        assert!(validate_episode_key("tv:296206:1:2", "tv", 296206).is_ok());
        assert!(validate_episode_key("tv:93405:1:2", "tv", 296206).is_err());
        assert!(validate_watch_url(
            "https://asianctv.cc/watch/tv/296206/1/2-agent-kim-reactivated",
            "tv",
            296206
        )
        .is_ok());
        assert!(validate_watch_url(
            "https://asianctv.cc/watch/tv/93405/1/2-squid-game",
            "tv",
            296206
        )
        .is_err());
    }
}
