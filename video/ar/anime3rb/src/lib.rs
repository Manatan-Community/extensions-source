use std::{cmp::Ordering, collections::BTreeSet};

use manatan_sdk::{
    browser::{
        self, WebViewCapturedRequest, WebViewExtractRequest, WebViewExtractResponse,
        WebViewRequestCapture, WebViewSession, WebViewSessionPersistence, WebViewWaitUntil,
    },
    client::Client,
    model::{
        CatalogItem, Headers, ImageRequest, Paged, UrlResolveResult, VideoEpisode, VideoStream,
    },
    Error, Result, VideoSource,
};
use scraper::{Html, Selector};
use serde_json::{json, Map, Value};
use url::Url;

const BASE: &str = "https://anime3rb.com";
const PROFILE: &str = "anime3rb-site";
const ANILIST_GRAPHQL: &str = "https://graphql.anilist.co";

#[derive(Default)]
pub struct Anime3rb {
    user_agent: Option<String>,
}

impl Anime3rb {
    fn browser_user_agent(&mut self) -> Result<String> {
        if let Some(user_agent) = self.user_agent.as_ref() {
            return Ok(user_agent.clone());
        }

        // A Cloudflare clearance is bound to the browser that solved the
        // challenge, including its user agent. Discover the host WebView's
        // real UA instead of pretending every platform is desktop Chrome.
        // The vid3rb origin is a lightweight, non-challenged page already
        // required for playback, so this works consistently on WKWebView,
        // Android WebView, and desktop engines.
        let response: WebViewExtractResponse = browser::extract(&WebViewExtractRequest {
            url: "https://video.vid3rb.com/".to_string(),
            cookie_url: None,
            session: Some(WebViewSession {
                id: "anime3rb-user-agent".to_string(),
                persistence: WebViewSessionPersistence::Ephemeral,
                ..WebViewSession::default()
            }),
            wait_until: Some(WebViewWaitUntil::DomReady),
            script: "navigator.userAgent".to_string(),
            timeout_ms: Some(15_000),
            cookies: false,
            headless: Some(true),
            method: Default::default(),
            body: None,
            headers: Vec::new(),
            user_agent: None,
            wait_for_script: None,
            wait_for_selector: None,
            wait_for_event: None,
            preload_scripts: Vec::new(),
            capture_requests: Vec::new(),
            capture_events: Vec::new(),
        })?;
        let user_agent = response
            .value
            .as_ref()
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::new("Anime3rb could not read the host browser user agent"))?
            .to_string();
        self.user_agent = Some(user_agent.clone());
        Ok(user_agent)
    }

    fn html(&mut self, url: &str) -> Result<String> {
        validate_site_url(url)?;
        let user_agent = self.browser_user_agent()?;
        let ready_script = page_ready_script(url)?;
        // Anime3rb binds its Cloudflare clearance to the browser's TLS
        // fingerprint as well as its cookie and user agent. Replaying the
        // clearance through the native HTTP stack is therefore rejected even
        // when every header is identical. Read protected pages through the
        // persistent host WebView so challenge solving and later requests use
        // the same browser network stack on every platform.
        let response: WebViewExtractResponse = browser::extract(&WebViewExtractRequest {
            url: url.to_string(),
            cookie_url: Some(BASE.to_string()),
            session: Some(persistent_session()),
            wait_until: Some(WebViewWaitUntil::LoadFinished),
            // Anime3rb renders its listings with Livewire after the document
            // load event. Wait for page-specific content rather than taking a
            // snapshot of the navigation shell and mistaking `/titles/list`
            // for a real title.
            wait_for_script: Some(ready_script),
            script: "document.documentElement.outerHTML".to_string(),
            timeout_ms: Some(30_000),
            cookies: true,
            headless: Some(true),
            method: Default::default(),
            body: None,
            headers: vec![("Referer".to_string(), format!("{BASE}/"))],
            user_agent: Some(user_agent),
            wait_for_selector: None,
            wait_for_event: None,
            preload_scripts: Vec::new(),
            capture_requests: Vec::new(),
            capture_events: Vec::new(),
        })?;
        let html = response
            .value
            .as_ref()
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("Anime3rb did not return a readable page"))?
            .to_string();
        if is_challenge(&html) {
            return Err(cloudflare_error());
        }
        Ok(html)
    }

    fn listing_page(&mut self, url: &str) -> Result<Paged<CatalogItem>> {
        let html = self.html(url)?;
        let mut page = parse_catalog(&html, self.user_agent.as_deref())?;
        self.enrich_covers(&mut page.entries);
        Ok(page)
    }

    fn enrich_covers(&self, items: &mut [CatalogItem]) {
        if items.is_empty() {
            return;
        }

        let mut query = String::from("query(");
        let mut variables = Map::new();
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                query.push(',');
            }
            query.push_str(&format!("$q{index}:String"));
            variables.insert(format!("q{index}"), json!(item.title));
        }
        query.push_str("){\n");
        for index in 0..items.len() {
            query.push_str(&format!(
                "m{index}:Media(search:$q{index},type:ANIME){{coverImage{{extraLarge large medium}}}}\n"
            ));
        }
        query.push('}');

        let response = Client::browser()
            .post(ANILIST_GRAPHQL)
            .header("Accept", "application/json")
            .rate_limit("anime3rb-anilist", 500)
            .timeout_ms(15_000)
            .max_body_bytes(2 * 1024 * 1024)
            .json(&json!({ "query": query, "variables": variables }))
            .and_then(|request| request.send())
            .and_then(|response| response.error_for_status())
            .and_then(|response| response.json::<Value>());
        let Ok(response) = response else {
            return;
        };
        apply_anilist_covers(items, &response);
    }

    fn capture_streams(&mut self, episode_url: &str) -> Result<Vec<VideoStream>> {
        validate_episode_url(episode_url)?;
        let episode_html = self.html(episode_url)?;
        let player_url = player_url_from_episode_html(&episode_html)?;
        let response: WebViewExtractResponse = browser::extract(&WebViewExtractRequest {
            url: player_url,
            cookie_url: None,
            session: Some(persistent_session()),
            wait_until: Some(WebViewWaitUntil::DomReady),
            wait_for_script: Some(
                r#"(() => {
                    const media = /\.m3u8(?:\?|$)|\.mp4(?:\?|$)/i;
                    document.querySelectorAll('video').forEach(video => video.play?.().catch(() => {}));
                    return !!document.querySelector('video[src], video source[src]') ||
                        performance.getEntriesByType('resource').some(entry => media.test(entry.name));
                })()"#
                    .to_string(),
            ),
            script: r#"(() => {
                const media = /\.m3u8(?:\?|$)|\.mp4(?:\?|$)/i;
                const isMedia = value => {
                    try {
                        const url = new URL(value, location.href);
                        return media.test(url.href) ||
                            (url.hostname === 'video.vid3rb.com' && url.pathname.startsWith('/video/'));
                    } catch (_) {
                        return false;
                    }
                };
                const urls = performance.getEntriesByType('resource').map(entry => entry.name).filter(isMedia);
                document.querySelectorAll('video, video source').forEach(node => {
                    const url = node.currentSrc || node.src || '';
                    if (isMedia(url)) urls.push(url);
                });
                return { urls: [...new Set(urls)] };
            })()"#
                .to_string(),
            timeout_ms: Some(60_000),
            cookies: false,
            // Reuse the source-scoped session and capture the site's current
            // player/CDN request instead of baking in rotating media hosts.
            headless: Some(true),
            capture_requests: vec![capture("/video/"), capture(".m3u8"), capture(".mp4")],
            method: Default::default(),
            body: None,
            headers: Vec::new(),
            user_agent: None,
            wait_for_selector: None,
            wait_for_event: None,
            preload_scripts: Vec::new(),
            capture_events: Vec::new(),
        })?;
        streams_from_capture(&response)
    }
}

fn page_ready_script(value: &str) -> Result<String> {
    let url = Url::parse(value).map_err(url_error)?;
    let path = url.path();
    let script = if path.starts_with("/episode/") {
        r#"!document.title.includes('Just a moment') && Array.from(document.querySelectorAll('iframe[src]')).some(frame => { try { const url = new URL(frame.src, location.href); return url.hostname === 'video.vid3rb.com' && url.pathname.startsWith('/player/'); } catch (_) { return false; } })"#
    } else if item_key(path).is_some() && path != "/titles/list" {
        r#"!document.title.includes('Just a moment') && !!document.querySelector('h1') && Array.from(document.querySelectorAll("a[href*='/episode/']")).some(link => { try { return new URL(link.href, location.href).pathname.startsWith('/episode/'); } catch (_) { return false; } })"#
    } else {
        r#"!document.title.includes('Just a moment') && Array.from(document.querySelectorAll("a[href*='/titles/']")).some(link => { try { const parts = new URL(link.href, location.href).pathname.split('/').filter(Boolean); return parts.length === 2 && parts[0] === 'titles' && parts[1] !== 'list'; } catch (_) { return false; } })"#
    };
    Ok(script.to_string())
}

impl VideoSource for Anime3rb {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing_page(&paged_url("/titles/list", page, None)?)
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        if page.max(1) == 1 {
            let mut result = self.listing_page(&format!("{BASE}/"))?;
            result.has_next_page = false;
            Ok(result)
        } else {
            Ok(Paged::default())
        }
    }

    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        let query = query.trim();
        if query.is_empty() {
            return self.popular(page);
        }
        self.listing_page(&paged_url("/search", page, Some(query))?)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = item_url_from_key(&item.key)?;
        let html = self.html(&url)?;
        let mut details = parse_details(&html, &item.key, &url, self.user_agent.as_deref())?;
        self.enrich_covers(std::slice::from_mut(&mut details));
        Ok(details)
    }

    fn episodes(&mut self, item: CatalogItem) -> Result<Vec<VideoEpisode>> {
        let url = item_url_from_key(&item.key)?;
        let html = self.html(&url)?;
        parse_episodes(&html, &item.key, self.user_agent.as_deref())
    }

    fn streams(&mut self, item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoStream>> {
        let item_slug = item_slug_from_key(&item.key)?;
        let episode_url = episode_url_from_key(&episode.key)?;
        let episode_slug = episode_slug_from_key(&episode.key)?;
        if item_slug != episode_slug {
            return Err(Error::new("episode does not belong to this Anime3rb title"));
        }
        self.capture_streams(&episode_url)
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(item_url_from_key(&item.key)?))
    }

    fn episode_url(
        &mut self,
        _item: &CatalogItem,
        episode: &VideoEpisode,
    ) -> Result<Option<String>> {
        Ok(Some(episode_url_from_key(&episode.key)?))
    }

    fn handle_url(&mut self, value: &str) -> Result<Option<UrlResolveResult>> {
        let url = Url::parse(value).map_err(url_error)?;
        if url.scheme() != "https" || url.host_str() != Some("anime3rb.com") {
            return Ok(None);
        }
        if let Some(key) = item_key(url.path()) {
            return Ok(Some(UrlResolveResult {
                item: Some(CatalogItem::new(key, "Anime3rb")),
                ..UrlResolveResult::default()
            }));
        }
        Ok(None)
    }
}

fn persistent_session() -> WebViewSession {
    WebViewSession {
        id: PROFILE.to_string(),
        persistence: WebViewSessionPersistence::Persistent,
        ..WebViewSession::default()
    }
}

fn is_challenge(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("just a moment")
        || lower.contains("challenge-platform")
        || lower.contains("cf-chl-")
}

fn cloudflare_error() -> Error {
    Error::new(
        "Failed to bypass Cloudflare: open Web View, complete Anime3rb's verification, close it, then retry",
    )
}

fn parse_catalog(html: &str, user_agent: Option<&str>) -> Result<Paged<CatalogItem>> {
    let document = Html::parse_document(html);
    let links = selector("a[href*='/titles/']")?;
    let image = selector("img")?;
    let title_node = selector("h2.title-name, h2, h3")?;
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    for link in document.select(&links) {
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let Some(key) = absolute_site_url(href)
            .and_then(|url| Url::parse(&url).ok().and_then(|url| item_key(url.path())))
        else {
            continue;
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        let title = link
            .select(&title_node)
            .next()
            .map(|node| clean_title(&node.text().collect::<String>()))
            .filter(|value| !value.is_empty())
            .or_else(|| {
                link.select(&image)
                    .next()
                    .and_then(|node| node.value().attr("alt"))
                    .map(clean_title)
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| clean_title(&link.text().collect::<String>()));
        if title.is_empty() {
            continue;
        }
        let cover = link
            .select(&image)
            .next()
            .and_then(|node| image_request(node.value().attr("src"), user_agent));
        entries.push(CatalogItem {
            key,
            title,
            url: absolute_site_url(href),
            cover,
            language: Some("ar".to_string()),
            content_rating: Some("suggestive".to_string()),
            ..CatalogItem::default()
        });
    }
    if entries.is_empty() {
        return Err(Error::new(
            "Anime3rb returned an unsupported page instead of its title catalog",
        ));
    }
    let next = selector("button[wire\\:click^='nextPage']:not([disabled]), a[rel='next']")?;
    let has_next = document.select(&next).next().is_some() || entries.len() >= 20;
    Ok(Paged::new(entries, has_next))
}

fn apply_anilist_covers(items: &mut [CatalogItem], response: &Value) {
    let Some(data) = response.get("data").and_then(Value::as_object) else {
        return;
    };
    for (index, item) in items.iter_mut().enumerate() {
        let Some(cover) = data
            .get(&format!("m{index}"))
            .and_then(|media| media.get("coverImage"))
            .and_then(|cover| {
                ["extraLarge", "large", "medium"]
                    .into_iter()
                    .find_map(|field| cover.get(field).and_then(Value::as_str))
            })
            .and_then(anilist_cover_request)
        else {
            continue;
        };
        item.cover = Some(cover);
    }
}

fn anilist_cover_request(value: &str) -> Option<ImageRequest> {
    let url = Url::parse(value).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("s4.anilist.co") {
        return None;
    }
    Some(ImageRequest::get(url.to_string()))
}

fn parse_details(
    html: &str,
    key: &str,
    url: &str,
    user_agent: Option<&str>,
) -> Result<CatalogItem> {
    let document = Html::parse_document(html);
    let title = clean_title(
        &required_text(&document, "h1 span[dir='ltr']")
            .or_else(|_| required_text(&document, "h1"))?,
    );
    let cover = meta_content(
        &document,
        "meta[property='twitter:image'], meta[property='og:image']",
    )
    .and_then(|value| image_request(Some(&value), user_agent));
    let description = meta_content(
        &document,
        "meta[name='description'], meta[property='og:description']",
    )
    .filter(|value| !value.is_empty());
    let genre = selector("a[href*='/genre/']")?;
    let tags = document
        .select(&genre)
        .map(|node| clean(&node.text().collect::<String>()))
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(CatalogItem {
        key: key.to_string(),
        title,
        url: Some(url.to_string()),
        cover,
        description,
        tags,
        initialized: true,
        language: Some("ar".to_string()),
        content_rating: Some("suggestive".to_string()),
        ..CatalogItem::default()
    })
}

fn parse_episodes(
    html: &str,
    item_key: &str,
    user_agent: Option<&str>,
) -> Result<Vec<VideoEpisode>> {
    let expected_slug = item_slug_from_key(item_key)?;
    let document = Html::parse_document(html);
    let links = selector("a[href*='/episode/']")?;
    let title_node = selector(".video-data p, p")?;
    let image = selector("img")?;
    let mut episodes = Vec::new();
    let mut seen = BTreeSet::new();
    for link in document.select(&links) {
        let Some(url) = link.value().attr("href").and_then(absolute_site_url) else {
            continue;
        };
        let Some(key) = Url::parse(&url)
            .ok()
            .and_then(|url| episode_key(url.path()))
        else {
            continue;
        };
        if episode_slug_from_key(&key).ok().as_deref() != Some(expected_slug.as_str())
            || !seen.insert(key.clone())
        {
            continue;
        }
        let number = episode_number_from_key(&key);
        let subtitle = link
            .select(&title_node)
            .next()
            .map(|node| clean(&node.text().collect::<String>()))
            .filter(|value| !value.is_empty());
        let title = subtitle.or_else(|| number.map(|number| format!("الحلقة {number}")));
        episodes.push(VideoEpisode {
            key,
            title,
            episode_number: number,
            thumbnail: link
                .select(&image)
                .next()
                .and_then(|node| image_request(node.value().attr("src"), user_agent)),
            url: Some(url),
            language: Some("ar".to_string()),
            ..VideoEpisode::default()
        });
    }
    if episodes.is_empty() {
        return Err(Error::new("Anime3rb returned no episodes for this title"));
    }
    episodes.sort_by(|left, right| {
        right
            .episode_number
            .partial_cmp(&left.episode_number)
            .unwrap_or(Ordering::Equal)
    });
    Ok(episodes)
}

fn player_url_from_episode_html(html: &str) -> Result<String> {
    let document = Html::parse_document(html);
    let iframe = selector("iframe[src]")?;
    let url = document
        .select(&iframe)
        .filter_map(|node| node.value().attr("src"))
        .find_map(|value| Url::parse(value).ok())
        .filter(|url| {
            url.scheme() == "https"
                && url.host_str() == Some("video.vid3rb.com")
                && url.path().starts_with("/player/")
        })
        .ok_or_else(|| Error::new("Anime3rb episode has no supported video player"))?;
    Ok(url.to_string())
}

fn streams_from_capture(response: &WebViewExtractResponse) -> Result<Vec<VideoStream>> {
    let mut candidates: Vec<(String, Option<&WebViewCapturedRequest>)> = response
        .captured_requests
        .iter()
        .filter(|request| media_url(&request.url))
        .map(|request| (request.url.clone(), Some(request)))
        .collect();
    if let Some(urls) = response
        .value
        .as_ref()
        .and_then(|value| value.get("urls"))
        .and_then(Value::as_array)
    {
        candidates.extend(
            urls.iter()
                .filter_map(Value::as_str)
                .filter(|url| media_url(url))
                .map(|url| (url.to_string(), None)),
        );
    }
    let mut seen = BTreeSet::new();
    candidates.retain(|(url, _)| seen.insert(url.clone()));
    if candidates.is_empty() {
        return Err(Error::new(
            "Anime3rb player did not expose a playable media stream",
        ));
    }
    let mut streams = Vec::new();
    for (index, (url, request)) in candidates.into_iter().enumerate() {
        validate_media_url(&url)?;
        let is_hls = url.to_ascii_lowercase().contains(".m3u8");
        let mut headers: Headers = request
            .map(|request| {
                request
                    .headers
                    .iter()
                    .filter(|(name, _)| {
                        matches!(
                            name.to_ascii_lowercase().as_str(),
                            "referer" | "origin" | "user-agent"
                        )
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        headers
            .entry("Referer".to_string())
            .or_insert_with(|| "https://video.vid3rb.com/".to_string());
        streams.push(VideoStream {
            url,
            name: Some(
                if is_hls {
                    "Anime3rb HLS"
                } else {
                    "Anime3rb MP4"
                }
                .to_string(),
            ),
            format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
            is_hls,
            requires_proxy: true,
            preferred: index == 0,
            initialized: true,
            headers,
            ..VideoStream::default()
        });
    }
    Ok(streams)
}

fn paged_url(path: &str, page: u32, query: Option<&str>) -> Result<String> {
    let mut url = Url::parse(BASE)
        .map_err(url_error)?
        .join(path)
        .map_err(url_error)?;
    {
        let mut pairs = url.query_pairs_mut();
        if let Some(query) = query {
            pairs.append_pair("q", query);
        }
        if page.max(1) > 1 {
            pairs.append_pair("page", &page.max(1).to_string());
        }
    }
    Ok(url.to_string())
}

fn item_key(path: &str) -> Option<String> {
    let segments: Vec<_> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() == 2
        && segments[0] == "titles"
        && segments[1] != "list"
        && valid_slug(segments[1])
    {
        Some(format!("/titles/{}", segments[1]))
    } else {
        None
    }
}

fn episode_key(path: &str) -> Option<String> {
    let segments: Vec<_> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() == 3
        && segments[0] == "episode"
        && valid_slug(segments[1])
        && segments[2]
            .parse::<f32>()
            .ok()
            .is_some_and(|number| number > 0.0)
    {
        Some(format!("/episode/{}/{}", segments[1], segments[2]))
    } else {
        None
    }
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 240
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn item_slug_from_key(key: &str) -> Result<String> {
    item_key(key)
        .and_then(|key| key.strip_prefix("/titles/").map(str::to_owned))
        .ok_or_else(|| Error::new("invalid Anime3rb title key"))
}

fn episode_slug_from_key(key: &str) -> Result<String> {
    episode_key(key)
        .and_then(|key| key.split('/').nth(2).map(str::to_owned))
        .ok_or_else(|| Error::new("invalid Anime3rb episode key"))
}

fn episode_number_from_key(key: &str) -> Option<f32> {
    episode_key(key)?.split('/').nth(3)?.parse().ok()
}

fn item_url_from_key(key: &str) -> Result<String> {
    item_key(key)
        .map(|key| format!("{BASE}{key}"))
        .ok_or_else(|| Error::new("invalid Anime3rb title key"))
}

fn episode_url_from_key(key: &str) -> Result<String> {
    episode_key(key)
        .map(|key| format!("{BASE}{key}"))
        .ok_or_else(|| Error::new("invalid Anime3rb episode key"))
}

fn absolute_site_url(value: &str) -> Option<String> {
    let url = Url::parse(BASE).ok()?.join(value).ok()?;
    validate_site_url(url.as_str()).ok()?;
    Some(url.to_string())
}

fn validate_site_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    if url.scheme() != "https" || url.host_str() != Some("anime3rb.com") {
        return Err(Error::new("unexpected Anime3rb URL"));
    }
    Ok(())
}

fn validate_episode_url(value: &str) -> Result<()> {
    validate_site_url(value)?;
    let url = Url::parse(value).map_err(url_error)?;
    episode_key(url.path()).ok_or_else(|| Error::new("invalid Anime3rb episode URL"))?;
    Ok(())
}

fn validate_media_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(url_error)?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https" || !(host == "vid3rb.com" || host.ends_with(".vid3rb.com")) {
        return Err(Error::new("Anime3rb returned an unexpected media URL"));
    }
    if !media_url(value) {
        return Err(Error::new("Anime3rb returned an unsupported media format"));
    }
    Ok(())
}

fn media_url(value: &str) -> bool {
    Url::parse(value).ok().is_some_and(|url| {
        let host = url.host_str().unwrap_or_default();
        let path = url.path().to_ascii_lowercase();
        url.scheme() == "https"
            && (host == "vid3rb.com" || host.ends_with(".vid3rb.com"))
            && (path.ends_with(".m3u8")
                || path.ends_with(".mp4")
                || (host == "video.vid3rb.com" && path.starts_with("/video/")))
    })
}

fn image_request(value: Option<&str>, user_agent: Option<&str>) -> Option<ImageRequest> {
    let url = value.and_then(|value| Url::parse(BASE).ok()?.join(value).ok())?;
    if url.scheme() != "https" {
        return None;
    }
    let host = url.host_str()?;
    if host != "anime3rb.com" && !host.ends_with(".anime3rb.com") {
        return None;
    }
    let url = url.to_string();
    let mut request = ImageRequest::get(url.clone()).header("Referer", format!("{BASE}/"));
    if let Some(user_agent) = user_agent.filter(|value| !value.trim().is_empty()) {
        request = request.header("User-Agent", user_agent.to_string());
    }
    Some(
        request
            // Artwork is served from a separate Cloudflare-protected origin.
            // Scope its browser clearance to that origin rather than the HTML
            // site's cookie jar so the host can recover challenged images.
            .cookies_for(url),
    )
}

fn capture(fragment: &str) -> WebViewRequestCapture {
    WebViewRequestCapture {
        url_contains: Some(fragment.to_string()),
        limit: Some(32),
        ..WebViewRequestCapture::default()
    }
}

fn selector(value: &str) -> Result<Selector> {
    Selector::parse(value).map_err(|_| Error::new(format!("invalid selector {value}")))
}

fn required_text(document: &Html, value: &str) -> Result<String> {
    document
        .select(&selector(value)?)
        .next()
        .map(|node| clean(&node.text().collect::<String>()))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new("Anime3rb title page has no title"))
}

fn meta_content(document: &Html, value: &str) -> Option<String> {
    document
        .select(&selector(value).ok()?)
        .find_map(|node| node.value().attr("content"))
        .map(str::to_owned)
}

fn clean(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clean_title(value: &str) -> String {
    let value = clean(value);
    value
        .strip_prefix("بوستر ")
        .or_else(|| value.strip_prefix("Poster "))
        .unwrap_or(&value)
        .trim()
        .to_string()
}

fn url_error(error: url::ParseError) -> Error {
    Error::new(format!("invalid URL: {error}"))
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(manatan_sdk::Extension::new().video("anime3rb", Anime3rb::default()));

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_catalog_cards() {
        let html = r#"
          <a href="/titles/list"><h2>قائمة الأنمي</h2></a>
          <a class="btn btn-md btn-plain w-full" href="/titles/one-piece">
            <img src="https://images.anime3rb.com/455312/cover.jpg" alt="بوستر One Piece">
            <h2 class="title-name">One Piece</h2>
          </a>
          <button wire:click="nextPage"></button>
        "#;
        let page = parse_catalog(html, Some("Anime3rb Test UA")).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].key, "/titles/one-piece");
        assert_eq!(page.entries[0].title, "One Piece");
        assert_eq!(
            page.entries[0]
                .cover
                .as_ref()
                .and_then(|request| request.cookie_url.as_deref()),
            Some("https://images.anime3rb.com/455312/cover.jpg")
        );
        assert_eq!(
            page.entries[0]
                .cover
                .as_ref()
                .and_then(|request| request.headers.get("User-Agent"))
                .map(String::as_str),
            Some("Anime3rb Test UA")
        );
        assert!(page.has_next_page);
    }

    #[test]
    fn removes_site_poster_labels_from_catalog_and_detail_titles() {
        let page = parse_catalog(
            r#"
              <a href="/titles/dark-machine-the-animation">
                <img src="https://images.anime3rb.com/dark-machine.jpg"
                     alt="بوستر Dark Machine: The Animation">
              </a>
            "#,
            None,
        )
        .unwrap();
        assert_eq!(page.entries[0].title, "Dark Machine: The Animation");

        let details = parse_details(
            r#"
              <h1>بوستر Dark Machine: The Animation</h1>
              <meta property="og:image" content="https://images.anime3rb.com/dark-machine.jpg">
            "#,
            "/titles/dark-machine-the-animation",
            "https://anime3rb.com/titles/dark-machine-the-animation",
            None,
        )
        .unwrap();
        assert_eq!(details.title, "Dark Machine: The Animation");
    }

    #[test]
    fn parses_title_and_newest_first_episodes() {
        let details = r#"
          <meta name="description" content="Pirate adventure">
          <meta property="twitter:image" content="https://images.anime3rb.com/455312/cover.jpg">
          <h1><span dir="ltr">One Piece</span><span>( مسلسل )</span></h1>
          <a href="/genre/action">أكشن</a>
        "#;
        let item = parse_details(
            details,
            "/titles/one-piece",
            "https://anime3rb.com/titles/one-piece",
            Some("Anime3rb Test UA"),
        )
        .unwrap();
        assert_eq!(item.title, "One Piece");
        assert_eq!(item.tags, vec!["أكشن"]);

        let episodes = r#"
          <a href="/episode/one-piece/1"><div class="video-data"><p>أنا لوفي!</p></div></a>
          <a href="https://anime3rb.com/episode/one-piece/2"><div class="video-data"><p>القرصان العظيم</p></div></a>
          <a href="/episode/another-title/9"><p>wrong show</p></a>
        "#;
        let episodes = parse_episodes(episodes, "/titles/one-piece", None).unwrap();
        assert_eq!(episodes.len(), 2);
        assert_eq!(episodes[0].episode_number, Some(2.0));
        assert_eq!(episodes[1].episode_number, Some(1.0));

        let player = player_url_from_episode_html(
            r#"<iframe src="https://video.vid3rb.com/player/abc?token=one"></iframe>"#,
        )
        .unwrap();
        assert_eq!(player, "https://video.vid3rb.com/player/abc?token=one");
    }

    #[test]
    fn rejects_challenge_and_untrusted_media_hosts() {
        assert!(is_challenge("<title>Just a moment...</title>"));
        assert!(validate_media_url("https://files-5.vid3rb.com/files/one/480p.mp4").is_ok());
        assert!(validate_media_url("https://video.vid3rb.com/video/one?token=abc").is_ok());
        assert!(validate_media_url("https://vid3rb.com.evil.test/video.mp4").is_err());
    }

    #[test]
    fn maps_real_player_capture_to_stream() {
        let response = WebViewExtractResponse {
            captured_requests: vec![WebViewCapturedRequest {
                url: "https://files-5.vid3rb.com/files/one/480p.mp4?token=abc".to_string(),
                method: "GET".to_string(),
                headers: vec![(
                    "Referer".to_string(),
                    "https://video.vid3rb.com/player/one".to_string(),
                )],
                ..WebViewCapturedRequest::default()
            }],
            value: Some(json!({"urls": []})),
            ..WebViewExtractResponse::default()
        };
        let streams = streams_from_capture(&response).unwrap();
        assert_eq!(streams.len(), 1);
        assert!(streams[0].requires_proxy);
        assert_eq!(streams[0].format.as_deref(), Some("mp4"));
    }

    #[test]
    fn replaces_protected_site_artwork_with_validated_anilist_covers() {
        let mut items = vec![CatalogItem {
            title: "DARK MACHINE: The Animation".to_string(),
            cover: image_request(
                Some("https://images.anime3rb.com/547106/160135.jpg"),
                Some("Anime3rb Test UA"),
            ),
            ..CatalogItem::default()
        }];
        apply_anilist_covers(
            &mut items,
            &json!({
                "data": {
                    "m0": {
                        "coverImage": {
                            "extraLarge": "https://s4.anilist.co/file/anilistcdn/media/anime/cover/large/example.jpg"
                        }
                    }
                }
            }),
        );

        let cover = items[0].cover.as_ref().unwrap();
        assert_eq!(
            cover.url,
            "https://s4.anilist.co/file/anilistcdn/media/anime/cover/large/example.jpg"
        );
        assert!(cover.cookie_url.is_none());

        apply_anilist_covers(
            &mut items,
            &json!({
                "data": {
                    "m0": {
                        "coverImage": {
                            "extraLarge": "https://s4.anilist.co.evil.test/cover.jpg"
                        }
                    }
                }
            }),
        );
        assert_eq!(
            items[0].cover.as_ref().unwrap().url,
            "https://s4.anilist.co/file/anilistcdn/media/anime/cover/large/example.jpg"
        );
    }
}
