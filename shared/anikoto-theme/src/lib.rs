//! AnikotoTheme video-family support derived from `yuzono/anime-extensions`.
//!
//! The upstream implementation is Apache-2.0. Android networking and the
//! local playlist server are deliberately replaced with Manatan host calls
//! and declarative resource processing.

use std::{collections::BTreeMap, marker::PhantomData};

use base64::{engine::general_purpose, Engine};
use manatan_common::{absolute_url, normalize_space, require};
use manatan_sdk::{
    client::Client,
    context,
    html::{self, ElementRef, Html, Selector},
    CatalogItem, Error, FilterDefinition, MediaResourceKind, MediaSegment, MediaTrack, OptionItem,
    Paged, PreferenceDefinition, Result, SegmentProcessing, SegmentRule, UrlResolveResult,
    VideoEpisode, VideoHoster, VideoSource, VideoStream,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

const PLAY_ALLOWED_RATINGS: &[&str] = &["G", "PG", "PG-13", "R", "R+"];
const BLOCKED_CONTENT_TAGS: &[&str] = &["adult", "hentai", "porn", "smut", "ecchi"];

pub trait AnikotoConfig: 'static {
    const NAME: &'static str;
    const LANG: &'static str;
    const BASE_URL: &'static str;
    const DOMAINS: &'static [&'static str];
    const HOSTERS: &'static [&'static str];
    const MAPPER_URL: &'static str = "https://mapper.nekostream.site/api";

    fn listing_thumbnail_selector() -> &'static str {
        "div.poster img"
    }

    fn detail_thumbnail_selector() -> &'static str {
        "div.poster img, div.detail img"
    }

    fn synopsis_content_selector() -> &'static str {
        "div.synopsis > div.shorting > div.content"
    }

    fn alias_container_selector() -> &'static str {
        "div.alias"
    }

    fn score_label() -> &'static str {
        "Score"
    }

    fn episode_list_selector() -> &'static str {
        "div.episodes ul > li > a"
    }

    fn server_group_selector() -> &'static str {
        "div.servers > div.type"
    }

    fn server_item_selector() -> &'static str {
        "li"
    }

    fn server_name_selector() -> Option<&'static str> {
        None
    }

    fn canonical_server_name(raw: &str) -> String {
        raw.trim_end_matches(['-', ' ']).to_owned()
    }

    fn server_matches(configured: &str, actual: &str) -> bool {
        configured.eq_ignore_ascii_case(actual)
    }

    fn map_filter_value(_key: &str, value: &str) -> String {
        value.to_owned()
    }

    fn should_generate_search_vrf(query: &str) -> bool {
        !query.is_empty()
    }
}

struct StandardAnikotoConfig;

impl AnikotoConfig for StandardAnikotoConfig {
    const NAME: &'static str = "Anikoto";
    const LANG: &'static str = "en";
    const BASE_URL: &'static str = "https://example.invalid";
    const DOMAINS: &'static [&'static str] = &[];
    const HOSTERS: &'static [&'static str] = &[];
}

pub struct AnikotoSource<C: AnikotoConfig> {
    client: Client,
    marker: PhantomData<C>,
}

impl<C: AnikotoConfig> Default for AnikotoSource<C> {
    fn default() -> Self {
        Self {
            client: Client::browser(),
            marker: PhantomData,
        }
    }
}

impl<C: AnikotoConfig> AnikotoSource<C> {
    fn base_url(&self) -> String {
        let selected = context::preference::<String>("domain")
            .ok()
            .flatten()
            .unwrap_or_else(|| C::BASE_URL.to_owned());
        let selected_host = Url::parse(&selected)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned));
        if selected_host
            .as_deref()
            .is_some_and(|host| C::DOMAINS.contains(&host))
        {
            selected.trim_end_matches('/').to_owned()
        } else {
            C::BASE_URL.trim_end_matches('/').to_owned()
        }
    }

    fn get_text(&self, url: &str, referer: Option<&str>, ajax: bool) -> Result<(String, String)> {
        let mut request = self
            .client
            .get(url)
            .cookies_for(url)
            .rate_limit(format!("anikoto:{}", host(url)?), 200);
        if let Some(referer) = referer {
            request = request.header("Referer", referer);
        }
        if ajax {
            request = request
                .header("Accept", "application/json, text/javascript, */*; q=0.01")
                .header("X-Requested-With", "XMLHttpRequest");
        }
        let response = request.send()?.error_for_status()?;
        Ok((response.text()?.to_owned(), response.final_url().to_owned()))
    }

    fn fetch_listing(&self, section: &str, page: u32) -> Result<Paged<CatalogItem>> {
        let url = search_url_for::<C>(
            &self.base_url(),
            "",
            page,
            &json!({"sort": section, "rating": PLAY_ALLOWED_RATINGS}),
        )?;
        let (body, final_url) =
            self.get_text(&url, Some(&format!("{}/", self.base_url())), false)?;
        parse_listing_html_for::<C>(&body, &final_url)
    }

    fn anime_id(&self, item: &CatalogItem) -> Result<(String, String)> {
        let item_url = item
            .url
            .as_deref()
            .map(str::to_owned)
            .unwrap_or_else(|| absolute_url(&self.base_url(), &item.key).unwrap_or_default());
        if let Some(id) = item.extra.get("animeId").and_then(Value::as_str) {
            return Ok((id.to_owned(), item_url));
        }
        let (body, final_url) =
            self.get_text(&item_url, Some(&format!("{}/", self.base_url())), false)?;
        let document = html::document(&body);
        let id = first_attr(&document, "[data-id]", "data-id")
            .or_else(|| first_attr(&document, "[data-tip]", "data-tip"));
        Ok((
            require(id, "Anikoto details page has no anime id")?,
            final_url,
        ))
    }

    fn server_hosters(&self, episode: &VideoEpisode) -> Result<Vec<VideoHoster>> {
        let ids = episode
            .extra
            .get("serverIds")
            .and_then(Value::as_str)
            .or_else(|| episode.key.split('&').next())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::new("Anikoto episode has no server ids"))?;
        let episode_path = episode
            .extra
            .get("episodePath")
            .and_then(Value::as_str)
            .or(episode.url.as_deref())
            .unwrap_or_default();
        let referer = absolute_url(&self.base_url(), episode_path)?;
        let url = format!(
            "{}/ajax/server/list?servers={}",
            self.base_url(),
            encode(ids)
        );
        let (body, _) = self.get_text(&url, Some(&referer), true)?;
        let fragment = result_fragment(&body)?;
        let mut hosters = parse_server_list_html_for::<C>(&fragment, episode_path)?;
        hosters.extend(self.mapper_hosters(episode).unwrap_or_default());
        Ok(filter_hosters::<C>(hosters))
    }

    fn mapper_hosters(&self, episode: &VideoEpisode) -> Result<Vec<VideoHoster>> {
        let mal = extra_string(episode, "malId")?;
        let slug = extra_string(episode, "slug")?;
        let timestamp = extra_string(episode, "timestamp")?;
        let url = format!(
            "{}/mal/{}/{}/{}",
            C::MAPPER_URL.trim_end_matches('/'),
            encode(&mal),
            encode(&slug),
            encode(&timestamp)
        );
        let (body, _) = self.get_text(&url, Some(&format!("{}/", self.base_url())), true)?;
        let value: Value = serde_json::from_str(&body)?;
        let mut hosters = Vec::new();
        let Some(map) = value.as_object() else {
            return Ok(hosters);
        };
        for (name, value) in map {
            if name.eq_ignore_ascii_case("status") {
                continue;
            }
            let display = mapper_name(name);
            for (key, label) in [("sub", "H-Sub"), ("dub", "A-Dub")] {
                if let Some(url) = value
                    .get(key)
                    .and_then(|value| value.get("url"))
                    .and_then(Value::as_str)
                {
                    hosters.push(VideoHoster {
                        key: format!("mapper:{name}:{key}"),
                        name: format!("{display} - {label}"),
                        url: Some(url.to_owned()),
                        lazy: true,
                        internal_data: Some(json!({"type": label, "serverId": url, "serverName": display, "episodePath": episode.url}).to_string()),
                        ..VideoHoster::default()
                    });
                }
            }
        }
        Ok(hosters)
    }

    fn resolve_hoster(&self, hoster: &VideoHoster) -> Result<Vec<VideoStream>> {
        let data: HosterData = serde_json::from_str(
            hoster
                .internal_data
                .as_deref()
                .ok_or_else(|| Error::new("Anikoto hoster has no internal data"))?,
        )?;
        let (embed_url, intro, outro) = if data.server_id.starts_with("http") {
            (data.server_id.clone(), None, None)
        } else {
            let referer = absolute_url(&self.base_url(), &data.episode_path)?;
            let url = format!(
                "{}/ajax/server?get={}",
                self.base_url(),
                encode(&data.server_id)
            );
            let (body, _) = self.get_text(&url, Some(&referer), true)?;
            let response: ServerResponse = serde_json::from_str(&body)?;
            (
                response.result.url,
                response
                    .result
                    .skip_data
                    .as_ref()
                    .and_then(|value| segment(&value.intro)),
                response
                    .result
                    .skip_data
                    .as_ref()
                    .and_then(|value| segment(&value.outro)),
            )
        };
        self.extract_player(&embed_url, &data, 0, intro, outro)
    }

    fn extract_player(
        &self,
        embed_url: &str,
        data: &HosterData,
        depth: u8,
        intro: Option<MediaSegment>,
        outro: Option<MediaSegment>,
    ) -> Result<Vec<VideoStream>> {
        ensure_depth(depth)?;
        if embed_url.contains("mewcdn.online/player/plyr.php") {
            return self.extract_mewcdn(embed_url, data, intro, outro);
        }
        if is_hls_url(embed_url) {
            return Ok(vec![stream::<C>(
                embed_url,
                data,
                None,
                Vec::new(),
                intro,
                outro,
            )?]);
        }
        let (body, _) = self.get_text(embed_url, Some(&format!("{}/", self.base_url())), false)?;
        if body.trim_start().starts_with("#EXTM3U") {
            return Ok(vec![stream::<C>(
                embed_url,
                data,
                None,
                Vec::new(),
                intro,
                outro,
            )?]);
        }
        if let Some(data_id) = capture(&body, r#"data-id=["']([^"']+)"#, 1) {
            return self.extract_sources_api(embed_url, &data_id, data, intro, outro);
        }
        if let Some(iframe) = capture(&body, r#"(?is)<iframe[^>]+src=["']([^"']+)"#, 1) {
            let next = absolute_url(embed_url, &iframe)?;
            return self.extract_player(&next, data, depth + 1, intro, outro);
        }
        let direct = capture(&body, r#"https?://[^\s"'<>]+\.m3u8[^\s"'<>]*"#, 0)
            .or_else(|| capture(&body, r#"(?is)<source[^>]+src=["']([^"']+\.m3u8[^"']*)"#, 1))
            .or_else(|| capture(&body, r#"(?i)(?:file|source|url|src)\s*[:=]\s*["']([^"']*(?:\.m3u8|/stream/)[^"']*)["']"#, 1));
        if let Some(direct) = direct {
            let url = absolute_url(embed_url, &direct)?;
            return Ok(vec![stream::<C>(
                &url,
                data,
                Some(embed_url),
                Vec::new(),
                intro,
                outro,
            )?]);
        }
        Err(Error::new(format!(
            "no supported stream found in {embed_url}"
        )))
    }

    fn extract_sources_api(
        &self,
        embed_url: &str,
        data_id: &str,
        data: &HosterData,
        intro: Option<MediaSegment>,
        outro: Option<MediaSegment>,
    ) -> Result<Vec<VideoStream>> {
        let embed = Url::parse(embed_url).map_err(url_error)?;
        let host = require(embed.host_str(), "player URL has no host")?;
        let origin = format!("{}://{}", embed.scheme(), host);
        let stream_type = embed
            .path_segments()
            .and_then(Iterator::last)
            .filter(|value| matches!(*value, "sub" | "dub"));
        let primary = format!(
            "{origin}/stream/getSources?id={}&id={}",
            encode(data_id),
            encode(data_id)
        );
        let response = self.get_text(&primary, Some(embed_url), true);
        let body = match response {
            Ok((body, _)) => body,
            Err(_) => {
                let mut fallback = format!(
                    "{origin}/stream/getSourcesNew?id={}&id={}",
                    encode(data_id),
                    encode(data_id)
                );
                if let Some(stream_type) = stream_type {
                    fallback.push_str(&format!("&type={0}&type={0}", encode(stream_type)));
                }
                self.get_text(&fallback, Some(embed_url), true)?.0
            }
        };
        let response: SourceResponse = serde_json::from_str(&body)?;
        let url = source_url(&response.sources)
            .filter(|value| value.starts_with("http"))
            .ok_or_else(|| Error::new("player source API returned no HLS URL"))?;
        let tracks = response
            .tracks
            .unwrap_or_default()
            .into_iter()
            .filter(|track| track.kind == "captions")
            .map(|track| MediaTrack {
                url: absolute_url(embed_url, &track.file).unwrap_or(track.file),
                label: Some(track.label),
                ..MediaTrack::default()
            })
            .collect();
        Ok(vec![stream::<C>(
            &url,
            data,
            Some(&format!("{origin}/")),
            tracks,
            intro,
            outro,
        )?])
    }

    fn extract_mewcdn(
        &self,
        embed_url: &str,
        data: &HosterData,
        intro: Option<MediaSegment>,
        outro: Option<MediaSegment>,
    ) -> Result<Vec<VideoStream>> {
        let url = Url::parse(embed_url).map_err(url_error)?;
        let fragment = require(url.fragment(), "mewcdn player URL has no fragment")?;
        let decoded = general_purpose::STANDARD
            .decode(fragment)
            .or_else(|_| general_purpose::URL_SAFE.decode(fragment))
            .map_err(|error| Error::new(error.to_string()))?;
        let mut hls = String::from_utf8(decoded).map_err(|error| Error::new(error.to_string()))?;
        let body = self
            .get_text(embed_url, Some(&format!("{}/", self.base_url())), false)?
            .0;
        if let Some(map) = capture(&body, r#"(?s)var\s+HOST_MAP\s*=\s*\{([^}]+)\}"#, 1) {
            let entries = Regex::new(r#"['"]([^'"]+)['"]\s*:\s*['"]([^'"]+)['"]"#).unwrap();
            for captures in entries.captures_iter(&map) {
                if hls.contains(&captures[1]) {
                    hls = hls.replace(&captures[1], &captures[2]);
                    break;
                }
            }
        }
        Ok(vec![stream::<C>(
            &hls,
            data,
            Some("https://mewcdn.online/"),
            Vec::new(),
            intro,
            outro,
        )?])
    }
}

impl<C: AnikotoConfig> VideoSource for AnikotoSource<C> {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.fetch_listing("most-viewed", page)
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.fetch_listing("latest-updated", page)
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let filters = play_safe_filters(filters)?;
        let url = search_url_for::<C>(&self.base_url(), query, page, &filters)?;
        let (body, final_url) =
            self.get_text(&url, Some(&format!("{}/", self.base_url())), false)?;
        parse_listing_html_for::<C>(&body, &final_url)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = item.url.as_deref().unwrap_or(&item.key);
        let url = absolute_url(&self.base_url(), url)?;
        let (body, final_url) =
            self.get_text(&url, Some(&format!("{}/", self.base_url())), false)?;
        let mut parsed = parse_details_html_for::<C>(&body, &final_url)?;
        reject_blocked_details(&parsed)?;
        parsed.key = item.key;
        Ok(parsed)
    }

    fn episodes(&mut self, item: CatalogItem) -> Result<Vec<VideoEpisode>> {
        let item = self.details(item)?;
        let (id, item_url) = self.anime_id(&item)?;
        let endpoint = format!(
            "{}/ajax/episode/list/{}?vrf={}",
            self.base_url(),
            encode(&id),
            encode(&vrf_encrypt(&id))
        );
        let (body, _) = self.get_text(&endpoint, Some(&item_url), true)?;
        parse_episodes_json_for::<C>(&body, &item_url)
    }

    fn streams(&mut self, item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoStream>> {
        let item = self.details(item)?;
        let hosters = self.server_hosters(&episode)?;
        let mut streams = Vec::new();
        let mut errors = Vec::new();
        for hoster in hosters {
            match self.hoster_streams(item.clone(), episode.clone(), hoster.clone()) {
                Ok(found) => streams.extend(found),
                Err(error) => errors.push(format!("{}: {error}", hoster.name)),
            }
        }
        if streams.is_empty() && !errors.is_empty() {
            return Err(Error::new(errors.join("; ")));
        }
        Ok(sort_streams(streams))
    }

    fn hosters(&mut self, item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoHoster>> {
        self.details(item)?;
        self.server_hosters(&episode)
    }

    fn hoster_streams(
        &mut self,
        _item: CatalogItem,
        _episode: VideoEpisode,
        hoster: VideoHoster,
    ) -> Result<Vec<VideoStream>> {
        self.resolve_hoster(&hoster)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(anikoto_filters(2027))
    }

    fn preferences(&mut self) -> Result<Vec<PreferenceDefinition>> {
        Ok(anikoto_preferences::<C>())
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(absolute_url(
            &self.base_url(),
            item.url.as_deref().unwrap_or(&item.key),
        )?))
    }

    fn episode_url(
        &mut self,
        _item: &CatalogItem,
        episode: &VideoEpisode,
    ) -> Result<Option<String>> {
        episode
            .url
            .as_deref()
            .map(|url| absolute_url(&self.base_url(), url))
            .transpose()
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let base = Url::parse(&self.base_url()).map_err(url_error)?;
        let url = Url::parse(candidate).map_err(url_error)?;
        if base.host_str() != url.host_str() || !url.path().starts_with("/watch/") {
            return Ok(None);
        }
        let (item_path, episode_number) = split_episode_path(url.path());
        let item_url = absolute_url(&self.base_url(), &item_path)?;
        let item = self.details(CatalogItem {
            key: item_path.clone(),
            url: Some(item_url),
            language: Some(C::LANG.to_owned()),
            ..CatalogItem::default()
        })?;
        let mut result = UrlResolveResult {
            item: Some(item),
            ..UrlResolveResult::default()
        };
        if let Some(number) = episode_number {
            result.episode_key = Some(url.path().to_owned());
            result.video_episode = Some(VideoEpisode {
                key: url.path().to_owned(),
                episode_number: Some(number),
                url: Some(url.to_string()),
                language: Some(C::LANG.to_owned()),
                ..VideoEpisode::default()
            });
        }
        Ok(Some(result))
    }
}

pub fn listing_url(base: &str, section: &str, page: u32) -> Result<String> {
    let mut url = Url::parse(base).map_err(url_error)?;
    url.set_path(&format!("/{}/", section.trim_matches('/')));
    url.query_pairs_mut()
        .append_pair("page", &page.max(1).to_string());
    Ok(url.to_string())
}

pub fn search_url(base: &str, query: &str, page: u32, filters: &Value) -> Result<String> {
    search_url_for::<StandardAnikotoConfig>(base, query, page, filters)
}

pub fn search_url_for<C: AnikotoConfig>(
    base: &str,
    query: &str,
    page: u32,
    filters: &Value,
) -> Result<String> {
    let mut url = Url::parse(base).map_err(url_error)?;
    url.set_path("/filter");
    let vrf = if C::should_generate_search_vrf(query) {
        vrf_encrypt(query)
    } else {
        String::new()
    };
    let mut pairs = url.query_pairs_mut();
    pairs
        .append_pair("keyword", query)
        .append_pair("page", &page.max(1).to_string())
        .append_pair("vrf", &vrf);
    for key in [
        "genre",
        "season",
        "year",
        "term_type",
        "status",
        "language",
        "rating",
    ] {
        for value in filter_values(filters, key) {
            pairs.append_pair(&format!("{key}[]"), &C::map_filter_value(key, &value));
        }
    }
    if let Some(sort) = filters
        .get("sort")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        pairs.append_pair("sort", sort);
    }
    drop(pairs);
    Ok(url.to_string())
}

fn filter_values(filters: &Value, key: &str) -> Vec<String> {
    match filters.get(key) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::Object(values)) => values
            .iter()
            .filter(|(_, value)| value.as_bool().unwrap_or(false))
            .map(|(key, _)| key.clone())
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.clone()],
        _ => Vec::new(),
    }
}

fn play_safe_filters(filters: &Value) -> Result<Value> {
    let mut filters = filters
        .as_object()
        .cloned()
        .ok_or_else(|| Error::new("Anikoto search filters must be a JSON object"))?;
    filters.insert("rating".into(), json!(PLAY_ALLOWED_RATINGS));
    if let Some(Value::Array(genres)) = filters.get_mut("genre") {
        genres.retain(|value| value.as_str() != Some("214"));
    }
    Ok(Value::Object(filters))
}

pub fn parse_listing_html(source: &str, base: &str) -> Result<Paged<CatalogItem>> {
    parse_listing_html_for::<StandardAnikotoConfig>(source, base)
}

pub fn parse_listing_html_for<C: AnikotoConfig>(
    source: &str,
    base: &str,
) -> Result<Paged<CatalogItem>> {
    let document = html::document(source);
    let items = selector("div.ani.items > div.item")?;
    let link = selector("a.name")?;
    let image = selector(C::listing_thumbnail_selector())?;
    let next = selector("nav > ul.pagination > li.active ~ li")?;
    let mut entries = Vec::new();
    for element in document.select(&items) {
        let Some(anchor) = element.select(&link).next() else {
            continue;
        };
        let Some(href) = attr(anchor, "href") else {
            continue;
        };
        let title = normalize_space(&html::text(anchor));
        if title.is_empty() {
            continue;
        }
        let href = absolute_url(base, &strip_episode_suffix(&href))?;
        let key = Url::parse(&href).map_err(url_error)?.path().to_owned();
        let cover = element.select(&image).next().and_then(|image| {
            attr(image, "data-src")
                .or_else(|| attr(image, "src"))
                .and_then(|candidate| absolute_url(base, &candidate).ok())
        });
        let mut item = CatalogItem::new(key, title);
        item.url = Some(href);
        item.cover = cover.map(Into::into);
        // The request that produced this listing is forced to the
        // non-explicit ratings accepted by the Play-safe package.
        item.content_rating = Some("suggestive".to_owned());
        entries.push(item);
    }
    Ok(Paged::new(entries, document.select(&next).next().is_some()))
}

pub fn parse_details_html(source: &str, page_url: &str) -> Result<CatalogItem> {
    parse_details_html_for::<StandardAnikotoConfig>(source, page_url)
}

pub fn parse_details_html_for<C: AnikotoConfig>(
    source: &str,
    page_url: &str,
) -> Result<CatalogItem> {
    let document = html::document(source);
    let title = first_text(&document, "h1.title, h2.title")
        .ok_or_else(|| Error::new("Anikoto details page has no title"))?;
    let mut item = CatalogItem::new(Url::parse(page_url).map_err(url_error)?.path(), title);
    item.url = Some(page_url.to_owned());
    item.cover = first_element(&document, C::detail_thumbnail_selector())
        .and_then(|image| attr(image, "data-src").or_else(|| attr(image, "src")))
        .and_then(|url| absolute_url(page_url, &url).ok())
        .map(Into::into);
    item.tags = metadata_links(&document, "Genres")?;
    item.authors = metadata_links(&document, "Studios")?;
    item.description = first_text(&document, C::synopsis_content_selector());
    item.rating =
        metadata_value(&document, C::score_label())?.and_then(|value| extract_decimal(&value));
    if let Some(aliases) = first_text(&document, C::alias_container_selector()) {
        item.extra.insert("aliases".into(), json!(aliases));
    }
    item.status = metadata_value(&document, "Status")?.map(|value| {
        let status = match value.to_ascii_lowercase().as_str() {
            "ongoing anime" | "currently airing" => "ongoing",
            "finished airing" | "completed" => "completed",
            _ => "unknown",
        };
        json!(status)
    });
    if let Some(id) = first_attr(&document, "[data-id]", "data-id")
        .or_else(|| first_attr(&document, "[data-tip]", "data-tip"))
    {
        item.extra.insert("animeId".into(), json!(id));
    }
    item.initialized = true;
    item.content_rating = Some("suggestive".to_owned());
    Ok(item)
}

fn reject_blocked_details(item: &CatalogItem) -> Result<()> {
    let blocked = item.tags.iter().find(|tag| {
        let normalized = tag.trim().to_ascii_lowercase();
        BLOCKED_CONTENT_TAGS.iter().any(|blocked| {
            normalized == *blocked
                || normalized.contains(&format!("{blocked} "))
                || normalized.contains(&format!(" {blocked}"))
        })
    });
    if let Some(tag) = blocked {
        return Err(Error::new(format!(
            "content tagged {tag:?} is unavailable in this package"
        )));
    }
    Ok(())
}

pub fn parse_episodes_json(source: &str, anime_url: &str) -> Result<Vec<VideoEpisode>> {
    parse_episodes_json_for::<StandardAnikotoConfig>(source, anime_url)
}

pub fn parse_episodes_json_for<C: AnikotoConfig>(
    source: &str,
    anime_url: &str,
) -> Result<Vec<VideoEpisode>> {
    let fragment = result_fragment(source)?;
    let document = html::fragment(&fragment);
    let episodes = selector(C::episode_list_selector())?;
    let name = selector("span.d-title")?;
    let mut entries = Vec::new();
    for anchor in document.select(&episodes) {
        let number = attr(anchor, "data-num")
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or_default();
        let ids = attr(anchor, "data-ids").unwrap_or_default();
        if ids.is_empty() {
            continue;
        }
        let parent = anchor.parent().and_then(ElementRef::wrap);
        let title_attr = parent
            .and_then(|element| attr(element, "title"))
            .unwrap_or_default();
        let episode_title = parent
            .and_then(|element| element.select(&name).next())
            .map(html::text)
            .filter(|value| !value.is_empty());
        let episode_path = format!(
            "{}/ep-{}",
            strip_episode_suffix(Url::parse(anime_url).map_err(url_error)?.path()),
            number_label(number)
        );
        let mut extra = BTreeMap::new();
        extra.insert("serverIds".into(), json!(ids));
        extra.insert("episodePath".into(), json!(episode_path));
        for (attribute, key) in [
            ("data-mal", "malId"),
            ("data-slug", "slug"),
            ("data-timestamp", "timestamp"),
        ] {
            if let Some(value) = attr(anchor, attribute).filter(|value| !value.is_empty()) {
                extra.insert(key.into(), json!(value));
            }
        }
        let labels = [
            (attr(anchor, "data-sub").as_deref() == Some("1"), "Sub"),
            (
                title_attr.to_ascii_lowercase().contains("softsub"),
                "SoftSub",
            ),
            (attr(anchor, "data-dub").as_deref() == Some("1"), "Dub"),
        ]
        .into_iter()
        .filter(|(present, _)| *present)
        .map(|(_, label)| label.to_owned())
        .collect();
        entries.push(VideoEpisode {
            key: format!("{ids}&epurl={episode_path}"),
            title: Some(
                episode_title
                    .filter(|value| value != &format!("Episode {}", number_label(number)))
                    .map_or_else(
                        || format!("Episode {}", number_label(number)),
                        |value| format!("Episode {}: {value}", number_label(number)),
                    ),
            ),
            episode_number: Some(number),
            url: Some(absolute_url(anime_url, &episode_path)?),
            labels,
            extra,
            ..VideoEpisode::default()
        });
    }
    entries.reverse();
    Ok(entries)
}

pub fn parse_server_list_html(source: &str, episode_path: &str) -> Result<Vec<VideoHoster>> {
    parse_server_list_html_for::<StandardAnikotoConfig>(source, episode_path)
}

pub fn parse_server_list_html_for<C: AnikotoConfig>(
    source: &str,
    episode_path: &str,
) -> Result<Vec<VideoHoster>> {
    let document = html::fragment(source);
    let types = selector(C::server_group_selector())?;
    let label_selector = selector("label")?;
    let servers = selector(C::server_item_selector())?;
    let server_name_selector = C::server_name_selector().map(selector).transpose()?;
    let mut hosters = Vec::new();
    for group in document.select(&types) {
        let label_text = group
            .select(&label_selector)
            .next()
            .map(html::text)
            .unwrap_or_default();
        let kind = type_label(
            &label_text,
            attr(group, "data-type").as_deref().unwrap_or_default(),
        );
        for server in group.select(&servers) {
            if has_class(server, "download-icon") {
                continue;
            }
            let Some(server_id) = attr(server, "data-link-id") else {
                continue;
            };
            let raw_name = server_name_selector
                .as_ref()
                .and_then(|selector| server.select(selector).next())
                .map(html::text)
                .unwrap_or_else(|| html::text(server));
            let name = C::canonical_server_name(&normalize_space(&raw_name));
            if name.is_empty() {
                continue;
            }
            hosters.push(VideoHoster {
                key: server_id.clone(),
                name: format!("{} - {kind}", name.trim_end_matches(['-', ' '])),
                lazy: true,
                internal_data: Some(json!({"type": kind, "serverId": server_id, "serverName": name, "episodePath": episode_path}).to_string()),
                ..VideoHoster::default()
            });
        }
    }
    Ok(hosters)
}

fn filter_hosters<C: AnikotoConfig>(hosters: Vec<VideoHoster>) -> Vec<VideoHoster> {
    let enabled_servers = context::preference::<Vec<String>>("servers").ok().flatten();
    let enabled_types = context::preference::<Vec<String>>("types").ok().flatten();
    hosters
        .into_iter()
        .filter(|hoster| {
            let Some(data) = hoster
                .internal_data
                .as_deref()
                .and_then(|value| serde_json::from_str::<HosterData>(value).ok())
            else {
                return false;
            };
            enabled_servers.as_ref().is_none_or(|values| {
                values
                    .iter()
                    .any(|value| C::server_matches(value, &data.server_name))
            }) && enabled_types.as_ref().is_none_or(|values| {
                values
                    .iter()
                    .any(|value| value.eq_ignore_ascii_case(&data.kind))
            })
        })
        .collect()
}

fn stream<C: AnikotoConfig>(
    url: &str,
    data: &HosterData,
    referer: Option<&str>,
    subtitles: Vec<MediaTrack>,
    intro: Option<MediaSegment>,
    outro: Option<MediaSegment>,
) -> Result<VideoStream> {
    let parsed = Url::parse(url).map_err(url_error)?;
    let origin = format!(
        "{}://{}",
        parsed.scheme(),
        require(parsed.host_str(), "stream URL has no host")?
    );
    let referer = referer
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{origin}/"));
    let headers = BTreeMap::from([("Referer".into(), referer), ("Origin".into(), origin)]);
    let preferred_server = context::preference::<String>("server").ok().flatten();
    let preferred_type = context::preference::<String>("type").ok().flatten();
    Ok(VideoStream {
        url: url.to_owned(),
        name: Some(format!(
            "{} - {} - Auto",
            data.server_name.trim_end_matches(['-', ' ']),
            data.kind
        )),
        quality: Some("Auto".into()),
        format: Some("hls".into()),
        is_hls: true,
        requires_proxy: true,
        preferred: preferred_server
            .as_deref()
            .is_some_and(|value| C::server_matches(value, &data.server_name))
            && preferred_type
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(&data.kind)),
        initialized: true,
        headers,
        subtitles,
        intro,
        outro,
        segment_processing: Some(anikoto_segment_processing()),
        ..VideoStream::default()
    })
}

pub fn anikoto_segment_processing() -> SegmentProcessing {
    SegmentProcessing {
        rewrite_playlist: true,
        max_resource_bytes: Some(64 * 1024 * 1024),
        rules: vec![
            SegmentRule {
                resource_types: vec![MediaResourceKind::Segment],
                host_patterns: vec!["*.ibyteimg.com".into(), "*.tiktokcdn.com".into()],
                strip_prefix_bytes: Some(252),
                ..SegmentRule::default()
            },
            SegmentRule {
                resource_types: vec![MediaResourceKind::Segment],
                auto_detect_media_offset: true,
                probe_bytes: Some(4096),
                ..SegmentRule::default()
            },
        ],
        ..SegmentProcessing::default()
    }
}

pub fn vrf_encrypt(input: &str) -> String {
    let mut value = exchange(input, "AP6GeR8H0lwUz1", "UAz8Gwl10P6ReH");
    value = general_purpose::URL_SAFE.encode(rc4("ItFKjuWokn4ZpB", value.as_bytes()));
    value = general_purpose::URL_SAFE.encode(rc4("fOyt97QWFB3", value.as_bytes()));
    value = exchange(&value, "1majSlPQd2M5", "da1l2jSmP5QM");
    value = exchange(&value, "CPYvHj09Au3", "0jHA9CPYu3v");
    value = value.chars().rev().collect();
    value = general_purpose::URL_SAFE.encode(rc4("736y1uTJpBLUX", value.as_bytes()));
    general_purpose::URL_SAFE.encode(value.as_bytes())
}

fn rc4(key: &str, input: &[u8]) -> Vec<u8> {
    let key = key.as_bytes();
    let mut state = [0u8; 256];
    for (index, value) in state.iter_mut().enumerate() {
        *value = index as u8;
    }
    let mut j = 0usize;
    for index in 0..256 {
        j = (j + state[index] as usize + key[index % key.len()] as usize) & 255;
        state.swap(index, j);
    }
    let (mut i, mut j) = (0usize, 0usize);
    input
        .iter()
        .map(|byte| {
            i = (i + 1) & 255;
            j = (j + state[i] as usize) & 255;
            state.swap(i, j);
            byte ^ state[(state[i] as usize + state[j] as usize) & 255]
        })
        .collect()
}

fn exchange(input: &str, source: &str, target: &str) -> String {
    input
        .chars()
        .map(|character| {
            source
                .find(character)
                .and_then(|index| target.chars().nth(index))
                .unwrap_or(character)
        })
        .collect()
}

pub fn anikoto_filters(current_year: i32) -> Vec<FilterDefinition> {
    vec![
        select(
            "sort",
            "Sort order",
            &[
                ("Default", "default"),
                ("Latest Updated", "latest-updated"),
                ("Latest Added", "latest-added"),
                ("Score", "score"),
                ("Name A-Z", "name-az"),
                ("Release Date", "release-date"),
                ("Most Viewed", "most-viewed"),
                ("Number of episodes", "number_of_episodes"),
            ],
        ),
        check_group(
            "genre",
            "Genre",
            &[
                ("Action", "1"),
                ("Adventure", "2"),
                ("Comedy", "8"),
                ("Drama", "62"),
                ("Fantasy", "3"),
                ("Harem", "215"),
                ("Historical", "70"),
                ("Horror", "222"),
                ("Isekai", "74"),
                ("Magic", "203"),
                ("Martial Arts", "114"),
                ("Mecha", "123"),
                ("Military", "125"),
                ("Music", "242"),
                ("Mystery", "57"),
                ("Psychological", "73"),
                ("Romance", "28"),
                ("School", "14"),
                ("Sci-Fi", "12"),
                ("Seinen", "50"),
                ("Shoujo", "252"),
                ("Shounen", "15"),
                ("Slice of Life", "35"),
                ("Sports", "29"),
                ("Super Power", "16"),
                ("Supernatural", "9"),
                ("Thriller", "54"),
            ],
        ),
        check_group(
            "season",
            "Season",
            &[
                ("Fall", "fall"),
                ("Summer", "summer"),
                ("Spring", "spring"),
                ("Winter", "winter"),
            ],
        ),
        FilterDefinition::Group {
            id: "year".into(),
            name: "Year".into(),
            filters: (1980..=current_year + 1)
                .rev()
                .map(|year| FilterDefinition::CheckBox {
                    id: year.to_string(),
                    name: year.to_string(),
                    default: false,
                })
                .collect(),
        },
        check_group(
            "term_type",
            "Type",
            &[
                ("Movie", "Movie"),
                ("TV", "TV"),
                ("OVA", "OVA"),
                ("ONA", "ONA"),
                ("Special", "Special"),
                ("Music", "Music"),
            ],
        ),
        check_group(
            "status",
            "Status",
            &[
                ("Finished Airing", "finished-airing"),
                ("Currently Airing", "currently-airing"),
                ("Not Yet Aired", "not-yet-aired"),
            ],
        ),
        check_group("language", "Language", &[("Sub", "sub"), ("Dub", "dub")]),
        check_group(
            "rating",
            "Rating",
            &[
                ("PG", "PG"),
                ("PG-13", "PG-13"),
                ("G", "G"),
                ("R", "R"),
                ("R+", "R+"),
            ],
        ),
    ]
}

fn anikoto_preferences<C: AnikotoConfig>() -> Vec<PreferenceDefinition> {
    vec![
        PreferenceDefinition::Select {
            key: "domain".into(),
            title: "Preferred domain".into(),
            options: C::DOMAINS
                .iter()
                .map(|host| OptionItem {
                    label: (*host).into(),
                    value: format!("https://{host}"),
                })
                .collect(),
            default: C::BASE_URL.into(),
        },
        PreferenceDefinition::Select {
            key: "quality".into(),
            title: "Preferred quality".into(),
            options: options(&[
                ("1080p", "1080p"),
                ("720p", "720p"),
                ("480p", "480p"),
                ("360p", "360p"),
            ]),
            default: "1080p".into(),
        },
        PreferenceDefinition::Select {
            key: "server".into(),
            title: "Preferred server".into(),
            options: C::HOSTERS
                .iter()
                .map(|value| OptionItem {
                    label: (*value).into(),
                    value: (*value).into(),
                })
                .collect(),
            default: C::HOSTERS.first().copied().unwrap_or_default().into(),
        },
        PreferenceDefinition::MultiSelect {
            key: "servers".into(),
            title: "Enabled servers".into(),
            summary: None,
            options: C::HOSTERS
                .iter()
                .map(|value| OptionItem {
                    label: (*value).into(),
                    value: (*value).into(),
                })
                .collect(),
            default: C::HOSTERS.iter().map(|value| (*value).into()).collect(),
        },
        PreferenceDefinition::Select {
            key: "type".into(),
            title: "Preferred language".into(),
            options: options(&[
                ("Sub", "Sub"),
                ("Hard Sub", "HSub"),
                ("Dub", "Dub"),
                ("Alternate Dub", "A-Dub"),
            ]),
            default: "Sub".into(),
        },
        PreferenceDefinition::MultiSelect {
            key: "types".into(),
            title: "Enabled languages".into(),
            summary: None,
            options: options(&[
                ("Sub", "Sub"),
                ("Hard Sub", "HSub"),
                ("Dub", "Dub"),
                ("Alternate Dub", "A-Dub"),
            ]),
            default: vec!["Sub".into(), "HSub".into(), "Dub".into(), "A-Dub".into()],
        },
    ]
}

fn select(id: &str, name: &str, values: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Select {
        id: id.into(),
        name: name.into(),
        options: options(values),
        default_index: 0,
    }
}

fn check_group(id: &str, name: &str, values: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Group {
        id: id.into(),
        name: name.into(),
        filters: values
            .iter()
            .map(|(label, value)| FilterDefinition::CheckBox {
                id: (*value).into(),
                name: (*label).into(),
                default: false,
            })
            .collect(),
    }
}

fn options(values: &[(&str, &str)]) -> Vec<OptionItem> {
    values
        .iter()
        .map(|(label, value)| OptionItem {
            label: (*label).into(),
            value: (*value).into(),
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HosterData {
    #[serde(rename = "type")]
    kind: String,
    server_id: String,
    server_name: String,
    episode_path: String,
}

#[derive(Debug, Deserialize)]
struct ResultResponse {
    result: String,
}

#[derive(Debug, Deserialize)]
struct ServerResponse {
    result: ServerResult,
}

#[derive(Debug, Deserialize)]
struct ServerResult {
    url: String,
    #[serde(default, rename = "skip_data")]
    skip_data: Option<SkipData>,
}

#[derive(Debug, Deserialize)]
struct SkipData {
    #[serde(default)]
    intro: Vec<f64>,
    #[serde(default)]
    outro: Vec<f64>,
}

#[derive(Debug, Deserialize)]
struct SourceResponse {
    sources: Value,
    #[serde(default)]
    tracks: Option<Vec<SourceTrack>>,
}

#[derive(Debug, Deserialize)]
struct SourceTrack {
    file: String,
    kind: String,
    #[serde(default)]
    label: String,
}

fn result_fragment(source: &str) -> Result<String> {
    serde_json::from_str::<ResultResponse>(source)
        .map(|response| response.result)
        .map_err(Error::from)
}

fn source_url(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Object(value) => value.get("file").and_then(Value::as_str).map(str::to_owned),
        Value::Array(value) => value.first().and_then(source_url),
        _ => None,
    }
}

fn segment(values: &[f64]) -> Option<MediaSegment> {
    (values.len() >= 2 && values[1] > values[0]).then(|| MediaSegment {
        start_seconds: values[0],
        end_seconds: values[1],
    })
}

fn sort_streams(mut streams: Vec<VideoStream>) -> Vec<VideoStream> {
    let quality = context::preference::<String>("quality")
        .ok()
        .flatten()
        .unwrap_or_else(|| "1080p".into());
    streams.sort_by_key(|stream| {
        !stream
            .name
            .as_deref()
            .unwrap_or_default()
            .contains(&quality)
    });
    streams.sort_by_key(|stream| !stream.preferred);
    streams
}

fn type_label(label: &str, data_type: &str) -> String {
    match label.trim().to_ascii_lowercase().as_str() {
        "sub" => "Sub".into(),
        "h-sub" => "H-Sub".into(),
        "hsub" => "HSub".into(),
        "dub" => "Dub".into(),
        "a-dub" | "adub" => "A-Dub".into(),
        "s-sub" => "S-Sub".into(),
        _ => match data_type.to_ascii_lowercase().as_str() {
            "sub" => "Sub".into(),
            "hsub" => "HSub".into(),
            "dub" => "Dub".into(),
            "adub" => "A-Dub".into(),
            "" => {
                if label.is_empty() {
                    "Unknown".into()
                } else {
                    label.into()
                }
            }
            value => value.into(),
        },
    }
}

fn mapper_name(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "gogoanime" => "Vidstream".into(),
        "anivibe" => "Vibe-Stream".into(),
        "animepahe" => "Kiwi-Stream".into(),
        _ => value.into(),
    }
}

fn strip_episode_suffix(value: &str) -> String {
    Regex::new(r"(?i)/ep-[0-9]+(?:\.[0-9]+)?/?$")
        .unwrap()
        .replace(value, "")
        .into_owned()
}

fn split_episode_path(value: &str) -> (String, Option<f32>) {
    let number = Regex::new(r"(?i)/ep-([0-9]+(?:\.[0-9]+)?)/?$")
        .unwrap()
        .captures(value)
        .and_then(|captures| captures.get(1))
        .and_then(|value| value.as_str().parse().ok());
    (strip_episode_suffix(value), number)
}

fn number_label(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

fn extra_string(episode: &VideoEpisode, key: &str) -> Result<String> {
    episode
        .extra
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::new(format!("episode is missing {key}")))
}

fn is_hls_url(value: &str) -> bool {
    value.contains(".m3u8") && !value.contains("/stream/")
}

fn capture(source: &str, pattern: &str, group: usize) -> Option<String> {
    Regex::new(pattern)
        .ok()?
        .captures(source)?
        .get(group)
        .map(|value| value.as_str().to_owned())
}

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn ensure_depth(depth: u8) -> Result<()> {
    if depth > 3 {
        Err(Error::new("player iframe nesting exceeded the safe limit"))
    } else {
        Ok(())
    }
}

fn host(value: &str) -> Result<String> {
    Url::parse(value)
        .map_err(url_error)?
        .host_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::new("URL has no host"))
}

fn selector(value: &str) -> Result<Selector> {
    html::selector(value)
}
fn attr(element: ElementRef<'_>, name: &str) -> Option<String> {
    html::attribute(element, name)
}
fn first_element<'a>(document: &'a Html, value: &str) -> Option<ElementRef<'a>> {
    selector(value)
        .ok()
        .and_then(|selector| document.select(&selector).next())
}
fn first_text(document: &Html, value: &str) -> Option<String> {
    first_element(document, value)
        .map(html::text)
        .filter(|value| !value.is_empty())
}
fn first_attr(document: &Html, value: &str, name: &str) -> Option<String> {
    first_element(document, value).and_then(|element| attr(element, name))
}
fn metadata_links(document: &Html, label: &str) -> Result<Vec<String>> {
    let containers = selector("div")?;
    let links = selector("span > a")?;
    for container in document.select(&containers) {
        let value = normalize_space(&html::text(container));
        if value.starts_with(label) {
            let entries = container
                .select(&links)
                .map(html::text)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            if !entries.is_empty() {
                return Ok(entries);
            }
        }
    }
    Ok(Vec::new())
}
fn metadata_value(document: &Html, label: &str) -> Result<Option<String>> {
    let containers = selector("div")?;
    let span = selector("span")?;
    for container in document.select(&containers) {
        let value = normalize_space(&html::text(container));
        if value.starts_with(label) {
            if let Some(value) = container.select(&span).next().map(html::text) {
                if !value.is_empty() {
                    return Ok(Some(value));
                }
            }
        }
    }
    Ok(None)
}
fn extract_decimal(value: &str) -> Option<f32> {
    Regex::new(r"[0-9]+(?:\.[0-9]+)?")
        .ok()?
        .find(value)?
        .as_str()
        .parse()
        .ok()
}
fn has_class(element: ElementRef<'_>, name: &str) -> bool {
    element.value().classes().any(|value| value == name)
}
fn url_error(error: url::ParseError) -> Error {
    Error::new(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AlternateLayout;

    impl AnikotoConfig for AlternateLayout {
        const NAME: &'static str = "Alternate";
        const LANG: &'static str = "en";
        const BASE_URL: &'static str = "https://alternate.invalid";
        const DOMAINS: &'static [&'static str] = &["alternate.invalid"];
        const HOSTERS: &'static [&'static str] = &["Kiwi-Stream"];

        fn listing_thumbnail_selector() -> &'static str {
            "a.poster img"
        }

        fn detail_thumbnail_selector() -> &'static str {
            "section#w-info div.poster img"
        }

        fn synopsis_content_selector() -> &'static str {
            "div.synopsis > div.content"
        }

        fn score_label() -> &'static str {
            "Scores"
        }

        fn episode_list_selector() -> &'static str {
            "ul.episodes > li > a"
        }

        fn server_group_selector() -> &'static str {
            "div.type"
        }

        fn server_item_selector() -> &'static str {
            "a.server"
        }

        fn server_name_selector() -> Option<&'static str> {
            Some("span")
        }

        fn canonical_server_name(raw: &str) -> String {
            if raw.to_ascii_lowercase().starts_with("server") {
                let suffix = raw.get("Server".len()..).unwrap_or_default().trim();
                let suffix = if suffix.is_empty() {
                    String::new()
                } else {
                    format!(" {suffix}")
                };
                return format!("Kiwi-Stream{suffix}");
            }
            raw.trim_end_matches(['-', ' ']).to_owned()
        }

        fn server_matches(configured: &str, actual: &str) -> bool {
            configured.eq_ignore_ascii_case(actual)
                || actual.strip_prefix(configured).is_some_and(|suffix| {
                    let suffix = suffix.trim();
                    !suffix.is_empty() && suffix.chars().all(|value| value.is_ascii_digit())
                })
        }

        fn map_filter_value(key: &str, value: &str) -> String {
            if key == "rating" {
                value.to_ascii_lowercase().replace('-', "_")
            } else {
                value.to_owned()
            }
        }

        fn should_generate_search_vrf(query: &str) -> bool {
            !query.trim().is_empty()
        }
    }

    #[test]
    fn constructs_listing_search_and_vrf_urls() {
        assert_eq!(
            listing_url("https://animewave.to", "most-viewed", 2).unwrap(),
            "https://animewave.to/most-viewed/?page=2"
        );
        let url = search_url(
            "https://animewave.to",
            "one piece",
            3,
            &json!({"genre":["1","2"],"language":["sub"],"sort":"score"}),
        )
        .unwrap();
        assert!(url.contains("keyword=one+piece"));
        assert!(url.contains("genre%5B%5D=1"));
        assert!(url.contains("language%5B%5D=sub"));
        assert_ne!(vrf_encrypt("1642"), "1642");
    }

    #[test]
    fn parses_list_details_episodes_and_servers() {
        let page = parse_listing_html(
            include_str!("../tests/fixtures/list.html"),
            "https://animewave.to/",
        )
        .unwrap();
        assert_eq!(page.entries[0].title, "Example Anime");
        assert_eq!(page.entries[0].key, "/watch/example-anime-abcd");
        assert!(page.has_next_page);
        let details = parse_details_html(
            include_str!("../tests/fixtures/details.html"),
            "https://animewave.to/watch/example-anime-abcd",
        )
        .unwrap();
        assert_eq!(details.extra["animeId"], "42");
        assert_eq!(details.tags, ["Action", "Fantasy"]);
        let episodes = parse_episodes_json(
            include_str!("../tests/fixtures/episodes.json"),
            "https://animewave.to/watch/example-anime-abcd",
        )
        .unwrap();
        assert_eq!(episodes.len(), 2);
        assert_eq!(episodes[0].episode_number, Some(1.0));
        let fragment = result_fragment(include_str!("../tests/fixtures/servers.json")).unwrap();
        let hosters = parse_server_list_html(&fragment, "/watch/example-anime-abcd/ep-1").unwrap();
        assert_eq!(hosters[0].name, "VidPlay-1 - Sub");
    }

    #[test]
    fn rejects_malformed_required_payloads() {
        assert!(parse_details_html("<html></html>", "https://animewave.to/watch/missing").is_err());
        assert!(parse_episodes_json("{}", "https://animewave.to/watch/missing").is_err());
        assert!(ensure_depth(4).is_err());
    }

    #[test]
    fn enforces_play_safe_ratings_and_rejects_adult_details() {
        let filters = play_safe_filters(&json!({
            "genre": ["1", "214"],
            "rating": ["Rx"]
        }))
        .unwrap();
        assert_eq!(filters["genre"], json!(["1"]));
        assert_eq!(filters["rating"], json!(PLAY_ALLOWED_RATINGS));

        let details = parse_details_html(
            r#"<html><h1 class="title">Blocked</h1><div>Genres <span><a>Hentai</a></span></div></html>"#,
            "https://animewave.to/watch/blocked",
        )
        .unwrap();
        assert!(reject_blocked_details(&details).is_err());
        assert!(!anikoto_filters(2027)
            .iter()
            .any(|filter| { serde_json::to_string(filter).unwrap().contains("Rx") }));
    }

    #[test]
    fn expresses_host_owned_segment_processing() {
        let rules = anikoto_segment_processing().rules;
        assert_eq!(rules[0].strip_prefix_bytes, Some(252));
        assert!(rules[1].auto_detect_media_offset);
    }

    #[test]
    fn supports_alternate_theme_selectors_and_normalization() {
        let listing = parse_listing_html_for::<AlternateLayout>(
            r#"<div class="ani items"><div class="item"><a class="name" href="/watch/a">A</a><a class="poster"><img data-src="/a.jpg"></a></div></div>"#,
            AlternateLayout::BASE_URL,
        )
        .unwrap();
        assert_eq!(
            listing.entries[0]
                .cover
                .as_ref()
                .map(|request| request.url.as_str()),
            Some("https://alternate.invalid/a.jpg")
        );

        let episodes = parse_episodes_json_for::<AlternateLayout>(
            r#"{"result":"<ul class='episodes'><li><a data-num='1' data-ids='7'></a></li></ul>"}"#,
            "https://alternate.invalid/watch/a",
        )
        .unwrap();
        assert_eq!(episodes.len(), 1);

        let hosters = parse_server_list_html_for::<AlternateLayout>(
            r#"<div class="type" data-type="sub"><a class="server" data-link-id="9"><span>Server 2</span></a></div>"#,
            "/watch/a/ep-1",
        )
        .unwrap();
        assert_eq!(hosters[0].name, "Kiwi-Stream 2 - Sub");
        assert!(AlternateLayout::server_matches(
            "Kiwi-Stream",
            "Kiwi-Stream 2"
        ));

        let search = search_url_for::<AlternateLayout>(
            AlternateLayout::BASE_URL,
            " ",
            1,
            &json!({"rating": ["PG-13"]}),
        )
        .unwrap();
        assert!(search.contains("rating%5B%5D=pg_13"));
        assert!(search.contains("vrf="));
    }
}
