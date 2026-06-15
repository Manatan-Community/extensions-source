use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageFormat, RgbImage};
use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage, UrlResolveResult,
    abi::{ExtensionResult, cookies_get},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{
    manga,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha512};
use std::{
    collections::BTreeMap,
    io::Cursor,
    time::{SystemTime, UNIX_EPOCH},
};

const SOURCE: KManga = KManga;
const BASE_URL: &str = "https://kmanga.kodansha.com";
const API_URL: &str = "https://api.kmanga.kodansha.com";
const PAGE_LIMIT: u64 = 25;

struct KManga;

impl MangaSource for KManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_title_list(TITLE_LIST_FIXTURE, false));
        }
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(fetch_latest());
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let offset = (page.saturating_sub(1)) * PAGE_LIMIT;
        let ranking_url = format!(
            "{API_URL}/ranking/all?ranking_id=12&offset={offset}&limit={}",
            PAGE_LIMIT + 1
        );
        let ranking: RankingResponse = serde_json::from_str(&fetch_api_get(
            &ranking_url,
            &[
                ("ranking_id", "12"),
                ("offset", &offset.to_string()),
                ("limit", &(PAGE_LIMIT + 1).to_string()),
            ],
            RANKING_FIXTURE,
        ))
        .unwrap_or_default();
        let ids = ranking
            .ranking_title_list
            .into_iter()
            .map(|entry| entry.id.to_string())
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        let has_next_page = ids.len() > PAGE_LIMIT as usize;
        let fetch_ids = if has_next_page {
            &ids[..PAGE_LIMIT as usize]
        } else {
            &ids[..]
        };
        let ids_param = fetch_ids.join(",");
        let body = fetch_api_get(
            &format!(
                "{API_URL}/title/list?title_id_list={}",
                url::query_escape(&ids_param)
            ),
            &[("title_id_list", &ids_param)],
            TITLE_LIST_FIXTURE,
        );
        let mut page = parse_title_list(&body, has_next_page);
        page.entries.reverse();
        Ok(page)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let (target, pairs) = if !query.is_empty() {
            (
                format!(
                    "{API_URL}/search/title?keyword={}&limit=99999",
                    url::query_escape(query)
                ),
                vec![
                    ("keyword".to_string(), query.to_string()),
                    ("limit".to_string(), "99999".to_string()),
                ],
            )
        } else {
            let genre = request
                .get("filters")
                .and_then(|filters| filters.get("genre"))
                .and_then(Value::as_str)
                .unwrap_or("1");
            (
                format!("{API_URL}/search/title?genre_id={genre}&limit=99999"),
                vec![
                    ("genre_id".to_string(), genre.to_string()),
                    ("limit".to_string(), "99999".to_string()),
                ],
            )
        };
        let refs = pairs
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        Ok(parse_title_list(
            &fetch_api_get(&target, &refs, TITLE_LIST_FIXTURE),
            false,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/1".to_string());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/1".to_string());
        let detail_body = fetch_detail_body(&title_id(&key));
        let details: DetailResponse = serde_json::from_str(&detail_body).unwrap_or_default();
        let ids = details
            .web_title
            .episode_id_list
            .into_iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids_param = ids.join(",");
        let body = fetch_api_post(
            &format!("{API_URL}/episode/list"),
            &[("episode_id_list", &ids_param)],
            EPISODE_LIST_FIXTURE,
        );
        let payload: EpisodeListResponse = serde_json::from_str(&body).unwrap_or_default();
        let hide_locked = preference_bool(&request, "hide_locked");
        let mut chapters = payload
            .episode_list
            .into_iter()
            .filter(|episode| !hide_locked || !episode.is_locked())
            .map(Episode::into_chapter)
            .collect::<Vec<_>>();
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/title/1/episode/1".to_string());
        let episode = key
            .trim_matches('/')
            .split('/')
            .last()
            .unwrap_or("1")
            .to_string();
        let target = format!("{API_URL}/web/episode/viewer?episode_id={episode}");
        let body = fetch_api_get(&target, &[("episode_id", &episode)], VIEWER_FIXTURE);
        let payload: ViewerResponse = serde_json::from_str(&body).unwrap_or_default();
        Ok(payload
            .page_list
            .into_iter()
            .enumerate()
            .map(|(index, image)| {
                let mut extra = BTreeMap::new();
                extra.insert("scrambleSeed".to_string(), json!(payload.scramble_seed));
                MangaPage {
                    content: PageContent::Url {
                        url: image,
                        context: Some(manga::image_headers(BASE_URL)),
                    },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(format!("Page {}", index + 1)),
                    extra,
                    ..MangaPage::default()
                }
            })
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let item = if key.starts_with("/title/") && !key.contains("/episode/") {
                Some(details_by_key(&key))
            } else {
                None
            };
            return Ok(Some(UrlResolveResult {
                item,
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.to_string(),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let image_base64 = request
            .get("imageBase64")
            .or_else(|| request.get("image_base64"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let seed = request
            .get("page")
            .and_then(|page| page.get("extra"))
            .and_then(|extra| {
                extra
                    .get("scrambleSeed")
                    .or_else(|| extra.get("scramble_seed"))
            })
            .and_then(Value::as_u64)
            .or_else(|| request.get("scrambleSeed").and_then(Value::as_u64));
        let processed = seed
            .and_then(|seed| unscramble_base64_jpeg(image_base64, seed).ok())
            .unwrap_or_else(|| image_base64.to_string());
        Ok(ProcessedImage {
            image_base64: processed,
            mime_type: Some("image/jpeg".to_string()),
            ..ProcessedImage::default()
        })
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_header("x-kmanga-platform", "3")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api_get(target: &str, params: &[(&str, &str)], fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .header("x-kmanga-hash", generate_hash(params))
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_api_post(target: &str, form: &[(&str, &str)], fixture: &str) -> String {
    client()
        .post(target)
        .xhr()
        .header("x-kmanga-hash", generate_hash(form))
        .form(form)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_detail_body(id: &str) -> String {
    let target = format!("{API_URL}/web/title/detail?title_id={id}");
    fetch_api_get(&target, &[("title_id", id)], DETAIL_FIXTURE)
}

fn details_by_key(key: &str) -> CatalogItem {
    let id = title_id(key);
    let body = fetch_detail_body(&id);
    let details: DetailResponse = serde_json::from_str(&body).unwrap_or_default();
    let mut item = details.web_title.into_catalog(format!("/title/{id}"));
    if !item.extra.contains_key("id") {
        item.extra.insert("id".to_string(), json!(id));
    }
    item
}

fn fetch_latest() -> Paged<CatalogItem> {
    let mut entries = Vec::new();
    for offset in 0..7 {
        let date = latest_base_date(offset);
        let target = format!("{API_URL}/web/top/updated/title?base_date={date}");
        let body = fetch_api_get(&target, &[("base_date", &date)], TITLE_LIST_FIXTURE);
        let page = parse_title_list(&body, false);
        if page.entries.is_empty() {
            break;
        }
        entries.extend(page.entries.into_iter().rev());
    }
    Paged {
        entries,
        has_next_page: false,
    }
}

fn latest_base_date(day_offset: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(1_704_067_200);
    let jst = now + 9 * 3600;
    let hour = (jst.rem_euclid(86_400)) / 3600;
    let day = jst.div_euclid(86_400) - day_offset - if hour < 10 { 1 } else { 0 };
    let (year, month, day) = civil_from_days(day);
    format!("{year:04}-{month:02}-{day:02}")
}

fn parse_title_list(body: &str, has_next_page: bool) -> Paged<CatalogItem> {
    let payload: TitleListResponse = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: payload
            .title_list
            .into_iter()
            .map(TitleDetail::into_catalog)
            .collect(),
        has_next_page,
    }
}

fn normalize_key(value: &str) -> String {
    if value.starts_with(BASE_URL) {
        if let Some(index) = value.find("/title/") {
            return format!("/{}", value[index + 1..].trim_end_matches('/'));
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn title_id(key: &str) -> String {
    key.trim_matches('/')
        .split('/')
        .nth(1)
        .unwrap_or("1")
        .to_string()
}

fn preference_bool(request: &Value, key: &str) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn generate_hash(params: &[(&str, &str)]) -> String {
    let (birthday, expires) = birthday_cookie();
    let mut sorted = BTreeMap::new();
    for (key, value) in params {
        sorted.insert(*key, *value);
    }
    let joined = sorted
        .into_iter()
        .map(|(key, value)| hashed_param(key, value))
        .collect::<Vec<_>>()
        .join(",");
    let first_hash = sha256_hex(joined.as_bytes());
    let cookie_hash = hashed_param(&birthday, &expires);
    sha512_hex(format!("{first_hash}{cookie_hash}").as_bytes())
}

fn birthday_cookie() -> (String, String) {
    let fallback = || {
        (
            "2000-01".to_string(),
            ((SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(1_704_067_200)
                + 315_360_000)
                .to_string()),
        )
    };
    let Ok(cookies) = cookies_get(BASE_URL) else {
        return fallback();
    };
    let Some(header) = cookies.header else {
        return fallback();
    };
    let Some(raw) = header
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("birthday="))
    else {
        return fallback();
    };
    let decoded = percent_decode(raw);
    let Ok(cookie) = serde_json::from_str::<BirthdayCookie>(&decoded) else {
        return fallback();
    };
    (cookie.value, cookie.expires.to_string())
}

fn hashed_param(key: &str, value: &str) -> String {
    format!(
        "{}_{}",
        sha256_hex(key.as_bytes()),
        sha512_hex(value.as_bytes())
    )
}

fn sha256_hex(input: &[u8]) -> String {
    hex_lower(&Sha256::digest(input))
}

fn sha512_hex(input: &[u8]) -> String {
    hex_lower(&Sha512::digest(input))
}

fn hex_lower(input: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(input.len() * 2);
    for byte in input {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(&input[index + 1..index + 3], 16) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

fn unscramble_base64_jpeg(input: &str, seed: u64) -> Result<String, image::ImageError> {
    let bytes = STANDARD.decode(input).map_err(|_| {
        image::ImageError::Limits(image::error::LimitError::from_kind(
            image::error::LimitErrorKind::InsufficientMemory,
        ))
    })?;
    let image = image::load_from_memory(&bytes)?.to_rgb8();
    let (width, height) = image.dimensions();
    let block_width = (width / 8 * 8) / 4;
    let block_height = (height / 8 * 8) / 4;
    let mut out = RgbImage::new(width, height);
    for pair in unscramble_coords(seed) {
        let src_x = pair.source.0 * block_width;
        let src_y = pair.source.1 * block_height;
        let dst_x = pair.dest.0 * block_width;
        let dst_y = pair.dest.1 * block_height;
        for y in 0..block_height {
            for x in 0..block_width {
                let pixel = image.get_pixel(src_x + x, src_y + y);
                out.put_pixel(dst_x + x, dst_y + y, *pixel);
            }
        }
    }
    let mut encoded = Vec::new();
    DynamicImage::ImageRgb8(out).write_to(&mut Cursor::new(&mut encoded), ImageFormat::Jpeg)?;
    Ok(STANDARD.encode(encoded))
}

#[derive(Clone, Copy)]
struct CoordPair {
    source: (u32, u32),
    dest: (u32, u32),
}

fn unscramble_coords(seed: u64) -> Vec<CoordPair> {
    let mut seed = seed as u32;
    let mut pairs = Vec::new();
    for index in 0..16 {
        seed = xorshift32(seed);
        pairs.push((seed, index));
    }
    pairs.sort_by_key(|pair| pair.0);
    pairs
        .into_iter()
        .enumerate()
        .map(|(dest, (_, source))| CoordPair {
            source: ((source % 4) as u32, (source / 4) as u32),
            dest: ((dest % 4) as u32, (dest / 4) as u32),
        })
        .collect()
}

fn xorshift32(mut value: u32) -> u32 {
    value ^= value << 13;
    value ^= value >> 17;
    value ^= value << 5;
    value
}

fn parse_iso_date(value: Option<&str>) -> Option<i64> {
    let value = value?;
    if value.len() < 10 {
        return None;
    }
    let year = value.get(0..4)?.parse::<i64>().ok()?;
    let month = value.get(5..7)?.parse::<i64>().ok()?;
    let day = value.get(8..10)?.parse::<i64>().ok()?;
    let hour = value.get(11..13).and_then(|v| v.parse().ok()).unwrap_or(0);
    let minute = value.get(14..16).and_then(|v| v.parse().ok()).unwrap_or(0);
    let second = value.get(17..19).and_then(|v| v.parse().ok()).unwrap_or(0);
    Some(timestamp_utc(year, month, day, hour, minute, second))
}

fn timestamp_utc(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> i64 {
    let y = year - (month <= 2) as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86_400 + hour * 3_600 + minute * 60 + second
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + (month <= 2) as i64;
    (year, month, day)
}

#[derive(Default, Deserialize)]
struct RankingResponse {
    #[serde(default)]
    ranking_title_list: Vec<RankingTitle>,
}

#[derive(Deserialize)]
struct RankingTitle {
    id: u64,
}

#[derive(Default, Deserialize)]
struct TitleListResponse {
    #[serde(default)]
    title_list: Vec<TitleDetail>,
}

#[derive(Default, Deserialize)]
struct TitleDetail {
    #[serde(default)]
    title_id: u64,
    #[serde(default)]
    title_name: String,
    thumbnail_image_url: Option<String>,
    banner_image_url: Option<String>,
    thumbnail_rect_image_url: Option<String>,
}

impl TitleDetail {
    fn into_catalog(self) -> CatalogItem {
        let key = format!("/title/{}", self.title_id);
        CatalogItem {
            key: key.clone(),
            title: if self.title_name.is_empty() {
                "K Manga".to_string()
            } else {
                self.title_name
            },
            cover: self
                .thumbnail_image_url
                .or(self.banner_image_url)
                .or(self.thumbnail_rect_image_url),
            url: Some(format!("{BASE_URL}{key}")),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct DetailResponse {
    #[serde(default)]
    web_title: WebTitle,
}

#[derive(Default, Deserialize)]
struct WebTitle {
    #[serde(default)]
    title_name: String,
    author_text: Option<String>,
    introduction_text: Option<String>,
    next_updated_text: Option<String>,
    title_in_japanese: Option<String>,
    #[serde(default)]
    episode_id_list: Vec<u64>,
    thumbnail_image_url: Option<String>,
    thumbnail_rect_image_url: Option<String>,
    banner_image_url: Option<String>,
}

impl WebTitle {
    fn into_catalog(self, key: String) -> CatalogItem {
        let mut description = Vec::new();
        description.extend(self.introduction_text.filter(|value| !value.is_empty()));
        description.extend(self.next_updated_text.filter(|value| !value.is_empty()));
        if let Some(japanese) = self.title_in_japanese.filter(|value| !value.is_empty()) {
            description.push(format!("Japanese Title: {japanese}"));
        }
        CatalogItem {
            key: key.clone(),
            title: if self.title_name.is_empty() {
                "K Manga".to_string()
            } else {
                self.title_name
            },
            cover: self
                .thumbnail_image_url
                .or(self.banner_image_url)
                .or(self.thumbnail_rect_image_url),
            authors: self.author_text.into_iter().collect(),
            description: if description.is_empty() {
                None
            } else {
                Some(description.join("\n\n"))
            },
            url: Some(format!("{BASE_URL}{key}")),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct EpisodeListResponse {
    #[serde(default)]
    episode_list: Vec<Episode>,
}

#[derive(Default, Deserialize)]
struct Episode {
    #[serde(default)]
    episode_id: u64,
    #[serde(default)]
    episode_name: String,
    start_time: Option<String>,
    #[serde(default)]
    point: u64,
    #[serde(default)]
    title_id: u64,
    #[serde(default)]
    index: u64,
    #[serde(default)]
    badge: u64,
    rental_finish_time: Option<String>,
}

impl Episode {
    fn is_locked(&self) -> bool {
        self.point > 0 && self.badge != 3 && self.rental_finish_time.is_none()
    }

    fn into_chapter(self) -> MangaChapter {
        let is_locked = self.is_locked();
        let key = format!("/title/{}/episode/{}", self.title_id, self.episode_id);
        let title = if self.episode_name.is_empty() {
            "Chapter".to_string()
        } else {
            self.episode_name
        };
        MangaChapter {
            key: key.clone(),
            title: Some(format!("{}{}", if is_locked { "[Locked] " } else { "" }, title)),
            chapter_number: Some(self.index as f32),
            date_uploaded: parse_iso_date(self.start_time.as_deref()),
            is_locked,
            url: Some(format!("{BASE_URL}{key}")),
            ..MangaChapter::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct ViewerResponse {
    #[serde(default)]
    page_list: Vec<String>,
    #[serde(default)]
    scramble_seed: u64,
}

#[derive(Deserialize)]
struct BirthdayCookie {
    value: String,
    expires: u64,
}

export_manga_source!(SOURCE);

const RANKING_FIXTURE: &str = r#"{ "ranking_title_list": [{ "id": 1 }] }"#;
const TITLE_LIST_FIXTURE: &str = r#"
{ "title_list": [{ "title_id": 1, "title_name": "Sample K Manga", "thumbnail_image_url": "https://kmanga.kodansha.com/cover.jpg" }] }
"#;
const DETAIL_FIXTURE: &str = r#"
{ "web_title": { "title_name": "Sample K Manga", "author_text": "Creator", "introduction_text": "A sample.", "episode_id_list": [1], "thumbnail_image_url": "https://kmanga.kodansha.com/cover.jpg" } }
"#;
const EPISODE_LIST_FIXTURE: &str = r#"
{ "episode_list": [{ "episode_id": 1, "episode_name": "Episode 1", "start_time": "2024-01-01 00:00:00", "point": 0, "title_id": 1, "index": 1, "badge": 0 }] }
"#;
const VIEWER_FIXTURE: &str = r#"
{ "page_list": ["https://kmanga.kodansha.com/page1.jpg"], "scramble_seed": 1 }
"#;
