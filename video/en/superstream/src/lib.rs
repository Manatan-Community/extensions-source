use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::{
    Encryptor,
    cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7},
};
use des::TdesEde3;
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, abi::system_time, export_video_source,
    source::VideoSource,
};
use manatan_shared::sdk::{Context, http::HttpClient};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const SOURCE: SuperStream = SuperStream;
const API_URL: &str = "https://showbox.shegu.net/api/api_client/index/";
const APP_KEY: &str = "moviebox";
const APP_ID: &str = "com.tdo.showbox";
const KEY: &str = "123d6cedf626dy54233aa1w6";
const IV: &str = "wVephTn!";
const TYPE_MOVIES: u64 = 1;
const TYPE_SERIES: u64 = 2;
const PAGE_SIZE: usize = 20;

struct SuperStream;

impl VideoSource for SuperStream {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = home_query(page(&request), hide_nsfw(&request));
        let response = query_api(&body).unwrap_or_else(|_| LIST_FIXTURE.to_string());
        Ok(parse_home(&response, listing(&request)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.is_empty() {
            return self.list(with_listing(&request, "popular"));
        }
        let body = search_query(page(&request), query, hide_nsfw(&request));
        let response = query_api(&body).unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
        let entries = serde_json::from_str::<MainData>(&response)
            .unwrap_or_default()
            .data
            .into_iter()
            .filter_map(Data::into_item)
            .collect::<Vec<_>>();
        Ok(Paged {
            has_next_page: entries.len() >= PAGE_SIZE,
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(sample_key);
        Ok(fetch_details(&key, &request))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_else(sample_key);
        let load = parse_load_data(&key).unwrap_or_else(|| LoadData {
            id: 1,
            media_type: Some(TYPE_MOVIES),
        });
        if load.media_type == Some(TYPE_MOVIES) {
            let body = movie_detail_query(load.id, hide_nsfw(&request));
            let response = query_api(&body).unwrap_or_else(|_| MOVIE_DETAILS_FIXTURE.to_string());
            let movie = serde_json::from_str::<MovieDataProp>(&response)
                .unwrap_or_default()
                .data
                .unwrap_or_default();
            let key = link_key(
                load.id,
                movie.box_type.or(load.media_type).unwrap_or(TYPE_MOVIES),
                None,
                Some(1),
            );
            return Ok(vec![VideoEpisode {
                key,
                title: Some("Movie".to_string()),
                episode_number: Some(1.0),
                date_uploaded: movie.update_time.map(|time| time * 1_000),
                thumbnail: movie.poster.or(movie.poster_org),
                language: Some("en".to_string()),
                ..VideoEpisode::default()
            }]);
        }

        let detail = fetch_series_detail(load.id, &request).unwrap_or_else(|| {
            serde_json::from_str::<SeriesData>(SERIES_DATA_FIXTURE).unwrap_or_default()
        });
        let mut episodes = Vec::new();
        for season in &detail.season {
            let query = series_episodes_query(load.id, *season, hide_nsfw(&request));
            let response =
                query_api(&query).unwrap_or_else(|_| SERIES_EPISODES_FIXTURE.to_string());
            let season_eps = serde_json::from_str::<SeriesSeasonProp>(&response)
                .unwrap_or_default()
                .data
                .unwrap_or_default();
            episodes.extend(season_eps.into_iter().filter_map(series_episode_to_video));
        }
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode").unwrap_or_else(sample_link_key);
        let link = parse_link_data(&key).unwrap_or_else(|| LinkData {
            id: 1,
            media_type: TYPE_MOVIES,
            season: None,
            episode: Some(1),
        });
        let body = stream_query(&link);
        let response = query_api(&body).unwrap_or_else(|_| STREAMS_FIXTURE.to_string());
        let payload = serde_json::from_str::<LinkDataProp>(&response).unwrap_or_default();
        if payload.code == Some(-102) {
            return Ok(Vec::new());
        }
        let data = payload.data.unwrap_or_default();
        let fid = data.list.iter().find_map(|item| item.fid);
        let subtitles = fetch_subtitles(&link, fid);
        let mut streams = data
            .list
            .into_iter()
            .filter_map(|item| item.into_stream(&subtitles))
            .collect::<Vec<_>>();
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(None)
    }

    fn episode_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(None)
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(API_URL)
        .with_cookies_for("https://showbox.shegu.net")
        .with_webview_challenge_fallback()
}

fn query_api(query: &str) -> ExtensionResult<String> {
    let encrypted = encrypt_query(query).unwrap_or_default();
    let app_key_hash = md5_hex(APP_KEY);
    let verify = md5_hex(&format!("{}{}{}", md5_hex(APP_KEY), KEY, encrypted));
    let envelope = json!({
        "app_key": app_key_hash,
        "verify": verify,
        "encrypt_data": encrypted
    });
    let encoded = STANDARD.encode(envelope.to_string().as_bytes());
    client()
        .post(API_URL)
        .header("Platform", "android")
        .header("Accept", "charset=utf-8")
        .form(&[
            ("data", encoded.as_str()),
            ("appid", "27"),
            ("platform", "android"),
            ("version", "129"),
            ("medium", "Website&token0123456789abcdef0123456789abcdef"),
        ])
        .send_text()
}

fn encrypt_query(query: &str) -> Option<String> {
    let mut key = [0_u8; 24];
    let source = KEY.as_bytes();
    key[..source.len().min(24)].copy_from_slice(&source[..source.len().min(24)]);
    let encrypted = Encryptor::<TdesEde3>::new_from_slices(&key, IV.as_bytes())
        .ok()?
        .encrypt_padded_vec_mut::<Pkcs7>(query.as_bytes());
    Some(STANDARD.encode(encrypted))
}

fn md5_hex(input: &str) -> String {
    format!("{:x}", Md5::digest(input.as_bytes()))
}

fn expiry_date() -> i64 {
    system_time()
        .map(|time| time.unix_seconds.max(0) + 60 * 60 * 12)
        .unwrap_or(60 * 60 * 12)
}

fn home_query(page: u64, hide_nsfw: bool) -> String {
    json!({
        "childmode": if hide_nsfw { "1" } else { "0" },
        "app_version": "11.5",
        "appid": APP_ID,
        "module": "Home_list_type_v2",
        "channel": "Website",
        "page": page.to_string(),
        "lang": "en",
        "type": "all",
        "pagelimit": "20",
        "expired_date": expiry_date().to_string(),
        "platform": "android"
    })
    .to_string()
}

fn search_query(page: u64, query: &str, hide_nsfw: bool) -> String {
    json!({
        "childmode": if hide_nsfw { "1" } else { "0" },
        "app_version": "11.5",
        "appid": APP_ID,
        "module": "Search3",
        "channel": "Website",
        "page": page.to_string(),
        "lang": "en",
        "type": "all",
        "keyword": query,
        "pagelimit": "20",
        "expired_date": expiry_date().to_string(),
        "platform": "android"
    })
    .to_string()
}

fn movie_detail_query(id: u64, hide_nsfw: bool) -> String {
    json!({
        "childmode": if hide_nsfw { "1" } else { "0" },
        "uid": "",
        "app_version": "11.5",
        "appid": APP_ID,
        "module": "Movie_detail",
        "channel": "Website",
        "mid": id.to_string(),
        "lang": "en",
        "expired_date": expiry_date().to_string(),
        "platform": "android",
        "oss": "",
        "group": ""
    })
    .to_string()
}

fn series_detail_query(id: u64, hide_nsfw: bool) -> String {
    json!({
        "childmode": if hide_nsfw { "1" } else { "0" },
        "uid": "",
        "app_version": "11.5",
        "appid": APP_ID,
        "module": "TV_detail_1",
        "display_all": "1",
        "channel": "Website",
        "lang": "en",
        "expired_date": expiry_date().to_string(),
        "platform": "android",
        "tid": id.to_string()
    })
    .to_string()
}

fn series_episodes_query(id: u64, season: u64, hide_nsfw: bool) -> String {
    json!({
        "childmode": if hide_nsfw { "1" } else { "0" },
        "app_version": "11.5",
        "year": "0",
        "appid": APP_ID,
        "module": "TV_episode",
        "display_all": "1",
        "channel": "Website",
        "season": season.to_string(),
        "lang": "en",
        "expired_date": expiry_date().to_string(),
        "platform": "android",
        "tid": id.to_string()
    })
    .to_string()
}

fn stream_query(link: &LinkData) -> String {
    if link.media_type == TYPE_MOVIES {
        json!({
            "childmode": "0",
            "uid": "",
            "app_version": "11.5",
            "appid": APP_ID,
            "module": "Movie_downloadurl_v3",
            "channel": "Website",
            "mid": link.id.to_string(),
            "lang": "",
            "expired_date": expiry_date().to_string(),
            "platform": "android",
            "oss": "1",
            "group": ""
        })
    } else {
        json!({
            "childmode": "0",
            "app_version": "11.5",
            "module": "TV_downloadurl_v3",
            "channel": "Website",
            "episode": link.episode.unwrap_or(1).to_string(),
            "expired_date": expiry_date().to_string(),
            "platform": "android",
            "tid": link.id.to_string(),
            "oss": "1",
            "uid": "",
            "appid": APP_ID,
            "season": link.season.unwrap_or(1).to_string(),
            "lang": "en",
            "group": ""
        })
    }
    .to_string()
}

fn subtitle_query(link: &LinkData, fid: Option<u64>) -> String {
    if link.media_type == TYPE_MOVIES {
        json!({
            "childmode": "0",
            "fid": fid.unwrap_or(0).to_string(),
            "uid": "",
            "app_version": "11.5",
            "appid": APP_ID,
            "module": "Movie_srt_list_v2",
            "channel": "Website",
            "mid": link.id.to_string(),
            "lang": "en",
            "expired_date": expiry_date().to_string(),
            "platform": "android"
        })
    } else {
        json!({
            "childmode": "0",
            "fid": fid.unwrap_or(0).to_string(),
            "app_version": "11.5",
            "module": "TV_srt_list_v2",
            "channel": "Website",
            "episode": link.episode.unwrap_or(1).to_string(),
            "expired_date": expiry_date().to_string(),
            "platform": "android",
            "tid": link.id.to_string(),
            "uid": "",
            "appid": APP_ID,
            "season": link.season.unwrap_or(1).to_string(),
            "lang": "en"
        })
    }
    .to_string()
}

fn parse_home(body: &str, listing: &str) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<HomeData>(body).unwrap_or_default();
    let mut entries = Vec::new();
    for (index, section) in payload.data.into_iter().enumerate() {
        if listing != "latest" && index == 0 {
            continue;
        }
        if listing == "latest"
            && !matches!(section.kind.as_deref(), Some("newupload" | "newupdate"))
        {
            continue;
        }
        entries.extend(section.list.into_iter().filter_map(PostJson::into_item));
    }
    Paged {
        has_next_page: !entries.is_empty(),
        entries,
    }
}

fn fetch_details(key: &str, request: &Value) -> CatalogItem {
    let load = parse_load_data(key).unwrap_or_else(|| LoadData {
        id: 1,
        media_type: Some(TYPE_MOVIES),
    });
    if load.media_type == Some(TYPE_MOVIES) {
        let query = movie_detail_query(load.id, hide_nsfw(request));
        let body = query_api(&query).unwrap_or_else(|_| MOVIE_DETAILS_FIXTURE.to_string());
        let movie = serde_json::from_str::<MovieDataProp>(&body)
            .unwrap_or_default()
            .data
            .unwrap_or_default();
        return movie.into_item(key.to_string());
    }
    let detail = fetch_series_detail(load.id, request).unwrap_or_default();
    detail.into_item(key.to_string())
}

fn fetch_series_detail(id: u64, request: &Value) -> Option<SeriesData> {
    let query = series_detail_query(id, hide_nsfw(request));
    let body = query_api(&query).unwrap_or_else(|_| SERIES_DETAILS_FIXTURE.to_string());
    serde_json::from_str::<SeriesDataProp>(&body).ok()?.data
}

fn series_episode_to_video(ep: SeriesEpisode) -> Option<VideoEpisode> {
    if ep.source_file != Some(1) {
        return None;
    }
    let id = ep.tid.or(ep.id)?;
    let season = ep.season.unwrap_or(1);
    let episode = ep.episode.unwrap_or(1);
    let title = ep.title.unwrap_or_default();
    let label = if title.is_empty() {
        format!("Season {season} Ep {episode}")
    } else {
        format!("Season {season} Ep {episode}: {title}")
    };
    Some(VideoEpisode {
        key: link_key(id, TYPE_SERIES, Some(season), Some(episode)),
        title: Some(label),
        episode_number: Some(episode as f32),
        season_number: Some(season as f32),
        date_uploaded: ep.update_time.map(|time| time * 1_000),
        thumbnail: ep.thumbs.or(ep.thumbs_original),
        language: Some("en".to_string()),
        ..VideoEpisode::default()
    })
}

fn fetch_subtitles(link: &LinkData, fid: Option<u64>) -> Vec<SubtitleTrack> {
    let query = subtitle_query(link, fid);
    let body = query_api(&query).unwrap_or_else(|_| SUBTITLES_FIXTURE.to_string());
    let payload = serde_json::from_str::<SubtitleDataProp>(&body).unwrap_or_default();
    payload
        .data
        .and_then(|data| data.list)
        .unwrap_or_default()
        .into_iter()
        .flat_map(|group| {
            group
                .subtitles
                .into_iter()
                .enumerate()
                .filter_map(|(index, sub)| {
                    let url = sub.file_path?;
                    let language = sub
                        .language
                        .or(sub.lang)
                        .unwrap_or_else(|| "Sub".to_string());
                    let point = sub
                        .point
                        .and_then(|value| value.as_str().map(ToString::to_string))
                        .unwrap_or_else(|| "0".to_string());
                    Some(SubtitleTrack {
                        url,
                        language: Some(language_code(&language)),
                        label: Some(format!("{language} {} ({point})", index + 1)),
                        format: Some("srt".to_string()),
                        headers: playback_headers(),
                        ..SubtitleTrack::default()
                    })
                })
        })
        .collect()
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or_default().to_lowercase();
        if quality.contains(&preferred.to_lowercase()) {
            0
        } else {
            1
        }
    });
}

fn playback_headers() -> Context {
    let mut headers = Context::new();
    headers.insert("Platform".to_string(), "android".to_string());
    headers.insert("Accept".to_string(), "charset=utf-8".to_string());
    headers
}

fn request_key(request: &Value, key: &str) -> Option<String> {
    request
        .get(key)
        .and_then(|value| {
            value
                .get("key")
                .or_else(|| value.get("url"))
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
        })
        .map(ToString::to_string)
}

fn parse_load_data(input: &str) -> Option<LoadData> {
    serde_json::from_str(input).ok()
}

fn parse_link_data(input: &str) -> Option<LinkData> {
    serde_json::from_str(input).ok()
}

fn link_key(id: u64, media_type: u64, season: Option<u64>, episode: Option<u64>) -> String {
    serde_json::to_string(&LinkData {
        id,
        media_type,
        season,
        episode,
    })
    .unwrap_or_else(|_| sample_link_key())
}

fn sample_key() -> String {
    serde_json::to_string(&LoadData {
        id: 1,
        media_type: Some(TYPE_MOVIES),
    })
    .unwrap_or_else(|_| r#"{"id":1,"type":1}"#.to_string())
}

fn sample_link_key() -> String {
    r#"{"id":1,"type":1,"season":null,"episode":1}"#.to_string()
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .or_else(|| request.get("pageNumber"))
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn hide_nsfw(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("pref_hide_nsfw"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn pref(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    } else {
        next = json!({ "listing": listing });
    }
    next
}

fn clean_tags(input: Option<String>) -> Vec<String> {
    input
        .unwrap_or_default()
        .split(',')
        .map(|tag| {
            let mut chars = tag.trim().chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .filter(|tag| !tag.is_empty())
        .collect()
}

fn names(input: Option<String>) -> Vec<String> {
    input
        .unwrap_or_default()
        .split('\n')
        .next()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn push_description_line(lines: &mut Vec<String>, label: &str, value: Option<String>) {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return;
    };
    lines.push(format!("{label}: {value}"));
}

fn language_code(input: &str) -> String {
    match input.to_lowercase().as_str() {
        value if value.contains("english") || value == "en" => "en".to_string(),
        value if value.contains("spanish") => "es".to_string(),
        value if value.contains("french") => "fr".to_string(),
        value if value.contains("german") => "de".to_string(),
        value if value.contains("portuguese") => "pt".to_string(),
        value if value.contains("arabic") => "ar".to_string(),
        value if value.contains("chinese") => "zh".to_string(),
        value if value.contains("japanese") => "ja".to_string(),
        _ => "und".to_string(),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LoadData {
    id: u64,
    #[serde(rename = "type")]
    media_type: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LinkData {
    id: u64,
    #[serde(rename = "type")]
    media_type: u64,
    season: Option<u64>,
    episode: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct HomeData {
    #[serde(default)]
    data: Vec<ListJson>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ListJson {
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    list: Vec<PostJson>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PostJson {
    id: Option<u64>,
    title: Option<String>,
    poster: Option<String>,
    poster_2: Option<String>,
    box_type: Option<u64>,
    imdb_rating: Option<String>,
    quality_tag: Option<String>,
}

impl PostJson {
    fn into_item(self) -> Option<CatalogItem> {
        let id = self.id?;
        let media_type = self.box_type;
        let key = serde_json::to_string(&LoadData { id, media_type }).ok()?;
        Some(CatalogItem {
            key,
            title: self.title?,
            cover: self.poster.or(self.poster_2),
            rating: self.imdb_rating.and_then(|value| value.parse().ok()),
            tags: self.quality_tag.into_iter().collect(),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            ..CatalogItem::default()
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct MainData {
    #[serde(default)]
    data: Vec<Data>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Data {
    id: Option<u64>,
    mid: Option<u64>,
    tid: Option<u64>,
    box_type: Option<u64>,
    title: Option<String>,
    poster_org: Option<String>,
    poster: Option<String>,
    cats: Option<String>,
    year: Option<u64>,
    imdb_rating: Option<String>,
    quality_tag: Option<String>,
}

impl Data {
    fn into_item(self) -> Option<CatalogItem> {
        let media_type = self
            .box_type
            .or_else(|| self.mid.map(|_| TYPE_MOVIES))
            .or_else(|| self.tid.map(|_| TYPE_SERIES))?;
        let id = self.id.or(self.mid).or(self.tid)?;
        let key = serde_json::to_string(&LoadData {
            id,
            media_type: Some(media_type),
        })
        .ok()?;
        let mut tags = clean_tags(self.cats);
        tags.extend(self.quality_tag);
        if let Some(year) = self.year {
            tags.push(year.to_string());
        }
        Some(CatalogItem {
            key,
            title: self.title?,
            cover: self.poster_org.or(self.poster),
            rating: self.imdb_rating.and_then(|value| value.parse().ok()),
            tags,
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            ..CatalogItem::default()
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct MovieDataProp {
    data: Option<MovieData>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct MovieData {
    title: Option<String>,
    director: Option<String>,
    writer: Option<String>,
    poster: Option<String>,
    description: Option<String>,
    cats: Option<String>,
    update_time: Option<i64>,
    imdb_rating: Option<String>,
    released: Option<String>,
    poster_org: Option<String>,
    box_type: Option<u64>,
}

impl MovieData {
    fn into_item(self, key: String) -> CatalogItem {
        let mut lines = Vec::new();
        if let Some(description) = self.description.filter(|value| !value.trim().is_empty()) {
            lines.push(description);
        }
        push_description_line(&mut lines, "Released", self.released);
        push_description_line(&mut lines, "Writers", self.writer.clone());
        push_description_line(&mut lines, "Directors", self.director.clone());
        CatalogItem {
            key,
            title: self.title.unwrap_or_else(|| "Movie".to_string()),
            cover: self.poster.or(self.poster_org),
            authors: names(self.writer),
            artists: names(self.director),
            description: (!lines.is_empty()).then(|| lines.join("\n\n")),
            tags: clean_tags(self.cats),
            rating: self.imdb_rating.and_then(|value| value.parse().ok()),
            latest_update: self.update_time.map(|time| time * 1_000),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            status: ItemStatus::Completed,
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SeriesDataProp {
    data: Option<SeriesData>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SeriesData {
    title: Option<String>,
    director: Option<String>,
    writer: Option<String>,
    poster: Option<String>,
    banner_mini: Option<String>,
    description: Option<String>,
    cats: Option<String>,
    released: Option<String>,
    imdb_rating: Option<String>,
    poster_org: Option<String>,
    banner_mini_org: Option<String>,
    #[serde(default)]
    season: Vec<u64>,
}

impl SeriesData {
    fn into_item(self, key: String) -> CatalogItem {
        let mut lines = Vec::new();
        if let Some(description) = self.description.filter(|value| !value.trim().is_empty()) {
            lines.push(description);
        }
        push_description_line(&mut lines, "Released", self.released);
        push_description_line(&mut lines, "Writers", self.writer.clone());
        push_description_line(&mut lines, "Directors", self.director.clone());
        CatalogItem {
            key,
            title: self.title.unwrap_or_else(|| "Series".to_string()),
            cover: self.poster_org.or(self.poster),
            banner: self.banner_mini_org.or(self.banner_mini),
            authors: names(self.writer),
            artists: names(self.director),
            description: (!lines.is_empty()).then(|| lines.join("\n\n")),
            tags: clean_tags(self.cats),
            rating: self.imdb_rating.and_then(|value| value.parse().ok()),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            status: ItemStatus::Completed,
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SeriesSeasonProp {
    data: Option<Vec<SeriesEpisode>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SeriesEpisode {
    id: Option<u64>,
    tid: Option<u64>,
    season: Option<u64>,
    episode: Option<u64>,
    title: Option<String>,
    thumbs: Option<String>,
    thumbs_original: Option<String>,
    source_file: Option<u64>,
    update_time: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct LinkDataProp {
    code: Option<i64>,
    data: Option<ParsedLinkData>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ParsedLinkData {
    #[serde(default)]
    list: Vec<LinkList>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct LinkList {
    path: Option<String>,
    quality: Option<String>,
    real_quality: Option<String>,
    format: Option<String>,
    size: Option<String>,
    size_bytes: Option<u64>,
    fid: Option<u64>,
    h265: Option<u64>,
    width: Option<u64>,
    height: Option<u64>,
}

impl LinkList {
    fn into_stream(self, subtitles: &[SubtitleTrack]) -> Option<VideoStream> {
        let url = self.path?.replace("\\/", "/");
        if url.trim().is_empty() {
            return None;
        }
        let quality = self
            .quality
            .or(self.real_quality)
            .unwrap_or_else(|| "quality".to_string());
        let mut name = quality.clone();
        if let Some(size) = self.size.filter(|value| !value.trim().is_empty()) {
            name.push(' ');
            name.push_str(&size);
        }
        let is_hls = url.contains(".m3u8");
        let format = self
            .format
            .or_else(|| is_hls.then(|| "hls".to_string()))
            .or_else(|| Some("mp4".to_string()));
        Some(VideoStream {
            url,
            name: Some(name),
            quality: Some(quality),
            format,
            resolution: match (self.width, self.height) {
                (Some(width), Some(height)) => Some(format!("{width}x{height}")),
                _ => None,
            },
            video_codec: self
                .h265
                .filter(|value| *value > 0)
                .map(|_| "h265".to_string()),
            size_bytes: self.size_bytes,
            is_hls,
            stream_kind: Some(if is_hls {
                VideoStreamKind::Hls
            } else {
                VideoStreamKind::Direct
            }),
            headers: playback_headers(),
            subtitles: subtitles.to_vec(),
            initialized: true,
            ..VideoStream::default()
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SubtitleDataProp {
    data: Option<SubtitleData>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SubtitleData {
    list: Option<Vec<SubtitleList>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SubtitleList {
    #[serde(default)]
    subtitles: Vec<Subtitles>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Subtitles {
    language: Option<String>,
    lang: Option<String>,
    file_path: Option<String>,
    point: Option<Value>,
}

const LIST_FIXTURE: &str = r#"{"data":[{"type":"featured","list":[{"id":1,"title":"Featured Fixture","poster":"https://fixtures.invalid/superstream/featured.jpg","box_type":1}]},{"type":"popular","list":[{"id":1,"title":"Sample Movie","poster":"https://fixtures.invalid/superstream/movie.jpg","box_type":1,"imdb_rating":"7.2","quality_tag":"HD"},{"id":2,"title":"Sample Series","poster":"https://fixtures.invalid/superstream/series.jpg","box_type":2,"imdb_rating":"8.0","quality_tag":"HD"}]},{"type":"newupload","list":[{"id":3,"title":"Latest Fixture","poster":"https://fixtures.invalid/superstream/latest.jpg","box_type":1}]}]}"#;
const SEARCH_FIXTURE: &str = r#"{"data":[{"id":1,"box_type":1,"title":"Sample Movie","poster":"https://fixtures.invalid/superstream/movie.jpg","cats":"action,drama","year":2024,"imdb_rating":"7.2","quality_tag":"HD"},{"tid":2,"box_type":2,"title":"Sample Series","poster":"https://fixtures.invalid/superstream/series.jpg","cats":"adventure","year":2024,"imdb_rating":"8.0"}]}"#;
const MOVIE_DETAILS_FIXTURE: &str = r#"{"data":{"id":1,"title":"Sample Movie","director":"Fixture Director","writer":"Fixture Writer","poster":"https://fixtures.invalid/superstream/movie.jpg","description":"Movie fixture used for smoke tests.","cats":"action,drama","update_time":1710000000,"imdb_rating":"7.2","released":"2024","box_type":1}}"#;
const SERIES_DETAILS_FIXTURE: &str = r#"{"data":{"title":"Sample Series","director":"Fixture Director","writer":"Fixture Writer","poster":"https://fixtures.invalid/superstream/series.jpg","description":"Series fixture used for smoke tests.","cats":"adventure","released":"2024","imdb_rating":"8.0","season":[1]}}"#;
const SERIES_DATA_FIXTURE: &str = r#"{"title":"Sample Series","director":"Fixture Director","writer":"Fixture Writer","poster":"https://fixtures.invalid/superstream/series.jpg","description":"Series fixture used for smoke tests.","cats":"adventure","released":"2024","imdb_rating":"8.0","season":[1]}"#;
const SERIES_EPISODES_FIXTURE: &str = r#"{"data":[{"id":20,"tid":2,"season":1,"episode":1,"title":"Pilot","thumbs":"https://fixtures.invalid/superstream/ep1.jpg","source_file":1,"update_time":1710000000}]}"#;
const STREAMS_FIXTURE: &str = r#"{"code":0,"data":{"list":[{"path":"https://fixtures.invalid/superstream/video-1080.mp4","quality":"1080p","format":"mp4","size":"100 MB","size_bytes":104857600,"fid":1,"width":1920,"height":1080},{"path":"https://fixtures.invalid/superstream/video-720.mp4","quality":"720p","format":"mp4","size":"50 MB","size_bytes":52428800,"fid":2,"width":1280,"height":720}]}}"#;
const SUBTITLES_FIXTURE: &str = r#"{"data":{"list":[{"language":"English","subtitles":[{"language":"English","file_path":"https://fixtures.invalid/superstream/en.srt","point":"0"}]}]}}"#;

export_video_source!(SOURCE);
