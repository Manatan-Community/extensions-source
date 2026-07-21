use std::collections::BTreeMap;

use manatan_sdk::{
    client::Client, CatalogItem, Error, FilterDefinition, ImageRequest, MediaTrack, OptionItem,
    Paged, Result, UrlResolveResult, VideoEpisode, VideoSource, VideoStream,
};
use regex::Regex;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use url::Url;

const SITE_URL: &str = "https://www.animeonsen.xyz";
const API_URL: &str = "https://api.animeonsen.xyz/v4";
const AUTH_URL: &str = "https://auth.animeonsen.xyz/oauth/token";
const SEARCH_URL: &str = "https://search.animeonsen.xyz/indexes/content/search";
const CLIENT_ID: &str = "f296be26-28b5-4358-b5a1-6259575e23b7";
const CLIENT_SECRET: &str = "349038c4157d0480784753841217270c3c5b35f4281eaee029de21cb04084235";
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Mobile Safari/537.36";

#[derive(Default)]
pub struct AnimeOnsenSource {
    access_token: Option<String>,
    search_token: Option<String>,
}

impl AnimeOnsenSource {
    fn client(&self) -> Client {
        Client::browser()
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Referer", format!("{SITE_URL}/"))
            .header("Origin", SITE_URL)
    }

    fn fetch_access_token(&self) -> Result<String> {
        let response = self
            .client()
            .post(AUTH_URL)
            .form(&[
                ("client_id", CLIENT_ID),
                ("client_secret", CLIENT_SECRET),
                ("grant_type", "client_credentials"),
            ])
            .timeout_ms(20_000)
            .max_body_bytes(128 * 1024)
            .send()?
            .error_for_status()?;
        let token: TokenResponse = response.json()?;
        if token.access_token.trim().is_empty() {
            return Err(Error::new("AnimeOnsen returned an empty access token"));
        }
        Ok(token.access_token)
    }

    fn api_json<T: DeserializeOwned>(&mut self, url: &str) -> Result<T> {
        validate_api_url(url)?;
        if self.access_token.is_none() {
            self.access_token = Some(self.fetch_access_token()?);
        }
        let mut response = self
            .client()
            .get(url)
            .header(
                "Authorization",
                format!(
                    "Bearer {}",
                    self.access_token.as_deref().unwrap_or_default()
                ),
            )
            .rate_limit("animeonsen-api", 150)
            .timeout_ms(30_000)
            .max_body_bytes(8 * 1024 * 1024)
            .send()?;
        if response.status() == 401 {
            self.access_token = Some(self.fetch_access_token()?);
            response = self
                .client()
                .get(url)
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        self.access_token.as_deref().unwrap_or_default()
                    ),
                )
                .rate_limit("animeonsen-api", 150)
                .timeout_ms(30_000)
                .max_body_bytes(8 * 1024 * 1024)
                .send()?;
        }
        response.error_for_status()?.json()
    }

    fn fetch_search_token(&self) -> Result<String> {
        let response = self
            .client()
            .get(SITE_URL)
            .timeout_ms(20_000)
            .max_body_bytes(2 * 1024 * 1024)
            .send()?
            .error_for_status()?;
        search_token_from_html(response.text()?)
            .ok_or_else(|| Error::new("AnimeOnsen search token was not present"))
    }

    fn search_items(&mut self, query: &str) -> Result<Vec<ListItem>> {
        if self.search_token.is_none() {
            self.search_token = Some(self.fetch_search_token()?);
        }
        let body = serde_json::json!({ "q": query });
        for attempt in 0..2 {
            let response = self
                .client()
                .post(SEARCH_URL)
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        self.search_token.as_deref().unwrap_or_default()
                    ),
                )
                .json(&body)?
                .timeout_ms(20_000)
                .max_body_bytes(4 * 1024 * 1024)
                .send()?;
            if response.status() != 401 {
                return Ok(response.error_for_status()?.json::<SearchResponse>()?.hits);
            }
            if attempt == 0 {
                self.search_token = Some(self.fetch_search_token()?);
            }
        }
        Err(Error::new("AnimeOnsen search authorization failed"))
    }

    fn safe_details(&mut self, id: &str) -> Result<Details> {
        let id = validate_content_id(id)?;
        let details: Details = self.api_json(&format!("{API_URL}/content/{id}/extensive"))?;
        ensure_safe_details(&details)?;
        Ok(details)
    }

    fn filter_safe_items(&mut self, items: Vec<ListItem>) -> Vec<CatalogItem> {
        items
            .into_iter()
            .filter_map(|entry| {
                let details = self.safe_details(&entry.content_id).ok()?;
                Some(catalog_from_details(&details, Some(&entry)))
            })
            .collect()
    }

    fn index(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let start = page.saturating_sub(1).saturating_mul(30);
        let response: ListResponse =
            self.api_json(&format!("{API_URL}/content/index?start={start}&limit=30"))?;
        let has_next_page = response
            .cursor
            .next
            .first()
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(Paged::new(
            self.filter_safe_items(response.content),
            has_next_page,
        ))
    }
}

impl VideoSource for AnimeOnsenSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.index(page.max(1))
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        if query.trim().is_empty() {
            if let Some(genre) = selected_genre(filters) {
                let response: GenreResponse =
                    self.api_json(&format!("{API_URL}/content/index/genre/{genre}"))?;
                return Ok(Paged::new(self.filter_safe_items(response.result), false));
            }
            return self.index(page.max(1));
        }
        if page > 1 {
            return Ok(Paged::default());
        }
        let entries = self.search_items(query.trim())?;
        Ok(Paged::new(self.filter_safe_items(entries), false))
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![FilterDefinition::Select {
            id: "genre".to_string(),
            name: "Genre".to_string(),
            options: safe_genres()
                .iter()
                .map(|(label, value)| OptionItem {
                    label: (*label).to_string(),
                    value: (*value).to_string(),
                })
                .collect(),
            default_index: 0,
        }])
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let details = self.safe_details(&item.key)?;
        Ok(catalog_from_details(&details, None))
    }

    fn episodes(&mut self, item: CatalogItem) -> Result<Vec<VideoEpisode>> {
        let details = self.safe_details(&item.key)?;
        let id = validate_content_id(&details.content_id)?;
        let episodes: BTreeMap<String, EpisodeDto> =
            self.api_json(&format!("{API_URL}/content/{id}/episodes"))?;
        let mut entries = episodes
            .into_iter()
            .map(|(number, episode)| VideoEpisode {
                key: format!("{id}/video/{number}"),
                title: Some(episode_title(&number, &episode)),
                episode_number: number.parse::<f32>().ok(),
                url: Some(format!("{SITE_URL}/watch/{id}/{number}")),
                language: Some("all".to_string()),
                ..VideoEpisode::default()
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right
                .episode_number
                .partial_cmp(&left.episode_number)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(entries)
    }

    fn streams(&mut self, item: CatalogItem, episode: VideoEpisode) -> Result<Vec<VideoStream>> {
        self.safe_details(&item.key)?;
        let id = validate_content_id(&item.key)?;
        let prefix = format!("{id}/video/");
        let episode_number = episode
            .key
            .strip_prefix(&prefix)
            .ok_or_else(|| Error::new("episode does not belong to this AnimeOnsen title"))?;
        validate_episode_number(episode_number)?;
        let payload: VideoData =
            self.api_json(&format!("{API_URL}/content/{id}/video/{episode_number}"))?;
        validate_stream_url(&payload.uri.stream)?;
        let subtitle_headers = api_media_headers(self.access_token.as_deref().unwrap_or_default());
        let mut subtitles = payload
            .uri
            .subtitles
            .into_iter()
            .map(|(language, url)| {
                validate_subtitle_url(&url)?;
                let label = payload
                    .metadata
                    .subtitles
                    .get(&language)
                    .cloned()
                    .unwrap_or_else(|| language.clone());
                Ok(MediaTrack {
                    url,
                    language: Some(language.clone()),
                    label: Some(label),
                    format: Some("vtt".to_string()),
                    headers: subtitle_headers.clone(),
                    is_default: language.eq_ignore_ascii_case("en-US"),
                    ..MediaTrack::default()
                })
            })
            .collect::<Result<Vec<_>>>()?;
        subtitles.sort_by_key(|track| !track.is_default);
        let is_hls = payload.uri.stream.contains(".m3u8");
        Ok(vec![VideoStream {
            url: payload.uri.stream,
            name: Some("AnimeOnsen".to_string()),
            quality: Some("720p".to_string()),
            resolution: Some("1280x720".to_string()),
            format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
            is_hls,
            preferred: true,
            initialized: true,
            headers: media_headers(),
            subtitles,
            ..VideoStream::default()
        }])
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let url = match Url::parse(candidate) {
            Ok(url) => url,
            Err(_) => return Ok(None),
        };
        if url.scheme() != "https" || url.host_str() != Some("www.animeonsen.xyz") {
            return Ok(None);
        }
        let path = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
        if path.len() != 2 || path[0] != "details" {
            return Ok(None);
        }
        let details = self.safe_details(path[1])?;
        Ok(Some(UrlResolveResult {
            item: Some(catalog_from_details(&details, None)),
            ..UrlResolveResult::default()
        }))
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ListResponse {
    #[serde(default)]
    content: Vec<ListItem>,
    #[serde(default)]
    cursor: Cursor,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Cursor {
    #[serde(default)]
    next: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    hits: Vec<ListItem>,
}

#[derive(Clone, Debug, Deserialize)]
struct GenreResponse {
    #[serde(default)]
    result: Vec<ListItem>,
}

#[derive(Clone, Debug, Deserialize)]
struct ListItem {
    content_id: String,
    #[serde(default)]
    content_title: Option<String>,
    #[serde(default)]
    content_title_en: Option<String>,
    #[serde(default)]
    content_title_jp: Option<String>,
    #[serde(default)]
    content_image: Option<String>,
    #[serde(default)]
    thumbnail: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct Details {
    content_id: String,
    #[serde(default)]
    content_title: Option<String>,
    #[serde(default)]
    content_title_en: Option<String>,
    #[serde(default)]
    mal_id: Option<u64>,
    #[serde(default)]
    mal_data: Value,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct MalData {
    #[serde(default)]
    genres: Vec<NamedValue>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    studios: Vec<NamedValue>,
    #[serde(default)]
    synopsis: Option<String>,
    #[serde(default)]
    mean_score: Option<f32>,
    #[serde(default)]
    rating: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct NamedValue {
    name: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct EpisodeDto {
    #[serde(default, rename = "contentTitle_episode_en")]
    name_en: Option<String>,
    #[serde(default, rename = "contentTitle_episode_jp")]
    name_jp: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct VideoData {
    metadata: VideoMetadata,
    uri: VideoUri,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct VideoMetadata {
    #[serde(default)]
    subtitles: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
struct VideoUri {
    stream: String,
    #[serde(default)]
    subtitles: BTreeMap<String, String>,
}

fn mal_data(details: &Details) -> Result<MalData> {
    if !details.mal_data.is_object() {
        return Err(Error::new("AnimeOnsen title has no classifiable metadata"));
    }
    serde_json::from_value(details.mal_data.clone())
        .map_err(|error| Error::new(format!("invalid AnimeOnsen metadata: {error}")))
}

fn ensure_safe_details(details: &Details) -> Result<()> {
    let metadata = mal_data(details)?;
    let rating = metadata
        .rating
        .as_deref()
        .map(normalize_label)
        .ok_or_else(|| Error::new("AnimeOnsen title has no content rating"))?;
    if !matches!(rating.as_str(), "g" | "pg" | "pg13") {
        return Err(Error::new(
            "AnimeOnsen title is outside the Play-safe rating boundary",
        ));
    }
    let blocked = [
        "ecchi", "erotic", "explicit", "hentai", "porn", "smut", "rx", "r18",
    ];
    if metadata
        .genres
        .iter()
        .map(|genre| normalize_label(&genre.name))
        .any(|genre| genre == "adult" || blocked.iter().any(|blocked| genre.contains(blocked)))
    {
        return Err(Error::new("AnimeOnsen title has a blocked genre"));
    }
    Ok(())
}

fn catalog_from_details(details: &Details, listing: Option<&ListItem>) -> CatalogItem {
    let metadata = mal_data(details).unwrap_or_default();
    let mut description = metadata.synopsis;
    if let Some(score) = metadata.mean_score {
        description = Some(format!(
            "Score: {score:.1}/10\n\n{}",
            description.unwrap_or_default()
        ));
    }
    let thumbnail = listing
        .and_then(|item| {
            item.thumbnail
                .clone()
                .or_else(|| item.content_image.clone())
        })
        .filter(|url| validate_cover_url(url).is_ok())
        .unwrap_or_else(|| format!("{API_URL}/image/210x300/{}", details.content_id));
    let title = details
        .content_title_en
        .clone()
        .or_else(|| details.content_title.clone())
        .or_else(|| listing.and_then(|item| item.content_title_en.clone()))
        .or_else(|| listing.and_then(|item| item.content_title.clone()))
        .or_else(|| listing.and_then(|item| item.content_title_jp.clone()))
        .unwrap_or_else(|| details.content_id.clone());
    let mut item = CatalogItem::new(&details.content_id, title);
    item.url = Some(format!("{SITE_URL}/details/{}", details.content_id));
    item.cover = Some(ImageRequest::get(thumbnail).header("Referer", format!("{SITE_URL}/")));
    item.description = description;
    item.authors = metadata
        .studios
        .into_iter()
        .map(|studio| studio.name)
        .collect();
    item.tags = metadata
        .genres
        .into_iter()
        .map(|genre| genre.name)
        .collect();
    item.status = metadata.status.map(Value::String);
    item.rating = metadata.mean_score.map(|score| score / 2.0);
    item.content_rating = Some("suggestive".to_string());
    item.language = Some("all".to_string());
    item.initialized = true;
    if let Some(mal_id) = details.mal_id {
        item.extra.insert("malId".to_string(), Value::from(mal_id));
    }
    item
}

fn episode_title(number: &str, episode: &EpisodeDto) -> String {
    let title = episode
        .name_en
        .as_deref()
        .or(episode.name_jp.as_deref())
        .unwrap_or_default();
    if title.is_empty() {
        format!("Episode {number}")
    } else {
        format!("Episode {number}: {title}")
    }
}

fn normalize_label(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn validate_content_id(value: &str) -> Result<&str> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(Error::new("invalid AnimeOnsen content id"));
    }
    Ok(value)
}

fn validate_episode_number(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'-'))
    {
        return Err(Error::new("invalid AnimeOnsen episode number"));
    }
    Ok(())
}

fn validate_api_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(|error| Error::new(error.to_string()))?;
    if url.scheme() != "https" || url.host_str() != Some("api.animeonsen.xyz") {
        return Err(Error::new(
            "AnimeOnsen API request left the approved origin",
        ));
    }
    Ok(())
}

fn validate_cover_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(|error| Error::new(error.to_string()))?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("api.animeonsen.xyz") | Some("cdn.animeonsen.xyz")
        )
    {
        return Err(Error::new("AnimeOnsen cover left the approved origins"));
    }
    Ok(())
}

fn validate_stream_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(|error| Error::new(error.to_string()))?;
    if url.scheme() != "https" || url.host_str() != Some("cdn.animeonsen.xyz") {
        return Err(Error::new(
            "AnimeOnsen returned an unapproved stream origin",
        ));
    }
    Ok(())
}

fn validate_subtitle_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(|error| Error::new(error.to_string()))?;
    if url.scheme() != "https" || url.host_str() != Some("api.animeonsen.xyz") {
        return Err(Error::new(
            "AnimeOnsen returned an unapproved subtitle origin",
        ));
    }
    Ok(())
}

fn media_headers() -> BTreeMap<String, String> {
    [
        ("Referer".to_string(), format!("{SITE_URL}/")),
        ("Origin".to_string(), SITE_URL.to_string()),
        ("User-Agent".to_string(), USER_AGENT.to_string()),
    ]
    .into_iter()
    .collect()
}

fn api_media_headers(access_token: &str) -> BTreeMap<String, String> {
    let mut headers = media_headers();
    if !access_token.is_empty() {
        headers.insert(
            "Authorization".to_string(),
            format!("Bearer {access_token}"),
        );
    }
    headers
}

fn search_token_from_html(html: &str) -> Option<String> {
    let regex = Regex::new(
        r#"(?is)<meta[^>]+name=[\"']ao-search-token[\"'][^>]+content=[\"']([a-f0-9]{32,128})[\"']"#,
    )
    .ok()?;
    regex
        .captures(html)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_string())
}

fn selected_genre(filters: &Value) -> Option<&str> {
    let candidate = filters.get("genre")?.as_str()?;
    safe_genres()
        .iter()
        .any(|(_, value)| *value == candidate && !value.is_empty())
        .then_some(candidate)
}

fn safe_genres() -> &'static [(&'static str, &'static str)] {
    &[
        ("All", ""),
        ("Action", "action"),
        ("Adult Cast", "adult-cast"),
        ("Adventure", "adventure"),
        ("Anthropomorphic", "anthropomorphic"),
        ("Avant Garde", "avant-garde"),
        ("Award Winning", "award-winning"),
        ("Childcare", "childcare"),
        ("Combat Sports", "combat-sports"),
        ("Comedy", "comedy"),
        ("Crossdressing", "crossdressing"),
        ("Cute Girls Doing Cute Things", "cgdct"),
        ("Delinquents", "delinquents"),
        ("Detective", "detective"),
        ("Drama", "drama"),
        ("Educational", "educational"),
        ("Fantasy", "fantasy"),
        ("Gag Humor", "gag-humor"),
        ("Gore", "gore"),
        ("Gourmet", "gourmet"),
        ("Harem", "harem"),
        ("High Stakes Game", "high-stakes-game"),
        ("Historical", "historical"),
        ("Horror", "horror"),
        ("Idols (Female)", "idols-female"),
        ("Idols (Male)", "idols-male"),
        ("Isekai", "isekai"),
        ("Iyashikei", "iyashikei"),
        ("Love Polygon", "love-polygon"),
        ("Josei", "josei"),
        ("Kids", "kids"),
        ("Magical Sex Shift", "magical-sex-shift"),
        ("Mahou Shoujo", "mahou-shoujo"),
        ("Martial Arts", "martial-arts"),
        ("Mecha", "mecha"),
        ("Medical", "medical"),
        ("Military", "military"),
        ("Music", "music"),
        ("Mystery", "mystery"),
        ("Mythology", "mythology"),
        ("Organized Crime", "organized-crime"),
        ("Otaku Culture", "otaku-culture"),
        ("Parody", "parody"),
        ("Performing Arts", "performing-arts"),
        ("Pets", "pets"),
        ("Psychological", "psychological"),
        ("Racing", "racing"),
        ("Reincarnation", "reincarnation"),
        ("Reverse Harem", "reverse-harem"),
        ("Romance", "romance"),
        ("Romantic Subtext", "romantic-subtext"),
        ("Samurai", "samurai"),
        ("School", "school"),
        ("Sci-Fi", "sci-fi"),
        ("Seinen", "seinen"),
        ("Shoujo", "shoujo"),
        ("Shoujo Ai", "shoujo-ai"),
        ("Shounen", "shounen"),
        ("Shounen Ai", "shounen-ai"),
        ("Showbiz", "showbiz"),
        ("Slice of Life", "slice-of-life"),
        ("Space", "space"),
        ("Sports", "sports"),
        ("Strategy Game", "strategy-game"),
        ("Super Power", "super-power"),
        ("Supernatural", "supernatural"),
        ("Survival", "survival"),
        ("Team Sports", "team-sports"),
        ("Suspense", "suspense"),
        ("Time Travel", "time-travel"),
        ("Vampire", "vampire"),
        ("Video Game", "video-game"),
        ("Visual Arts", "visual-arts"),
        ("Workplace", "workplace"),
    ]
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().video("animeonsen", AnimeOnsenSource::default())
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_safe_fixture_and_builds_catalog() {
        let details: Details =
            serde_json::from_str(include_str!("../fixtures/detail-safe.json")).unwrap();
        ensure_safe_details(&details).unwrap();
        let item = catalog_from_details(&details, None);
        assert_eq!(item.key, "safeTitle123");
        assert_eq!(item.title, "Safe title");
        assert_eq!(item.content_rating.as_deref(), Some("suggestive"));
    }

    #[test]
    fn rejects_adult_rating_and_genre() {
        let adult: Details =
            serde_json::from_str(include_str!("../fixtures/detail-adult.json")).unwrap();
        assert!(ensure_safe_details(&adult).is_err());

        let ecchi: Details =
            serde_json::from_str(include_str!("../fixtures/detail-ecchi.json")).unwrap();
        assert!(ensure_safe_details(&ecchi).is_err());
    }

    #[test]
    fn rejects_unclassified_or_unapproved_origins() {
        let details: Details =
            serde_json::from_str(r#"{"content_id":"x","content_title":"X","mal_data":false}"#)
                .unwrap();
        assert!(ensure_safe_details(&details).is_err());
        assert!(validate_stream_url("https://evil.example/video.m3u8").is_err());
        assert!(validate_subtitle_url("http://api.animeonsen.xyz/sub.vtt").is_err());
    }

    #[test]
    fn extracts_only_bounded_hex_search_token() {
        assert_eq!(
            search_token_from_html(
                r#"<meta name="ao-search-token" content="0123456789abcdef0123456789abcdef">"#
            )
            .as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert!(search_token_from_html(
            r#"<meta name="ao-search-token" content="bad token; alert(1)">"#
        )
        .is_none());
    }

    #[test]
    fn validates_ids_and_episode_ownership_primitives() {
        assert!(validate_content_id("abc_123-Z").is_ok());
        assert!(validate_content_id("../escape").is_err());
        assert!(validate_episode_number("12.5").is_ok());
        assert!(validate_episode_number("1/../../x").is_err());
    }

    #[test]
    fn filter_list_excludes_ecchi_and_unknown_values() {
        assert!(!safe_genres().iter().any(|(_, value)| *value == "ecchi"));
        assert_eq!(
            selected_genre(&serde_json::json!({ "genre": "fantasy" })),
            Some("fantasy")
        );
        assert_eq!(
            selected_genre(&serde_json::json!({ "genre": "ecchi" })),
            None
        );
    }

    #[test]
    fn subtitle_headers_include_the_api_bearer_and_browser_context() {
        let headers = api_media_headers("token");
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer token")
        );
        assert_eq!(headers.get("Origin").map(String::as_str), Some(SITE_URL));
        assert_eq!(
            headers.get("User-Agent").map(String::as_str),
            Some(USER_AGENT)
        );
    }
}
