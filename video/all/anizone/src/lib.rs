use std::collections::{BTreeMap, BTreeSet};

use manatan_sdk::{
    client::{Client, Response},
    context, CatalogItem, Error, FilterDefinition, ImageRequest, MediaTrack, OptionItem, Paged,
    PreferenceDefinition, Result, UrlResolveResult, VideoEpisode, VideoSource, VideoStream,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use url::Url;

const BASE_URL: &str = "https://anizone.to";
#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "anizone";
const LANG: &str = "all";
const RATE_LIMIT_MS: u32 = 340;
const MAX_HTML_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PLAYLIST_BYTES: u64 = 1024 * 1024;
const MAX_EPISODE_PAGES: u32 = 100;

const PREF_TITLE_LANGUAGE: &str = "preferred_title_language";
const PREF_QUALITY: &str = "preferred_quality";
const PREF_AUDIO: &str = "preferred_audio";
const PREF_SUBTITLE: &str = "preferred_subtitle";
const PREF_LOAD_ALL: &str = "load_all_tracks";
const PREF_SUBTITLE_COUNT: &str = "subtitle_count";

const BLOCKED_TERMS: &[&str] = &[
    "adult",
    "mature",
    "smut",
    "ecchi",
    "hentai",
    "erotic",
    "explicit",
    "porn",
    "pornographic",
    "sexual",
    "nudity",
    "r18",
    "18plus",
    "18",
];

#[derive(Default)]
pub struct AniZoneSource;

impl AniZoneSource {
    fn ensure_safe_search(&self) -> Result<()> {
        #[cfg(target_arch = "wasm32")]
        {
            let cookie = manatan_sdk::cookies::Cookie {
                name: "saf-sch".to_string(),
                value: "on".to_string(),
                domain: ".anizone.to".to_string(),
                path: Some("/".to_string()),
                secure: true,
                http_only: false,
                expires_at: None,
            };
            manatan_sdk::cookies::set(BASE_URL, &[cookie]).map_err(Error::new)?;
        }
        Ok(())
    }

    fn client(&self) -> Client {
        Client::browser()
            .cookies_for(BASE_URL)
            .header("Referer", format!("{BASE_URL}/"))
    }

    fn get_html(&self, url: &str) -> Result<String> {
        validate_site_url(url)?;
        self.ensure_safe_search()?;
        let response = self
            .client()
            .get(url)
            .rate_limit("anizone:site", RATE_LIMIT_MS)
            .timeout_ms(30_000)
            .max_body_bytes(MAX_HTML_BYTES)
            .send()?
            .error_for_status()?;
        Ok(response.text()?.to_string())
    }

    fn string_pref(&self, key: &str, default: &str) -> String {
        context::preference::<String>(key)
            .ok()
            .flatten()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| default.to_string())
    }

    fn title_key(&self) -> String {
        let value = self.string_pref(PREF_TITLE_LANGUAGE, "1");
        if matches!(value.as_str(), "1" | "5") {
            value
        } else {
            "1".to_string()
        }
    }

    fn load_all(&self) -> bool {
        context::preference::<bool>(PREF_LOAD_ALL)
            .ok()
            .flatten()
            .unwrap_or(false)
    }

    fn subtitle_count(&self) -> usize {
        context::preference::<i64>(PREF_SUBTITLE_COUNT)
            .ok()
            .flatten()
            .unwrap_or(2)
            .clamp(1, 20) as usize
    }

    fn fetch_safe_details(&self, slug: &str) -> Result<(CatalogItem, String)> {
        let slug = validate_slug(slug)?;
        let html = self.get_html(&format!("{BASE_URL}/anime/{slug}"))?;
        let item = parse_safe_details(&html, slug, &self.title_key())?;
        Ok((item, html))
    }

    fn catalog_page(&self, url: &str) -> Result<Paged<CatalogItem>> {
        let html = self.get_html(url)?;
        let parsed = parse_listing_page(&html, &self.title_key())?;
        self.ensure_safe_search()?;
        let requests = parsed
            .entries
            .iter()
            .map(|entry| {
                self.client()
                    .get(format!("{BASE_URL}/anime/{}", entry.slug))
                    .rate_limit("anizone:details", RATE_LIMIT_MS)
                    .timeout_ms(30_000)
                    .max_body_bytes(MAX_HTML_BYTES)
            })
            .collect();
        let responses = Client::send_many(requests, 3);
        let mut entries = Vec::new();
        for (listing, response) in parsed.entries.into_iter().zip(responses) {
            let Ok(response) = response.and_then(Response::error_for_status) else {
                continue;
            };
            let Ok(body) = response.text() else {
                continue;
            };
            let Ok(mut details) = parse_safe_details(body, &listing.slug, &self.title_key()) else {
                continue;
            };
            if details.cover.is_none() {
                details.cover = listing.cover;
            }
            entries.push(details);
        }
        Ok(Paged::new(entries, parsed.has_next_page))
    }

    fn safe_episodes(&self, item: &CatalogItem) -> Result<Vec<VideoEpisode>> {
        let slug = validate_slug(&item.key)?.to_string();
        let (_, first_html) = self.fetch_safe_details(&slug)?;
        let mut page = 1;
        let mut current = first_html;
        let mut episodes = Vec::new();
        let mut seen = BTreeSet::new();
        loop {
            let parsed = parse_episode_page(&current, &slug, &self.title_key())?;
            episodes.extend(
                parsed
                    .entries
                    .into_iter()
                    .filter(|episode| seen.insert(episode.key.clone())),
            );
            if !parsed.has_next_page || page >= MAX_EPISODE_PAGES {
                break;
            }
            page += 1;
            current = self.get_html(&format!("{BASE_URL}/anime/{slug}?page={page}"))?;
            parse_safe_details(&current, &slug, &self.title_key())?;
        }
        episodes.sort_by(|left, right| {
            right
                .episode_number
                .partial_cmp(&left.episode_number)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(episodes)
    }

    fn authorized_episode(
        &self,
        item: &CatalogItem,
        requested: &VideoEpisode,
    ) -> Result<VideoEpisode> {
        let expected_key = episode_key(&item.key, requested)?;
        self.safe_episodes(item)?
            .into_iter()
            .find(|episode| episode.key == expected_key)
            .ok_or_else(|| Error::new("AniZone episode is not present in the safe episode list"))
    }

    fn fetch_playlist(&self, url: &str, referer: &str) -> Result<String> {
        validate_media_url(url)?;
        let response = Client::browser()
            .get(url)
            .header("Referer", referer)
            .header("Origin", BASE_URL)
            .rate_limit("anizone:playlist", 150)
            .timeout_ms(30_000)
            .max_body_bytes(MAX_PLAYLIST_BYTES)
            .send()?
            .error_for_status()?;
        Ok(response.text()?.to_string())
    }

    fn livewire_video(
        &self,
        episode_url: &str,
        csrf: &str,
        snapshot: &str,
        server_id: u32,
    ) -> Result<LivewireVideo> {
        let payload = json!({
            "_token": csrf,
            "components": [{
                "snapshot": snapshot,
                "updates": {},
                "calls": [{"path": "", "method": "setVideo", "params": [server_id]}]
            }]
        });
        let response = self
            .client()
            .post(format!("{BASE_URL}/livewire/update"))
            .header("X-Livewire", "")
            .header("X-CSRF-TOKEN", csrf)
            .header("Origin", BASE_URL)
            .header("Referer", episode_url)
            .json(&payload)?
            .rate_limit("anizone:livewire", RATE_LIMIT_MS)
            .timeout_ms(30_000)
            .max_body_bytes(MAX_HTML_BYTES)
            .send()?
            .error_for_status()?;
        let response: LivewireResponse = response.json()?;
        let component = response
            .components
            .into_iter()
            .next()
            .ok_or_else(|| Error::new("AniZone Livewire response had no component"))?;
        Ok(LivewireVideo {
            snapshot: component.snapshot,
            html: component.effects.html,
        })
    }

    fn streams_for_source(
        &self,
        source: PlaybackSource,
        episode_url: &str,
        preferred_quality: &str,
        preferred_subtitle: &str,
        subtitle_count: usize,
        load_all: bool,
    ) -> Result<Vec<VideoStream>> {
        validate_media_url(&source.url)?;
        let mut subtitles = filter_subtitles(
            source.subtitles,
            preferred_subtitle,
            subtitle_count,
            load_all,
        );
        let parsed = self
            .fetch_playlist(&source.url, episode_url)
            .ok()
            .and_then(|body| parse_hls_master(&body, &source.url).ok())
            .unwrap_or_default();
        for track in parsed.subtitles {
            if !subtitles.iter().any(|entry| entry.url == track.url) {
                subtitles.push(track);
            }
        }
        subtitles = filter_subtitles(subtitles, preferred_subtitle, subtitle_count, load_all);
        let headers = media_headers(episode_url);
        if parsed.variants.is_empty() {
            return Ok(vec![VideoStream {
                url: source.url,
                name: Some(source.name),
                quality: Some("Auto".to_string()),
                format: Some("hls".to_string()),
                is_hls: true,
                preferred: true,
                initialized: true,
                headers,
                audio_tracks: parsed.audio_tracks,
                subtitles,
                ..VideoStream::default()
            }]);
        }
        parsed
            .variants
            .into_iter()
            .map(|variant| {
                validate_media_url(&variant.url)?;
                Ok(VideoStream {
                    url: variant.url,
                    name: Some(format!("{} - {}", source.name, variant.quality)),
                    preferred: variant.quality.contains(preferred_quality),
                    quality: Some(variant.quality),
                    resolution: variant.resolution,
                    bitrate: variant.bandwidth,
                    format: Some("hls".to_string()),
                    is_hls: true,
                    initialized: true,
                    headers: headers.clone(),
                    audio_tracks: parsed.audio_tracks.clone(),
                    subtitles: subtitles.clone(),
                    ..VideoStream::default()
                })
            })
            .collect()
    }
}

impl VideoSource for AniZoneSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.catalog_page(&anime_index_url("", "title-asc", page))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let mut url = Url::parse(&format!("{BASE_URL}/episode")).map_err(url_error)?;
        url.query_pairs_mut()
            .append_pair("sort", "release-desc")
            .append_pair("page", &page.max(1).to_string());
        self.catalog_page(url.as_str())
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let sort = filters
            .get("sort")
            .and_then(Value::as_str)
            .filter(|sort| is_allowed_sort(sort))
            .unwrap_or("title-asc");
        self.catalog_page(&anime_index_url(query.trim(), sort, page))
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        self.fetch_safe_details(&item.key).map(|(item, _)| item)
    }

    fn episodes(&mut self, item: CatalogItem) -> Result<Vec<VideoEpisode>> {
        self.safe_episodes(&item)
    }

    fn streams(&mut self, item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoStream>> {
        let episode = self.authorized_episode(&item, &episode)?;
        let episode_url = episode
            .url
            .clone()
            .ok_or_else(|| Error::new("AniZone episode had no canonical URL"))?;
        let html = self.get_html(&episode_url)?;
        reject_blocked_document(&html)?;
        let playback = parse_playback(&html)?;
        let preferred_audio = self.string_pref(PREF_AUDIO, "jpn");
        let preferred_quality = self.string_pref(PREF_QUALITY, "1080");
        let preferred_subtitle = self.string_pref(PREF_SUBTITLE, "eng");
        let load_all = self.load_all();
        let subtitle_count = self.subtitle_count();
        let selected = select_servers(&playback.servers, &preferred_audio, load_all);
        let mut snapshot = playback.snapshot.clone();
        let mut streams = Vec::new();
        for server in selected {
            let source = if server.id == playback.default_server_id {
                playback.default_source.clone()
            } else {
                let updated =
                    self.livewire_video(&episode_url, &playback.csrf, &snapshot, server.id)?;
                snapshot = updated.snapshot;
                parse_playback_fragment(&updated.html, &server.name)?
            };
            streams.extend(self.streams_for_source(
                source,
                &episode_url,
                &preferred_quality,
                &preferred_subtitle,
                subtitle_count,
                load_all,
            )?);
        }
        streams.sort_by_key(|stream| !stream.preferred);
        if streams.is_empty() {
            return Err(Error::new("AniZone returned no approved video streams"));
        }
        Ok(streams)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![FilterDefinition::Select {
            id: "sort".to_string(),
            name: "Sort".to_string(),
            options: [
                ("A-Z", "title-asc"),
                ("Z-A", "title-desc"),
                ("Earliest Release", "release-asc"),
                ("Latest Release", "release-desc"),
                ("First Added", "added-asc"),
                ("Last Added", "added-desc"),
            ]
            .into_iter()
            .map(|(label, value)| option(label, value))
            .collect(),
            default_index: 0,
        }])
    }

    fn preferences(&mut self) -> Result<Vec<PreferenceDefinition>> {
        Ok(preferences())
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(format!(
            "{BASE_URL}/anime/{}",
            validate_slug(&item.key)?
        )))
    }

    fn episode_url(
        &mut self,
        item: &CatalogItem,
        episode: &VideoEpisode,
    ) -> Result<Option<String>> {
        Ok(Some(format!(
            "{BASE_URL}/anime/{}",
            episode_key(&item.key, episode)?
        )))
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let Some((slug, episode_slug)) = parse_anizone_url(candidate)? else {
            return Ok(None);
        };
        let (item, _) = self.fetch_safe_details(&slug)?;
        if let Some(episode_slug) = episode_slug {
            let wanted = format!("{slug}/{episode_slug}");
            let episode = self
                .safe_episodes(&item)?
                .into_iter()
                .find(|episode| episode.key == wanted)
                .ok_or_else(|| {
                    Error::new("AniZone deep link targets an unsafe or absent episode")
                })?;
            return Ok(Some(UrlResolveResult {
                item: Some(item),
                episode_key: Some(episode.key.clone()),
                video_episode: Some(episode),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            item: Some(item),
            ..UrlResolveResult::default()
        }))
    }
}

#[derive(Clone)]
struct ListingEntry {
    slug: String,
    cover: Option<ImageRequest>,
}

struct ListingPage {
    entries: Vec<ListingEntry>,
    has_next_page: bool,
}

struct EpisodePage {
    entries: Vec<VideoEpisode>,
    has_next_page: bool,
}

#[derive(Clone)]
struct ServerChoice {
    id: u32,
    name: String,
}

struct Playback {
    csrf: String,
    snapshot: String,
    default_server_id: u32,
    default_source: PlaybackSource,
    servers: Vec<ServerChoice>,
}

#[derive(Clone)]
struct PlaybackSource {
    url: String,
    name: String,
    subtitles: Vec<MediaTrack>,
}

struct LivewireVideo {
    snapshot: String,
    html: String,
}

#[derive(Deserialize)]
struct LivewireResponse {
    components: Vec<LivewireComponent>,
}

#[derive(Deserialize)]
struct LivewireComponent {
    snapshot: String,
    effects: LivewireEffects,
}

#[derive(Deserialize)]
struct LivewireEffects {
    html: String,
}

#[derive(Default)]
struct HlsMaster {
    variants: Vec<HlsVariant>,
    audio_tracks: Vec<MediaTrack>,
    subtitles: Vec<MediaTrack>,
}

struct HlsVariant {
    url: String,
    quality: String,
    resolution: Option<String>,
    bandwidth: Option<u64>,
}

fn anime_index_url(query: &str, sort: &str, page: u32) -> String {
    let mut url = Url::parse(&format!("{BASE_URL}/anime")).expect("constant AniZone URL");
    let mut pairs = url.query_pairs_mut();
    if !query.is_empty() {
        pairs.append_pair("search", query);
    }
    pairs
        .append_pair("sort", sort)
        .append_pair("page", &page.max(1).to_string());
    drop(pairs);
    url.to_string()
}

fn is_allowed_sort(value: &str) -> bool {
    matches!(
        value,
        "title-asc" | "title-desc" | "release-asc" | "release-desc" | "added-asc" | "added-desc"
    )
}

fn parse_listing_page(html: &str, preferred_title_key: &str) -> Result<ListingPage> {
    let document = Html::parse_document(html);
    let card_selector = selector("div.grid > div, div.grid > li, ul.grid > li")?;
    let link_selector = selector("a[href*='/anime/']")?;
    let image_selector = selector("img[src]")?;
    let tag_selector = selector("a[href*='/tag/']")?;
    let root_dict = document
        .select(&selector("[x-data*='animeDict']")?)
        .find_map(|element| {
            element
                .value()
                .attr("x-data")
                .and_then(|data| extract_json_object(data, "animeDict"))
        })
        .unwrap_or_default();
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();

    for card in document.select(&card_selector) {
        let x_data = card.value().attr("x-data").unwrap_or_default();
        if unsafe_marker(x_data) {
            continue;
        }
        let Some((slug, _)) = card.select(&link_selector).find_map(|link| {
            link.value()
                .attr("href")
                .and_then(|href| parse_anizone_url(href).ok().flatten())
        }) else {
            continue;
        };
        if !seen.insert(slug.clone()) {
            continue;
        }
        let local_titles = extract_json_object(x_data, "anmTitles").unwrap_or_default();
        let dictionary_titles = root_dict
            .get(&slug)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let title = preferred_title(&local_titles, preferred_title_key)
            .or_else(|| preferred_title(&dictionary_titles, preferred_title_key))
            .unwrap_or_else(|| slug.clone());
        let tags = card
            .select(&tag_selector)
            .map(text)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if ensure_safe_text(&title, &tags, None).is_err() {
            continue;
        }
        let cover = card
            .select(&image_selector)
            .find_map(|image| image.value().attr("src"))
            .and_then(|url| image_request(url).ok());
        entries.push(ListingEntry { slug, cover });
    }
    Ok(ListingPage {
        entries,
        has_next_page: has_next_page(html),
    })
}

fn parse_safe_details(
    html: &str,
    expected_slug: &str,
    preferred_title_key: &str,
) -> Result<CatalogItem> {
    reject_blocked_document(html)?;
    let expected_slug = validate_slug(expected_slug)?;
    let document = Html::parse_document(html);
    let title_data = document
        .select(&selector("[x-data*='anmTitles']")?)
        .find_map(|element| {
            element
                .value()
                .attr("x-data")
                .and_then(|data| extract_json_object(data, "anmTitles"))
        })
        .ok_or_else(|| Error::new("AniZone details had no title metadata"))?;
    let title = preferred_title(&title_data, preferred_title_key)
        .ok_or_else(|| Error::new("AniZone details had no usable title"))?;
    let tags = document
        .select(&selector("a[href*='/tag/']")?)
        .map(text)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if tags.is_empty() {
        return Err(Error::new(
            "AniZone details had no classifiable tags; refusing fail-open content",
        ));
    }
    let description = synopsis(&document);
    ensure_safe_text(&title, &tags, description.as_deref())?;
    let cover = document
        .select(&selector(
            "div.flex.items-start img[src], img[alt*='Cover'][src]",
        )?)
        .find_map(|image| image.value().attr("src"))
        .map(image_request)
        .transpose()?;
    let status = document
        .select(&selector("span.inline-block")?)
        .map(text)
        .find(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "completed" | "ongoing" | "upcoming" | "cancelled"
            )
        });
    let mut item = CatalogItem::new(expected_slug, title);
    item.url = Some(format!("{BASE_URL}/anime/{expected_slug}"));
    item.cover = cover;
    item.description = description;
    item.tags = tags;
    item.status = status.map(Value::String);
    item.initialized = true;
    item.language = Some(LANG.to_string());
    item.content_rating = Some("suggestive".to_string());
    Ok(item)
}

fn parse_episode_page(
    html: &str,
    expected_slug: &str,
    preferred_title_key: &str,
) -> Result<EpisodePage> {
    let document = Html::parse_document(html);
    let item_selector = selector("ul.grid > li[x-data], div.grid > li[x-data]")?;
    let link_selector = selector("a[href*='/anime/']")?;
    let heading_selector = selector("h3")?;
    let image_selector = selector("img[src]")?;
    let mut entries = Vec::new();
    for element in document.select(&item_selector) {
        let x_data = element.value().attr("x-data").unwrap_or_default();
        if !x_data.contains("isUnsafe: false") || unsafe_marker(x_data) {
            continue;
        }
        let Some((slug, Some(episode_slug))) = element.select(&link_selector).find_map(|link| {
            link.value()
                .attr("href")
                .and_then(|href| parse_anizone_url(href).ok().flatten())
        }) else {
            continue;
        };
        if slug != expected_slug {
            continue;
        }
        let base_title = element
            .select(&heading_selector)
            .next()
            .map(text)
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| format!("Episode {episode_slug}"));
        let episode_titles = extract_json_object(x_data, "epsTitles").unwrap_or_default();
        let translated = preferred_title(&episode_titles, preferred_title_key);
        let title = translated
            .filter(|value| !base_title.contains(value))
            .map(|value| format!("{base_title} - {value}"))
            .unwrap_or(base_title);
        ensure_safe_text(&title, &[], None)?;
        let thumbnail = element
            .select(&image_selector)
            .find_map(|image| image.value().attr("src"))
            .map(image_request)
            .transpose()?;
        entries.push(VideoEpisode {
            key: format!("{slug}/{episode_slug}"),
            title: Some(title.clone()),
            episode_number: episode_number(&title),
            thumbnail,
            url: Some(format!("{BASE_URL}/anime/{slug}/{episode_slug}")),
            language: Some(LANG.to_string()),
            ..VideoEpisode::default()
        });
    }
    Ok(EpisodePage {
        entries,
        has_next_page: has_next_page(html),
    })
}

fn parse_playback(html: &str) -> Result<Playback> {
    reject_blocked_document(html)?;
    let document = Html::parse_document(html);
    let csrf = document
        .select(&selector("meta[name='csrf-token'][content]")?)
        .next()
        .and_then(|element| element.value().attr("content"))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| Error::new("AniZone playback page had no CSRF token"))?
        .to_string();
    let player_selector = selector("media-player")?;
    let snapshot = document
        .select(&selector("[wire\\:snapshot]")?)
        .find(|element| element.select(&player_selector).next().is_some())
        .and_then(|element| element.value().attr("wire:snapshot"))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| Error::new("AniZone playback page had no Livewire snapshot"))?
        .to_string();
    let servers = parse_servers(&document)?;
    let default_server_id = servers.first().map(|server| server.id).unwrap_or(0);
    let default_name = servers
        .first()
        .map(|server| server.name.as_str())
        .unwrap_or("Default");
    let default_source = parse_playback_document(&document, default_name)?;
    Ok(Playback {
        csrf,
        snapshot,
        default_server_id,
        default_source,
        servers: if servers.is_empty() {
            vec![ServerChoice {
                id: 0,
                name: "Default".to_string(),
            }]
        } else {
            servers
        },
    })
}

fn parse_playback_fragment(html: &str, name: &str) -> Result<PlaybackSource> {
    reject_blocked_document(html)?;
    parse_playback_document(&Html::parse_fragment(html), name)
}

fn parse_playback_document(document: &Html, name: &str) -> Result<PlaybackSource> {
    let player = document
        .select(&selector("media-player[src]")?)
        .next()
        .ok_or_else(|| Error::new("AniZone playback response had no media player"))?;
    let url = player
        .value()
        .attr("src")
        .ok_or_else(|| Error::new("AniZone media player had no source"))?
        .to_string();
    validate_media_url(&url)?;
    let subtitles = player
        .select(&selector("track[kind='subtitles'][src]")?)
        .map(|track| track_from_element(track, &url))
        .collect::<Result<Vec<_>>>()?;
    Ok(PlaybackSource {
        url,
        name: name.to_string(),
        subtitles,
    })
}

fn parse_servers(document: &Html) -> Result<Vec<ServerChoice>> {
    let mut servers = Vec::new();
    for button in document.select(&selector("button[wire\\:click*='setVideo']")?) {
        let Some(call) = button.value().attr("wire:click") else {
            continue;
        };
        let Some(id) = set_video_id(call) else {
            continue;
        };
        let name = text(button);
        servers.push(ServerChoice {
            id,
            name: if name.is_empty() {
                format!("Server {id}")
            } else {
                name
            },
        });
    }
    servers.sort_by_key(|server| server.id);
    servers.dedup_by_key(|server| server.id);
    Ok(servers)
}

fn select_servers(
    servers: &[ServerChoice],
    preferred_audio: &str,
    load_all: bool,
) -> Vec<ServerChoice> {
    if load_all {
        return servers.to_vec();
    }
    let preferred_label = language_label(preferred_audio);
    let fallback_label = language_label("jpn");
    servers
        .iter()
        .find(|server| contains_language(&server.name, preferred_audio, preferred_label))
        .or_else(|| {
            servers
                .iter()
                .find(|server| contains_language(&server.name, "jpn", fallback_label))
        })
        .or_else(|| servers.first())
        .cloned()
        .into_iter()
        .collect()
}

fn filter_subtitles(
    tracks: Vec<MediaTrack>,
    preferred: &str,
    count: usize,
    load_all: bool,
) -> Vec<MediaTrack> {
    if load_all {
        return deduplicate_tracks(tracks);
    }
    let preferred_label = language_label(preferred);
    let fallback_label = language_label("eng");
    let mut chosen = tracks
        .iter()
        .filter(|track| {
            let value = track
                .language
                .as_deref()
                .or(track.label.as_deref())
                .unwrap_or_default();
            contains_language(value, preferred, preferred_label)
        })
        .cloned()
        .collect::<Vec<_>>();
    chosen.extend(
        tracks
            .iter()
            .filter(|track| {
                let value = track
                    .language
                    .as_deref()
                    .or(track.label.as_deref())
                    .unwrap_or_default();
                contains_language(value, "eng", fallback_label)
            })
            .cloned(),
    );
    chosen.extend(tracks);
    deduplicate_tracks(chosen).into_iter().take(count).collect()
}

fn deduplicate_tracks(tracks: Vec<MediaTrack>) -> Vec<MediaTrack> {
    let mut seen = BTreeSet::new();
    tracks
        .into_iter()
        .filter(|track| seen.insert(track.url.clone()))
        .collect()
}

fn parse_hls_master(source: &str, playlist_url: &str) -> Result<HlsMaster> {
    if !source.lines().any(|line| line.trim() == "#EXTM3U") {
        return Err(Error::new("AniZone stream was not an HLS playlist"));
    }
    let mut result = HlsMaster::default();
    let mut pending = None::<(String, Option<String>, Option<u64>)>;
    for raw in source.lines() {
        let line = raw.trim();
        if let Some(attributes) = line.strip_prefix("#EXT-X-MEDIA:") {
            let kind = hls_attribute(attributes, "TYPE").unwrap_or_default();
            let Some(uri) = hls_attribute(attributes, "URI") else {
                continue;
            };
            let url = absolute_media_url(playlist_url, &uri)?;
            validate_media_url(&url)?;
            let track = MediaTrack {
                url,
                language: hls_attribute(attributes, "LANGUAGE"),
                label: hls_attribute(attributes, "NAME"),
                format: media_format(&uri),
                headers: media_headers(BASE_URL),
                is_default: hls_attribute(attributes, "DEFAULT")
                    .is_some_and(|value| value.eq_ignore_ascii_case("YES")),
                ..MediaTrack::default()
            };
            if kind.eq_ignore_ascii_case("AUDIO") {
                result.audio_tracks.push(track);
            } else if kind.eq_ignore_ascii_case("SUBTITLES") {
                result.subtitles.push(track);
            }
            continue;
        }
        if let Some(attributes) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let resolution = hls_attribute(attributes, "RESOLUTION");
            let quality = resolution
                .as_deref()
                .and_then(|resolution| resolution.split_once('x'))
                .map(|(_, height)| format!("{height}p"))
                .or_else(|| hls_attribute(attributes, "NAME"))
                .unwrap_or_else(|| "Auto".to_string());
            let bandwidth =
                hls_attribute(attributes, "BANDWIDTH").and_then(|value| value.parse::<u64>().ok());
            pending = Some((quality, resolution, bandwidth));
            continue;
        }
        if !line.is_empty() && !line.starts_with('#') {
            if let Some((quality, resolution, bandwidth)) = pending.take() {
                let url = absolute_media_url(playlist_url, line)?;
                validate_media_url(&url)?;
                result.variants.push(HlsVariant {
                    url,
                    quality,
                    resolution,
                    bandwidth,
                });
            }
        }
    }
    result.audio_tracks = deduplicate_tracks(result.audio_tracks);
    result.subtitles = deduplicate_tracks(result.subtitles);
    Ok(result)
}

fn hls_attribute(attributes: &str, key: &str) -> Option<String> {
    let marker = format!("{key}=");
    let mut quoted = false;
    let mut start = 0;
    for (index, character) in attributes.char_indices() {
        if character == '"' {
            quoted = !quoted;
        } else if character == ',' && !quoted {
            let field = &attributes[start..index];
            if let Some(value) = field.trim().strip_prefix(&marker) {
                return Some(value.trim_matches('"').to_string());
            }
            start = index + 1;
        }
    }
    attributes[start..]
        .trim()
        .strip_prefix(&marker)
        .map(|value| value.trim_matches('"').to_string())
}

fn track_from_element(track: ElementRef<'_>, base: &str) -> Result<MediaTrack> {
    let candidate = track
        .value()
        .attr("src")
        .ok_or_else(|| Error::new("AniZone subtitle track had no URL"))?;
    let url = absolute_media_url(base, candidate)?;
    validate_media_url(&url)?;
    Ok(MediaTrack {
        url: url.clone(),
        language: track.value().attr("srclang").map(ToOwned::to_owned),
        label: track.value().attr("label").map(ToOwned::to_owned),
        format: track
            .value()
            .attr("data-type")
            .map(ToOwned::to_owned)
            .or_else(|| media_format(&url)),
        headers: media_headers(BASE_URL),
        is_default: track.value().attr("default").is_some(),
        ..MediaTrack::default()
    })
}

fn parse_anizone_url(candidate: &str) -> Result<Option<(String, Option<String>)>> {
    let url = match Url::parse(candidate) {
        Ok(url) => url,
        Err(_) => return Ok(None),
    };
    if url.scheme() != "https"
        || url.host_str() != Some("anizone.to")
        || url.port_or_known_default() != Some(443)
    {
        return Ok(None);
    }
    let segments = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !(segments.len() == 2 || segments.len() == 3) || segments[0] != "anime" {
        return Ok(None);
    }
    let slug = validate_slug(segments[1])?.to_string();
    let episode = if segments.len() == 3 {
        Some(validate_episode_slug(segments[2])?.to_string())
    } else {
        None
    };
    Ok(Some((slug, episode)))
}

fn episode_key(item_key: &str, episode: &VideoEpisode) -> Result<String> {
    let item_key = validate_slug(item_key)?;
    if let Some((slug, Some(episode_slug))) = episode
        .url
        .as_deref()
        .map(parse_anizone_url)
        .transpose()?
        .flatten()
    {
        if slug == item_key {
            return Ok(format!("{slug}/{episode_slug}"));
        }
    }
    let prefix = format!("{item_key}/");
    let episode_slug = episode
        .key
        .strip_prefix(&prefix)
        .ok_or_else(|| Error::new("AniZone episode does not belong to this title"))?;
    Ok(format!(
        "{item_key}/{}",
        validate_episode_slug(episode_slug)?
    ))
}

fn validate_site_url(candidate: &str) -> Result<()> {
    let url = Url::parse(candidate).map_err(url_error)?;
    if url.scheme() != "https"
        || url.host_str() != Some("anizone.to")
        || url.port_or_known_default() != Some(443)
    {
        return Err(Error::new("AniZone request left the approved site origin"));
    }
    Ok(())
}

fn validate_media_url(candidate: &str) -> Result<()> {
    let url = Url::parse(candidate).map_err(url_error)?;
    let host = url.host_str().unwrap_or_default();
    let approved = host == "anizone.to"
        || wildcard_host(host, "vid-cdn.xyz")
        || wildcard_host(host, "xin-cdn.xyz");
    if url.scheme() != "https" || url.port_or_known_default() != Some(443) || !approved {
        return Err(Error::new("AniZone media left the approved HTTPS origins"));
    }
    Ok(())
}

fn wildcard_host(host: &str, suffix: &str) -> bool {
    host.strip_suffix(suffix)
        .is_some_and(|prefix| prefix.ends_with('.') && prefix.len() > 1)
}

fn absolute_media_url(base: &str, candidate: &str) -> Result<String> {
    Url::parse(base)
        .map_err(url_error)?
        .join(candidate)
        .map(|url| url.to_string())
        .map_err(url_error)
}

fn validate_slug(value: &str) -> Result<&str> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(Error::new("invalid AniZone title slug"));
    }
    Ok(value)
}

fn validate_episode_slug(value: &str) -> Result<&str> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(Error::new("invalid AniZone episode slug"));
    }
    Ok(value)
}

fn reject_blocked_document(html: &str) -> Result<()> {
    if unsafe_marker(html) {
        return Err(Error::new("AniZone marked this content as unsafe"));
    }
    Ok(())
}

fn unsafe_marker(value: &str) -> bool {
    value.contains("isUnsafe: true")
        || value.contains("isUnsafe:true")
        || value.contains("\"isUnsafe\":true")
}

fn ensure_safe_text(title: &str, tags: &[String], description: Option<&str>) -> Result<()> {
    let mut values = Vec::with_capacity(tags.len() + 2);
    values.push(title);
    values.extend(tags.iter().map(String::as_str));
    if let Some(description) = description {
        values.push(description);
    }
    if values.into_iter().any(contains_blocked_term) {
        return Err(Error::new(
            "AniZone content is outside the Play-safe classification boundary",
        ));
    }
    Ok(())
}

fn contains_blocked_term(value: &str) -> bool {
    value
        .split(|character: char| !character.is_alphanumeric() && character != '+')
        .map(normalize_label)
        .filter(|word| !word.is_empty())
        .any(|word| BLOCKED_TERMS.iter().any(|blocked| word == *blocked))
}

fn normalize_label(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn extract_json_object(x_data: &str, key: &str) -> Option<Map<String, Value>> {
    let marker = format!("{key}: JSON.parse('");
    let start = x_data.find(&marker)? + marker.len();
    let encoded = &x_data[start..];
    let end = encoded.find("')")?;
    let encoded = encoded[..end].replace("\\'", "'");
    let decoded: String = serde_json::from_str(&format!("\"{encoded}\"")).ok()?;
    serde_json::from_str::<Value>(&decoded)
        .ok()?
        .as_object()
        .cloned()
}

fn preferred_title(titles: &Map<String, Value>, preferred: &str) -> Option<String> {
    [preferred, "1", "5", "8"]
        .into_iter()
        .find_map(|key| titles.get(key).and_then(Value::as_str))
        .map(clean_title)
        .filter(|title| !title.is_empty())
}

fn clean_title(value: &str) -> String {
    value
        .replace("\\/", "/")
        .replace(char::from(96), "'")
        .trim_matches('"')
        .trim()
        .to_string()
}

fn synopsis(document: &Html) -> Option<String> {
    let heading = document
        .select(&selector("h3").ok()?)
        .find(|heading| text(*heading).eq_ignore_ascii_case("Synopsis"))?;
    let parent = ElementRef::wrap(heading.parent()?)?;
    parent
        .children()
        .filter_map(ElementRef::wrap)
        .find(|element| element.value().name() == "div")
        .map(text)
        .filter(|value| !value.is_empty())
}

fn image_request(candidate: &str) -> Result<ImageRequest> {
    let url = absolute_media_url(BASE_URL, candidate)?;
    validate_media_url(&url)?;
    Ok(ImageRequest::get(url).header("Referer", format!("{BASE_URL}/")))
}

fn has_next_page(html: &str) -> bool {
    html.contains("x-intersect=\"$wire.loadMore()\"")
        || html.contains("x-intersect=\"$wire.loadMore()")
        || html.contains("x-intersect~=loadMore")
}

fn episode_number(value: &str) -> Option<f32> {
    Regex::new(r"(?i)episode\s+(\d+(?:\.\d+)?)")
        .ok()?
        .captures(value)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}

fn set_video_id(value: &str) -> Option<u32> {
    Regex::new(r#"setVideo\(['"]?(\d+)['"]?\)"#)
        .ok()?
        .captures(value)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}

fn contains_language(value: &str, code: &str, label: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    if normalized.contains(&code.to_ascii_lowercase())
        || normalized.contains(&label.to_ascii_lowercase())
    {
        return true;
    }
    language_aliases(code).iter().any(|alias| {
        normalized
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|word| word == *alias)
    })
}

fn language_aliases(code: &str) -> &'static [&'static str] {
    match code {
        "jpn" => &["ja", "jp", "jap"],
        "eng" => &["en", "eng"],
        "fra" => &["fr", "fra"],
        "deu" => &["de", "deu"],
        "ita" => &["it", "ita"],
        "kor" => &["ko", "kor"],
        "ara" => &["ar", "ara"],
        "rus" => &["ru", "rus"],
        "spa" | "spa-la" | "spa-eu" => &["es", "spa"],
        "por-br" | "por-eu" => &["pt", "por"],
        _ => &[],
    }
}

fn language_label(code: &str) -> &'static str {
    language_options()
        .iter()
        .find(|(_, value)| *value == code)
        .map(|(label, _)| *label)
        .unwrap_or("")
}

fn language_options() -> &'static [(&'static str, &'static str)] {
    &[
        ("English", "eng"),
        ("Japanese", "jpn"),
        ("Arabic", "ara"),
        ("Spanish", "spa"),
        ("Catalan", "cat"),
        ("Czech", "ces"),
        ("Danish", "dan"),
        ("German", "deu"),
        ("Greek", "ell"),
        ("Spanish (Latin American)", "spa-la"),
        ("Spanish (European)", "spa-eu"),
        ("Basque", "eus"),
        ("Finnish", "fin"),
        ("Filipino", "fil"),
        ("French", "fra"),
        ("Galician", "glg"),
        ("Hebrew", "heb"),
        ("Hindi", "hin"),
        ("Latin", "lat"),
        ("Croatian", "hrv"),
        ("Hungarian", "hun"),
        ("Indonesian", "ind"),
        ("Italian", "ita"),
        ("Korean", "kor"),
        ("Malay", "msa"),
        ("Norwegian", "nor"),
        ("Dutch", "nld"),
        ("Polish", "pol"),
        ("Portuguese (Brazilian)", "por-br"),
        ("Portuguese (European)", "por-eu"),
        ("Romanian", "ron"),
        ("Russian", "rus"),
        ("Swedish", "swe"),
        ("Thai", "tha"),
        ("Turkish", "tur"),
        ("Ukrainian", "ukr"),
        ("Vietnamese", "vie"),
        ("Chinese (Simplified)", "zho-s"),
        ("Chinese (Traditional)", "zho-t"),
    ]
}

fn audio_options() -> &'static [(&'static str, &'static str)] {
    &[
        ("English", "eng"),
        ("French", "fra"),
        ("Polish", "pol"),
        ("Korean", "kor"),
        ("Japanese", "jpn"),
        ("German", "deu"),
        ("Italian", "ita"),
        ("Spanish", "spa"),
        ("Hungarian", "hun"),
        ("Portuguese (Brazilian)", "por-br"),
        ("Arabic", "ara"),
        ("Thai", "tha"),
        ("Spanish (Latin American)", "spa-la"),
        ("Filipino", "fil"),
        ("Indonesian", "ind"),
        ("Hindi", "hin"),
    ]
}

fn preferences() -> Vec<PreferenceDefinition> {
    vec![
        PreferenceDefinition::Select {
            key: PREF_TITLE_LANGUAGE.to_string(),
            title: "Preferred title language".to_string(),
            options: vec![option("English", "1"), option("Romaji", "5")],
            default: "1".to_string(),
        },
        PreferenceDefinition::Select {
            key: PREF_QUALITY.to_string(),
            title: "Preferred quality".to_string(),
            options: ["1080", "720", "480", "360"]
                .into_iter()
                .map(|value| option(&format!("{value}p"), value))
                .collect(),
            default: "1080".to_string(),
        },
        PreferenceDefinition::Select {
            key: PREF_AUDIO.to_string(),
            title: "Preferred audio language".to_string(),
            options: audio_options()
                .iter()
                .map(|(label, value)| option(label, value))
                .collect(),
            default: "jpn".to_string(),
        },
        PreferenceDefinition::Select {
            key: PREF_SUBTITLE.to_string(),
            title: "Preferred subtitle language".to_string(),
            options: language_options()
                .iter()
                .map(|(label, value)| option(label, value))
                .collect(),
            default: "eng".to_string(),
        },
        PreferenceDefinition::Switch {
            key: PREF_LOAD_ALL.to_string(),
            title: "Load all audio and subtitle tracks".to_string(),
            summary: Some("May make playback startup slower.".to_string()),
            default: false,
        },
        PreferenceDefinition::Number {
            key: PREF_SUBTITLE_COUNT.to_string(),
            title: "Subtitle track count".to_string(),
            summary: Some("Maximum tracks when load-all is disabled.".to_string()),
            default: 2,
            min: 1,
            max: 20,
            step: 1,
        },
    ]
}

fn option(label: &str, value: &str) -> OptionItem {
    OptionItem {
        label: label.to_string(),
        value: value.to_string(),
    }
}

fn media_headers(referer: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Referer".to_string(), referer.to_string()),
        ("Origin".to_string(), BASE_URL.to_string()),
        (
            "User-Agent".to_string(),
            "Mozilla/5.0 (Linux; Android 13) AppleWebKit/537.36 Chrome/138 Mobile Safari/537.36"
                .to_string(),
        ),
    ])
}

fn media_format(value: &str) -> Option<String> {
    Url::parse(value)
        .ok()
        .and_then(|url| {
            url.path()
                .rsplit_once('.')
                .map(|(_, extension)| extension.to_ascii_lowercase())
        })
        .filter(|extension| matches!(extension.as_str(), "ass" | "ssa" | "srt" | "vtt"))
}

fn text(element: ElementRef<'_>) -> String {
    element
        .text()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn selector(value: &str) -> Result<Selector> {
    Selector::parse(value).map_err(|error| Error::new(error.to_string()))
}

fn url_error(error: url::ParseError) -> Error {
    Error::new(error.to_string())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(manatan_sdk::Extension::new().video(SOURCE_ID, AniZoneSource));

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn parses_catalog_and_filters_unsafe_cards_before_covers_surface() {
        let page = parse_listing_page(include_str!("../fixtures/catalog.html"), "1").unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].slug, "safe1234");
        assert!(page.entries[0].cover.is_some());
        assert!(page.has_next_page);
    }

    #[test]
    fn details_are_classified_and_blocked_metadata_fails_closed() {
        let safe = parse_safe_details(
            include_str!("../fixtures/details-safe.html"),
            "safe1234",
            "1",
        )
        .unwrap();
        assert_eq!(safe.title, "Safe Adventure");
        assert_eq!(safe.tags, vec!["Adventure", "Comedy"]);
        assert_eq!(safe.content_rating.as_deref(), Some("suggestive"));
        assert!(parse_safe_details(
            include_str!("../fixtures/details-blocked.html"),
            "blocked1",
            "1",
        )
        .is_err());
    }

    #[test]
    fn episode_parser_rejects_unsafe_cards_and_keeps_thumbnails() {
        let page =
            parse_episode_page(include_str!("../fixtures/episodes.html"), "safe1234", "1").unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].key, "safe1234/1");
        assert_eq!(page.entries[0].episode_number, Some(1.0));
        assert!(page.entries[0].thumbnail.is_some());
    }

    #[test]
    fn playback_parses_subtitles_servers_and_hls_variants() {
        let playback = parse_playback(include_str!("../fixtures/playback.html")).unwrap();
        assert_eq!(playback.csrf, "fixture-token");
        assert_eq!(playback.servers.len(), 2);
        assert_eq!(playback.default_source.subtitles.len(), 1);
        assert_eq!(
            playback.default_source.subtitles[0].format.as_deref(),
            Some("ass")
        );
        let master = parse_hls_master(
            include_str!("../fixtures/master.m3u8"),
            "https://seiryuu.vid-cdn.xyz/video/master.m3u8",
        )
        .unwrap();
        assert_eq!(master.variants.len(), 2);
        assert_eq!(master.variants[0].quality, "1080p");
        assert_eq!(master.audio_tracks.len(), 1);
        assert_eq!(master.subtitles.len(), 1);
    }

    #[test]
    fn url_and_media_boundaries_are_https_and_exact() {
        assert_eq!(
            parse_anizone_url("https://anizone.to/anime/safe1234/1")
                .unwrap()
                .unwrap(),
            ("safe1234".to_string(), Some("1".to_string()))
        );
        assert!(parse_anizone_url("https://evil.example/anime/safe1234")
            .unwrap()
            .is_none());
        assert!(validate_media_url("https://seiryuu.vid-cdn.xyz/v/master.m3u8").is_ok());
        assert!(validate_media_url("https://vid-cdn.xyz/v/master.m3u8").is_err());
        assert!(validate_media_url("http://seiryuu.vid-cdn.xyz/v/master.m3u8").is_err());
        assert!(validate_media_url("https://vid-cdn.xyz.evil.example/v/master.m3u8").is_err());
    }

    #[test]
    fn safety_words_are_tokenized() {
        assert!(contains_blocked_term("Mature"));
        assert!(contains_blocked_term("R18"));
        assert!(contains_blocked_term("18+"));
        assert!(contains_blocked_term("Adult Cast"));
        assert!(!contains_blocked_term("Adventure"));
    }

    #[test]
    fn manifest_declares_only_required_play_safe_capabilities() {
        let manifest: Value = serde_json::from_str(include_str!("../manifest.json")).unwrap();
        assert_eq!(manifest["minimumManatanVersion"], "0.3.1");
        assert_eq!(manifest["permissions"]["webview"], false);
        assert_eq!(manifest["permissions"]["javascript"], false);
        assert_eq!(
            manifest["publisher"]["publicKey"],
            "88b67d201d387960b96b64b5c4ca39d5edceef6e8a088316449a2d5437a889ac"
        );
        assert_eq!(
            manifest["assets"][0]["sha256"],
            format!("{:x}", Sha256::digest(include_bytes!("../assets/icon.png")))
        );
    }

    #[test]
    #[ignore = "requires snapshots downloaded from live AniZone endpoints"]
    fn parses_live_downloaded_catalog_details_episodes_playback_and_playlist() {
        let directory = std::env::var("ANIZONE_LIVE_FIXTURE_DIR")
            .expect("set ANIZONE_LIVE_FIXTURE_DIR to downloaded live snapshots");
        let read = |name: &str| {
            std::fs::read_to_string(format!("{directory}/{name}"))
                .unwrap_or_else(|error| panic!("failed to read {name}: {error}"))
        };
        let catalog = parse_listing_page(&read("catalog.html"), "1").unwrap();
        assert!(!catalog.entries.is_empty());
        let latest = parse_listing_page(&read("latest.html"), "1").unwrap();
        assert!(!latest.entries.is_empty());
        let details = parse_safe_details(&read("details.html"), "qtskwpje", "1").unwrap();
        assert_eq!(details.title, "Eiji");
        assert!(details.cover.is_some());
        let episodes = parse_episode_page(&read("details.html"), "qtskwpje", "1").unwrap();
        assert!(!episodes.entries.is_empty());
        let playback = parse_playback(&read("playback.html")).unwrap();
        assert!(playback.default_source.url.starts_with("https://"));
        assert!(!playback.default_source.subtitles.is_empty());
        let playlist =
            parse_hls_master(&read("master.m3u8"), &playback.default_source.url).unwrap();
        assert!(!playlist.variants.is_empty() || read("master.m3u8").contains("#EXTINF"));
    }
}
