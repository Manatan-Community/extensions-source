use std::{cmp::Ordering, collections::BTreeSet};

use manatan_sdk::{
    client::Client, CatalogItem, Error, ImageRequest, Paged, Result, VideoEpisode, VideoSource,
    VideoStream,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use url::Url;

const BASE_URL: &str = "https://animeav1.com";
const PLAYER_HOST: &str = "player.zilla-networks.com";
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
        if !candidates.is_empty() {
            return Err(Error::new(format!(
                "AnimeAv1 playback mirrors failed: {}",
                errors.join("; ")
            )));
        }
        let streams = parse_hls_streams(&html, &episode_url)?;
        if !streams.is_empty() {
            return Ok(streams);
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

fn parse_hls_streams(html: &str, episode_url: &str) -> Result<Vec<VideoStream>> {
    let embeds = html
        .split("embeds:")
        .nth(1)
        .and_then(|value| value.split(",downloads:").next())
        .ok_or_else(|| Error::new("AnimeAv1 episode embeds were not found"))?;
    let block = Regex::new(r#"(DUB|SUB):\[([^]]*)\]"#).map_err(regex_error)?;
    let hls = Regex::new(
        r#"\{server:\"HLS\",url:\"https://player\.zilla-networks\.com/play/([A-Za-z0-9_-]+)\"\}"#,
    )
    .map_err(regex_error)?;
    let mut streams = Vec::new();
    let mut seen = BTreeSet::new();
    for group in block.captures_iter(embeds) {
        let label = group.get(1).map(|value| value.as_str()).unwrap_or("ES");
        let body = group.get(2).map(|value| value.as_str()).unwrap_or_default();
        for capture in hls.captures_iter(body) {
            let Some(id) = capture.get(1).map(|value| value.as_str()) else {
                continue;
            };
            let url = format!("https://{PLAYER_HOST}/m3u8/{id}");
            if !seen.insert(url.clone()) {
                continue;
            }
            streams.push(VideoStream {
                url,
                name: Some(format!("AnimeAv1 {label}")),
                format: Some("hls".to_string()),
                is_hls: true,
                requires_proxy: true,
                preferred: label == "SUB",
                initialized: true,
                headers: [
                    ("Referer".to_string(), episode_url.to_string()),
                    ("Origin".to_string(), BASE_URL.to_string()),
                ]
                .into_iter()
                .collect(),
                ..VideoStream::default()
            });
        }
    }
    streams.sort_by_key(|stream| !stream.preferred);
    Ok(streams)
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
    fn parses_catalog_details_episodes_and_direct_hls() {
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
        let streams =
            parse_hls_streams(episode, "https://animeav1.com/media/shingeki-no-kyojin/1").unwrap();
        assert_eq!(streams.len(), 2);
        assert_eq!(
            streams[0].url,
            "https://player.zilla-networks.com/m3u8/sub123"
        );
        assert!(streams[0].preferred);
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
            validate_streamtape_media_url("https://streamtape.com/get_video?id=abc&token=def")
                .is_ok()
        );
    }
}
