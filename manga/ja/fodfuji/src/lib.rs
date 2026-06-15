use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, manga_image, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: FodFuji = FodFuji;
const BASE_URL: &str = "https://manga.fod.fujitv.co.jp";
const API_URL: &str = "https://manga.fod.fujitv.co.jp/web/books";

struct FodFuji;

impl MangaSource for FodFuji {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_ranking(RANKING_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            Ok(fetch_latest(page))
        } else {
            Ok(fetch_popular(page))
        }
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(fetch_search(query, page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample/1".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample/1".into());
        let hide_locked = preference_bool(&request, "hide_locked");
        Ok(fetch_chapters(&key, hide_locked))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".into());
        Ok(fetch_pages(&key))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = fetch_popular(1);
        let latest = fetch_latest(1);
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
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::GuardianBlockImage::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/books/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}/viewer/{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
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
        .with_header("Zk-Web-Version", "1.3.5")
        .with_header("Cookie", "sfsc=0")
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_popular(page: u64) -> Paged<CatalogItem> {
    parse_ranking(&api_get(
        "genreRanking",
        vec![
            ("category".into(), "0".into()),
            ("sort_type".into(), "2".into()),
            ("page".into(), page.to_string()),
        ],
        RANKING_FIXTURE,
    ))
}

fn fetch_latest(page: u64) -> Paged<CatalogItem> {
    let body = api_get(
        "newArrival",
        vec![
            ("category".into(), "0".into()),
            ("sort_type".into(), "0".into()),
            ("page".into(), page.to_string()),
        ],
        LATEST_FIXTURE,
    );
    let response = serde_json::from_str::<LatestResponse>(&body).unwrap_or_default();
    Paged {
        entries: response
            .new_arrival_books
            .into_iter()
            .map(title_to_item)
            .collect(),
        has_next_page: response.current_page < response.total,
    }
}

fn fetch_search(query: &str, page: u64) -> Paged<CatalogItem> {
    let body = api_get(
        "search",
        vec![
            ("keyword".into(), query.into()),
            ("page".into(), page.to_string()),
        ],
        SEARCH_FIXTURE,
    );
    let response = serde_json::from_str::<SearchResponse>(&body).unwrap_or_default();
    Paged {
        entries: response
            .search_books
            .into_iter()
            .map(title_to_item)
            .collect(),
        has_next_page: response.search_info.current_page < response.search_info.search_result_num,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let (book_id, episode_id) = key_parts(key);
    let body = api_get(
        "detail",
        vec![
            ("book_id".into(), book_id.into()),
            ("episode_id".into(), episode_id.into()),
        ],
        DETAILS_FIXTURE,
    );
    parse_details(&body, key)
}

fn fetch_chapters(key: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let (book_id, episode_id) = key_parts(key);
    let body = api_get(
        "detail",
        vec![
            ("book_id".into(), book_id.into()),
            ("episode_id".into(), episode_id.into()),
        ],
        DETAILS_FIXTURE,
    );
    let response = serde_json::from_str::<DetailsResponse>(&body).unwrap_or_default();
    response
        .book_series
        .into_iter()
        .filter(|book| !hide_locked || !book.is_locked())
        .rev()
        .map(book_to_chapter)
        .collect()
}

fn fetch_pages(key: &str) -> Vec<MangaPage> {
    let (book_id, episode_id) = key_parts(key);
    let body = api_get(
        "licenceKey",
        vec![
            ("book_id".into(), book_id.into()),
            ("episode_id".into(), episode_id.into()),
        ],
        VIEWER_FIXTURE,
    );
    let response = serde_json::from_str::<ViewerResponse>(&body).unwrap_or_default();
    let Some(book_data) = response.book_data.as_ref() else {
        return Vec::new();
    };
    let guardian_server = response.guardian_server.as_deref().unwrap_or_default();
    if guardian_server.is_empty() {
        return Vec::new();
    }
    let guardian_url = format!(
        "{}/{}",
        guardian_server.trim_end_matches('/'),
        book_data.s3_key
    );
    if book_data.imaged_reflow {
        fetch_reflow_pages(&response, &guardian_url)
    } else {
        parse_manga_pages(&response, &guardian_url)
    }
}

fn fetch_reflow_pages(response: &ViewerResponse, guardian_url: &str) -> Vec<MangaPage> {
    let signed = response
        .additional_query_string
        .as_deref()
        .unwrap_or_default();
    let body = client()
        .get(format!("{guardian_url}/book.json?{signed}"))
        .header("Referer", BASE_URL)
        .send_text()
        .unwrap_or_else(|_| REFLOW_FIXTURE.into());
    let book = serde_json::from_str::<ReflowBook>(&body).unwrap_or_default();
    let Some(profile) = book.reflow_data.and_then(|data| {
        let mut profiles = data.profiles.into_iter();
        let first = profiles.next()?;
        Some(if first.id == "mincho_small" {
            first
        } else {
            profiles
                .find(|profile| profile.id == "mincho_small")
                .unwrap_or(first)
        })
    }) else {
        return Vec::new();
    };
    let key = response
        .pages_data
        .as_ref()
        .and_then(|pages| pages.keys.get(&profile.id))
        .and_then(Value::as_str)
        .unwrap_or_default();
    (0..profile.book_info.page_count)
        .map(|index| {
            page_with_key(
                guardian_url,
                &format!("{}/{}.jpg", profile.id, index + 1),
                signed,
                key,
                index as usize,
            )
        })
        .collect()
}

fn parse_manga_pages(response: &ViewerResponse, guardian_url: &str) -> Vec<MangaPage> {
    let signed = response
        .additional_query_string
        .as_deref()
        .unwrap_or_default();
    response
        .pages_data
        .as_ref()
        .and_then(|pages| pages.keys.as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, key)| {
            page_with_key(
                guardian_url,
                &format!("{}.jpg", index + 1),
                signed,
                key,
                index,
            )
        })
        .collect()
}

fn page_with_key(
    guardian_url: &str,
    path: &str,
    signed: &str,
    key: &str,
    index: usize,
) -> MangaPage {
    let mut extra = BTreeMap::new();
    if !key.is_empty() {
        extra.insert("guardianKey".into(), json!(key));
    }
    let url = if signed.is_empty() {
        format!("{guardian_url}/{path}")
    } else {
        format!("{guardian_url}/{path}?{signed}")
    };
    MangaPage {
        content: PageContent::Url {
            url,
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        extra,
        ..MangaPage::default()
    }
}

fn parse_ranking(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<RankingResponse>(body).unwrap_or_default();
    Paged {
        entries: response
            .ranking_books
            .into_iter()
            .map(title_to_item)
            .collect(),
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let response = serde_json::from_str::<DetailsResponse>(body).unwrap_or_default();
    let detail = response.book_detail;
    let mut description = detail
        .book_review_long
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    if !detail.publishers.is_empty() {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Publisher: ");
        description.push_str(
            &detail
                .publishers
                .into_iter()
                .map(|item| item.name)
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    CatalogItem {
        key: key.into(),
        title: detail.book_name.unwrap_or_else(|| "FOD".into()),
        cover: detail.thumbnail,
        authors: detail.authors.into_iter().map(|item| item.name).collect(),
        tags: detail
            .genres
            .into_iter()
            .map(|item| item.name)
            .chain(detail.sub_genres.into_iter().map(|item| item.name))
            .collect(),
        description: (!description.is_empty()).then_some(description),
        status: ItemStatus::Unknown,
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        url: Some(format!("{BASE_URL}/books/{key}")),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn title_to_item(title: TitleResponse) -> CatalogItem {
    let key = format!("{}/{}", title.book_id, title.episode_id);
    CatalogItem {
        key: key.clone(),
        title: title.book_name,
        cover: title.thumbnail,
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        url: Some(format!("{BASE_URL}/books/{key}")),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn book_to_chapter(book: BookSeries) -> MangaChapter {
    let key = format!("{}/{}", book.book_id, book.episode_id);
    let locked = book.is_locked();
    let preview = if book.is_sample == Some(true) && locked {
        "(Preview) "
    } else {
        ""
    };
    MangaChapter {
        key: key.clone(),
        title: Some(format!(
            "{}{}{}",
            if locked { "[Locked] " } else { "" },
            preview,
            book.book_name
        )),
        chapter_number: book.episode_count.map(|value| value as f32),
        date_uploaded: book
            .episode_price_start
            .as_deref()
            .and_then(parse_jst_datetime),
        url: Some(format!("{BASE_URL}/viewer/{key}")),
        ..MangaChapter::default()
    }
}

fn api_get(path: &str, params: Vec<(String, String)>, fixture: &str) -> String {
    client()
        .get(api_url(path, &params))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
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

fn key_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .and_then(|path| {
            path.trim_start_matches('/')
                .strip_prefix("books/")
                .or_else(|| path.trim_start_matches('/').strip_prefix("viewer/"))
        })
        .map(|path| path.trim_matches('/').to_string())
}

fn key_parts(key: &str) -> (&str, &str) {
    let mut parts = key.trim_matches('/').split('/');
    (
        parts.next().unwrap_or("sample"),
        parts.next().unwrap_or("1"),
    )
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .and_then(Value::as_object)
        .and_then(|preferences| preferences.get(id))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn parse_jst_datetime(value: &str) -> Option<i64> {
    let normalized = value.get(..value.len().saturating_sub(3)).unwrap_or(value);
    let (date, time) = normalized.split_once(' ')?;
    let day_start = dates::parse_ymd(date)?;
    let mut parts = time.split(':');
    let hour = parts.next()?.parse::<i64>().ok()?;
    let minute = parts.next()?.parse::<i64>().ok()?;
    let second = parts.next()?.split('.').next()?.parse::<i64>().ok()?;
    Some(day_start + hour * 3600 + minute * 60 + second - 9 * 3600)
}

#[derive(Default, Deserialize)]
struct RankingResponse {
    #[serde(default)]
    ranking_books: Vec<TitleResponse>,
}

#[derive(Default, Deserialize)]
struct LatestResponse {
    #[serde(default)]
    current_page: i64,
    #[serde(default)]
    total: i64,
    #[serde(default)]
    new_arrival_books: Vec<TitleResponse>,
}

#[derive(Default, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    search_info: SearchInfo,
    #[serde(default)]
    search_books: Vec<TitleResponse>,
}

#[derive(Default, Deserialize)]
struct SearchInfo {
    #[serde(default)]
    current_page: i64,
    #[serde(default)]
    search_result_num: i64,
}

#[derive(Deserialize)]
struct TitleResponse {
    book_id: String,
    book_name: String,
    episode_id: String,
    #[serde(default)]
    thumbnail: Option<String>,
}

#[derive(Default, Deserialize)]
struct DetailsResponse {
    #[serde(default)]
    book_detail: BookDetail,
    #[serde(default)]
    book_series: Vec<BookSeries>,
}

#[derive(Default, Deserialize)]
struct BookDetail {
    #[serde(default)]
    book_name: Option<String>,
    #[serde(default)]
    book_review_long: Option<String>,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    authors: Vec<Named>,
    #[serde(default)]
    publishers: Vec<Named>,
    #[serde(default)]
    genres: Vec<Named>,
    #[serde(default)]
    sub_genres: Vec<Named>,
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct BookSeries {
    book_id: String,
    book_name: String,
    episode_id: String,
    #[serde(default)]
    is_purchased: Option<bool>,
    #[serde(default)]
    episode_price_start: Option<String>,
    #[serde(default)]
    is_free: Option<bool>,
    #[serde(default)]
    episode_count: Option<i64>,
    #[serde(default)]
    is_sample: Option<bool>,
}

impl BookSeries {
    fn is_locked(&self) -> bool {
        self.is_free != Some(true) && self.is_purchased != Some(true)
    }
}

#[derive(Default, Deserialize)]
struct ViewerResponse {
    #[serde(default, rename = "GUARDIAN_SERVER")]
    guardian_server: Option<String>,
    #[serde(default, rename = "ADDITIONAL_QUERY_STRING")]
    additional_query_string: Option<String>,
    #[serde(default)]
    book_data: Option<BookData>,
    #[serde(default)]
    pages_data: Option<PagesData>,
}

#[derive(Deserialize)]
struct BookData {
    s3_key: String,
    #[serde(default)]
    imaged_reflow: bool,
}

#[derive(Deserialize)]
struct PagesData {
    #[serde(default)]
    keys: Value,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReflowBook {
    #[serde(default)]
    reflow_data: Option<ReflowData>,
}

#[derive(Default, Deserialize)]
struct ReflowData {
    #[serde(default)]
    profiles: Vec<ReflowProfile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReflowProfile {
    id: String,
    book_info: ReflowBookInfo,
}

#[derive(Deserialize)]
struct ReflowBookInfo {
    #[serde(rename = "page_count")]
    page_count: i64,
}

const RANKING_FIXTURE: &str = r#"{"ranking_books":[{"book_id":"book","book_name":"Sample FOD","episode_id":"1","thumbnail":"https://img.example/cover.jpg"}]}"#;
const LATEST_FIXTURE: &str = r#"{"current_page":1,"total":1,"new_arrival_books":[{"book_id":"book","book_name":"Sample FOD","episode_id":"1","thumbnail":"https://img.example/cover.jpg"}]}"#;
const SEARCH_FIXTURE: &str = r#"{"search_info":{"current_page":1,"search_result_num":1},"search_books":[{"book_id":"book","book_name":"Sample FOD","episode_id":"1","thumbnail":"https://img.example/cover.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"book_detail":{"book_name":"Sample FOD","book_review_long":"<p>Summary</p>","thumbnail":"https://img.example/cover.jpg","authors":[{"name":"Author"}],"publishers":[{"name":"Publisher"}],"genres":[{"name":"Drama"}],"sub_genres":[{"name":"Tag"}]},"book_series":[{"book_id":"book","book_name":"Episode 1","episode_id":"1","is_purchased":true,"episode_price_start":"2024-01-01 00:00:00.000000","is_free":true,"episode_count":1,"is_sample":false}]}"#;
const VIEWER_FIXTURE: &str = r#"{"GUARDIAN_SERVER":"https://guardian.example","ADDITIONAL_QUERY_STRING":"token=1","book_data":{"s3_key":"book","imaged_reflow":false},"pages_data":{"keys":["seed"]}}"#;
const REFLOW_FIXTURE: &str =
    r#"{"reflowData":{"profiles":[{"id":"mincho_small","bookInfo":{"page_count":1}}]}}"#;

export_manga_source!(SOURCE);
