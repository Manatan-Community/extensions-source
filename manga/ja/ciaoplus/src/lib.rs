use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult,
    abi::{ExtensionResult, system_time},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{dates, manga, manga_image, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha512};
use std::collections::{BTreeMap, BTreeSet};

const SOURCE: CiaoPlus = CiaoPlus;
const BASE_URL: &str = "https://ciao.shogakukan.co.jp";
const API_URL: &str = "https://api.ciao.shogakukan.co.jp";
const PAGE_LIMIT: u64 = 25;

struct CiaoPlus;

impl MangaSource for CiaoPlus {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_title_list(TITLE_LIST_FIXTURE, false));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_latest(&fetch_latest(page)));
        }
        fetch_ranking("1", page)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            let body = fetch_hashed_get(
                "search/title",
                vec![
                    ("keyword".into(), query.into()),
                    ("limit".into(), "99999".into()),
                    ("platform".into(), "3".into()),
                ],
                TITLE_LIST_FIXTURE,
            );
            return Ok(parse_title_list(&body, false));
        }

        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let category = filter_string(&request, "category").unwrap_or_else(|| "ranking|1".into());
        let Some((kind, id)) = category.split_once('|') else {
            return fetch_ranking("1", page);
        };
        if kind == "genre" {
            let body = fetch_hashed_get(
                "search/title",
                vec![
                    ("platform".into(), "3".into()),
                    ("genre_id".into(), id.into()),
                    ("limit".into(), "99999".into()),
                ],
                TITLE_LIST_FIXTURE,
            );
            return Ok(parse_title_list(&body, false));
        }
        fetch_ranking(id, page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/title/00001".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/title/00001".into());
        let details = fetch_detail(title_id_from_key(&key));
        Ok(parse_chapters(&details))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comics/title/00001/episode/1".into());
        let body = fetch_hashed_get(
            "web/episode/viewer",
            vec![
                ("platform".into(), "3".into()),
                ("episode_id".into(), episode_id_from_key(&key).into()),
            ],
            VIEWER_FIXTURE,
        );
        Ok(parse_pages(&body))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = fetch_ranking("1", 1)?;
        let latest = parse_latest(&fetch_latest(1));
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Compact),
                has_more: false,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::CiaoImage::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            let item = key
                .contains("/title/")
                .then(|| details_by_key(&manga_key_from_any(&key)));
            return Ok(Some(UrlResolveResult {
                item,
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_ranking(ranking_id: &str, page: u64) -> ExtensionResult<Paged<CatalogItem>> {
    let offset = page.saturating_sub(1) * PAGE_LIMIT;
    let body = fetch_hashed_get(
        "ranking/all",
        vec![
            ("platform".into(), "3".into()),
            ("ranking_id".into(), ranking_id.into()),
            ("offset".into(), offset.to_string()),
            ("limit".into(), "51".into()),
            ("is_top".into(), "0".into()),
        ],
        RANKING_FIXTURE,
    );
    let ranking = serde_json::from_str::<RankingResponse>(&body).unwrap_or_default();
    let ids = ranking
        .ranking_title_list
        .into_iter()
        .map(|entry| format!("{:05}", entry.id))
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(Paged::default());
    }
    let has_next_page = ids.len() > PAGE_LIMIT as usize;
    let selected = if has_next_page {
        &ids[..PAGE_LIMIT as usize]
    } else {
        &ids
    };
    let body = fetch_title_list(selected);
    Ok(parse_title_list(&body, has_next_page))
}

fn fetch_latest(page: u64) -> String {
    let date = latest_date(page);
    fetch_hashed_get(
        "web/title/ids",
        vec![("updated_at".into(), date), ("platform".into(), "3".into())],
        LATEST_FIXTURE,
    )
}

fn fetch_title_list(ids: &[String]) -> String {
    fetch_hashed_get(
        "title/list",
        vec![
            ("platform".into(), "3".into()),
            ("title_id_list".into(), ids.join(",")),
        ],
        TITLE_LIST_FIXTURE,
    )
}

fn fetch_detail(title_id: String) -> String {
    fetch_title_list(&[title_id])
}

fn fetch_hashed_get(path: &str, params: Vec<(String, String)>, fixture: &str) -> String {
    let target = api_url(path, &params);
    client()
        .get(target)
        .header("x-bambi-hash", generate_hash(&params))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_episode_list(ids: &[i64]) -> String {
    let joined = ids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let params = vec![
        ("platform".to_string(), "3".to_string()),
        ("episode_id_list".to_string(), joined),
    ];
    let form = [("platform", "3"), ("episode_id_list", params[1].1.as_str())];
    client()
        .post(format!("{API_URL}/episode/list"))
        .header("Origin", BASE_URL)
        .header("x-bambi-is-crawler", "false")
        .header("x-bambi-hash", generate_hash(&params))
        .form(&form)
        .send_text()
        .unwrap_or_else(|_| EPISODES_FIXTURE.to_string())
}

fn api_url(path: &str, params: &[(String, String)]) -> String {
    let mut target = format!("{API_URL}/{path}");
    if !params.is_empty() {
        target.push('?');
        target.push_str(
            &params
                .iter()
                .map(|(key, value)| format!("{key}={}", url::query_escape(value)))
                .collect::<Vec<_>>()
                .join("&"),
        );
    }
    target
}

fn parse_title_list(body: &str, has_next_page: bool) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<TitleListResponse>(body).unwrap_or_default();
    Paged {
        entries: response.title_list.into_iter().map(title_to_item).collect(),
        has_next_page,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<LatestResponse>(body).unwrap_or_default();
    let mut seen = BTreeSet::new();
    let entries = response
        .update_episode_titles
        .into_values()
        .flatten()
        .filter(|entry| seen.insert(entry.title_id))
        .map(latest_to_item)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_detail(&fetch_detail(title_id_from_key(key)), key)
}

fn parse_detail(body: &str, key: &str) -> CatalogItem {
    let response = serde_json::from_str::<DetailResponse>(body).unwrap_or_default();
    let Some(title) = response.title_list.into_iter().next() else {
        return CatalogItem {
            key: key.into(),
            title: "Ciao Plus".into(),
            language: Some("ja".into()),
            content_rating: Some("safe".into()),
            initialized: true,
            ..CatalogItem::default()
        };
    };
    CatalogItem {
        key: key.into(),
        title: title.title_name,
        authors: (!title.author_text.is_empty())
            .then_some(title.author_text)
            .into_iter()
            .collect(),
        description: (!title.introduction_text.is_empty()).then_some(title.introduction_text),
        tags: title
            .genre_id_list
            .into_iter()
            .map(|id| format!("Genre {id}"))
            .collect(),
        status: ItemStatus::Unknown,
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        url: Some(absolute_url(key)),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(details_body: &str) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<DetailResponse>(details_body).unwrap_or_default();
    let Some(title) = response.title_list.into_iter().next() else {
        return Vec::new();
    };
    let body = post_episode_list(&title.episode_id_list);
    let episodes = serde_json::from_str::<EpisodeListResponse>(&body).unwrap_or_default();
    episodes
        .episode_list
        .into_iter()
        .rev()
        .map(|episode| {
            let chapter_title = if episode
                .episode_name
                .trim()
                .starts_with(title.title_name.trim())
            {
                format!("【第{}話】 {}", episode.index, episode.episode_name.trim())
            } else {
                episode.episode_name.trim().to_string()
            };
            let key = format!(
                "/comics/title/{:05}/episode/{}",
                episode.title_id, episode.episode_id
            );
            MangaChapter {
                key: key.clone(),
                title: Some(chapter_title),
                chapter_number: Some(episode.index as f32),
                date_uploaded: parse_datetime_jst(&episode.start_time),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<ViewerResponse>(body).unwrap_or_default();
    response
        .page_list
        .into_iter()
        .enumerate()
        .map(|(index, image)| {
            let mut extra = BTreeMap::new();
            extra.insert("scrambleSeed".into(), json!(response.scramble_seed));
            extra.insert("scrambleVersion".into(), json!(response.scramble_ver));
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
        .collect()
}

fn title_to_item(title: TitleDetail) -> CatalogItem {
    let key = format!("/comics/title/{:05}", title.title_id);
    CatalogItem {
        key: key.clone(),
        title: title.title_name,
        cover: title.thumbnail_image_url,
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        url: Some(absolute_url(&key)),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn latest_to_item(title: LatestTitleDetail) -> CatalogItem {
    let key = format!("/comics/title/{:05}", title.title_id);
    CatalogItem {
        key: key.clone(),
        title: title.title_name,
        cover: title.thumbnail_image,
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        url: Some(absolute_url(&key)),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn generate_hash(params: &[(String, String)]) -> String {
    let mut sorted = params.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let joined = sorted
        .iter()
        .map(|(key, value)| format!("{}_{}", sha256_hex(key), sha512_hex(value)))
        .collect::<Vec<_>>()
        .join(",");
    sha512_hex(&sha256_hex(&joined))
}

fn sha256_hex(input: &str) -> String {
    format!("{:x}", Sha256::digest(input.as_bytes()))
}

fn sha512_hex(input: &str) -> String {
    format!("{:x}", Sha512::digest(input.as_bytes()))
}

fn latest_date(page: u64) -> String {
    let seconds = system_time()
        .map(|time| time.unix_seconds)
        .unwrap_or(1_704_067_200)
        + 9 * 3600
        - page.saturating_sub(1) as i64 * 86_400;
    let days = seconds.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}{month:02}{day:02}")
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + (month <= 2) as i64;
    (year as i32, month as u32, day as u32)
}

fn parse_datetime_jst(value: &str) -> Option<i64> {
    let (date, time) = value.split_once(' ')?;
    let day_start = dates::parse_ymd(date)?;
    let mut parts = time.split(':');
    let hour = parts.next()?.parse::<i64>().ok()?;
    let minute = parts.next()?.parse::<i64>().ok()?;
    let second = parts.next().unwrap_or("0").parse::<i64>().ok()?;
    Some(day_start + hour * 3600 + minute * 60 + second - 9 * 3600)
}

fn title_id_from_key(key: &str) -> String {
    key.trim_matches('/')
        .split('/')
        .skip_while(|part| *part != "title")
        .nth(1)
        .unwrap_or("00001")
        .to_string()
}

fn episode_id_from_key(key: &str) -> &str {
    key.trim_matches('/')
        .split('/')
        .skip_while(|part| *part != "episode")
        .nth(1)
        .unwrap_or("1")
}

fn manga_key_from_any(key: &str) -> String {
    format!("/comics/title/{}", title_id_from_key(key))
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .map(|path| format!("/{}", path.trim_matches('/')))
        .filter(|key| key.contains("/comics/title/"))
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

#[derive(Default, Deserialize)]
struct RankingResponse {
    #[serde(default)]
    ranking_title_list: Vec<RankingTitle>,
}

#[derive(Deserialize)]
struct RankingTitle {
    id: i64,
}

#[derive(Default, Deserialize)]
struct TitleListResponse {
    #[serde(default)]
    title_list: Vec<TitleDetail>,
}

#[derive(Deserialize)]
struct TitleDetail {
    title_id: i64,
    title_name: String,
    #[serde(default)]
    thumbnail_image_url: Option<String>,
}

#[derive(Default, Deserialize)]
struct LatestResponse {
    #[serde(default)]
    update_episode_titles: BTreeMap<String, Vec<LatestTitleDetail>>,
}

#[derive(Deserialize)]
struct LatestTitleDetail {
    title_id: i64,
    title_name: String,
    #[serde(default)]
    thumbnail_image: Option<String>,
}

#[derive(Default, Deserialize)]
struct DetailResponse {
    #[serde(default, alias = "webTitle")]
    title_list: Vec<WebTitle>,
}

#[derive(Deserialize)]
struct WebTitle {
    title_name: String,
    #[serde(default)]
    author_text: String,
    #[serde(default)]
    introduction_text: String,
    #[serde(default)]
    genre_id_list: Vec<i64>,
    #[serde(default)]
    episode_id_list: Vec<i64>,
}

#[derive(Default, Deserialize)]
struct EpisodeListResponse {
    #[serde(default)]
    episode_list: Vec<Episode>,
}

#[derive(Deserialize)]
struct Episode {
    episode_id: i64,
    episode_name: String,
    index: i64,
    start_time: String,
    title_id: i64,
}

#[derive(Default, Deserialize)]
struct ViewerResponse {
    #[serde(default)]
    page_list: Vec<String>,
    #[serde(default)]
    scramble_seed: i64,
    #[serde(default)]
    scramble_ver: u64,
}

const RANKING_FIXTURE: &str = r#"{"ranking_title_list":[{"id":1}]}"#;
const TITLE_LIST_FIXTURE: &str = r#"{"title_list":[{"title_id":1,"title_name":"Sample Ciao","thumbnail_image_url":"https://img.example/cover.jpg","author_text":"Author","introduction_text":"Summary","genre_id_list":[1],"episode_id_list":[10]}]}"#;
const LATEST_FIXTURE: &str = r#"{"update_episode_titles":{"2024-01-01":[{"title_id":1,"title_name":"Sample Ciao","thumbnail_image":"https://img.example/cover.jpg"}]}}"#;
const EPISODES_FIXTURE: &str = r#"{"episode_list":[{"episode_id":10,"episode_name":"Episode 1","index":1,"start_time":"2024-01-01 00:00:00","title_id":1}]}"#;
const VIEWER_FIXTURE: &str =
    r#"{"page_list":["https://img.example/001.jpg"],"scramble_seed":12345,"scramble_ver":2}"#;

export_manga_source!(SOURCE);
