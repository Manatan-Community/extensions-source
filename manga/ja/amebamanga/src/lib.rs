use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, manga_image, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const SOURCE: AmebaManga = AmebaManga;
const BASE_URL: &str = "https://dokusho-ojikan.jp";
const API_URL: &str = "https://api.dokusho-ojikan.jp/dokusho-server";
const PAGE_SIZE: u64 = 50;

struct AmebaManga;

impl MangaSource for AmebaManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_titles(TITLES_FIXTURE, false));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(fetch_latest(page));
        }
        Ok(fetch_popular(page))
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
        Ok(fetch_search(query, page, &request))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let hide_locked = preference_bool(&request, "hide_locked");
        Ok(fetch_chapters(&title_id_from_key(&key), hide_locked))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1".into());
        Ok(fetch_pages(&chapter_id_from_key(&key)))
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
        Ok(manga::request_key(&request, "manga").map(|key| manga_url(&title_id_from_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            format!(
                "{BASE_URL}/reader/index.html?cid={}",
                chapter_id_from_key(&key)
            )
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            let item = (!key.contains("cid=")).then(|| details_by_key(&key));
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
        .with_header("Cookie", "AC=1")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_popular(page: u64) -> Paged<CatalogItem> {
    let offset = page.saturating_sub(1) * PAGE_SIZE;
    let body = api_get(
        "rank/title/category",
        vec![
            ("ac".into(), "1".into()),
            ("term_code".into(), "monthly".into()),
            ("category".into(), "page_type_all".into()),
            ("offset".into(), offset.to_string()),
            ("limit".into(), PAGE_SIZE.to_string()),
        ],
        RANKING_FIXTURE,
    );
    let response = serde_json::from_str::<RankingResponse>(&body).unwrap_or_default();
    Paged {
        entries: response
            .title_rank_responses
            .into_iter()
            .map(title_to_item)
            .collect(),
        has_next_page: offset + PAGE_SIZE < response.total_count as u64,
    }
}

fn fetch_latest(page: u64) -> Paged<CatalogItem> {
    let offset = page.saturating_sub(1) * PAGE_SIZE;
    let body = api_get(
        "release/book/recent",
        vec![
            ("ac".into(), "1".into()),
            ("category".into(), "page_type_all".into()),
            ("sort".into(), "releaseDate".into()),
            ("offset".into(), offset.to_string()),
            ("limit".into(), PAGE_SIZE.to_string()),
        ],
        LATEST_FIXTURE,
    );
    parse_titles(&body, false)
}

fn fetch_search(query: &str, page: u64, request: &Value) -> Paged<CatalogItem> {
    let offset = page.saturating_sub(1) * PAGE_SIZE;
    let mut params = vec![
        ("ac".into(), "1".into()),
        ("word".into(), query.into()),
        ("offset".into(), offset.to_string()),
        ("limit".into(), PAGE_SIZE.to_string()),
    ];
    append_filter(request, &mut params, "sort", "sort_key");
    append_filter(request, &mut params, "genre_id", "genre_id");
    append_filter(request, &mut params, "pub_id", "pub_id");
    if let Some(volume) = filter_string(request, "volume").filter(|value| !value.is_empty()) {
        if let Some((key, value)) = volume.split_once('|') {
            params.push((key.into(), value.into()));
        }
    }
    let body = api_get("search/search/v2", params, LATEST_FIXTURE);
    parse_titles(&body, false)
}

fn details_by_key(key: &str) -> CatalogItem {
    let id = title_id_from_key(key);
    let body = api_get(
        &format!("titles/{id}"),
        vec![("ac".into(), "1".into())],
        DETAILS_FIXTURE,
    );
    parse_details(&body, &id)
}

fn fetch_chapters(title_id: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let params = vec![
        ("ac".into(), "1".into()),
        ("title_id".into(), title_id.into()),
        ("sales_status".into(), "IN_RESERVATION".into()),
        ("sales_status".into(), "ON_SALE".into()),
        ("sort".into(), "VOL_DESC".into()),
        ("offset".into(), "0".into()),
        ("limit".into(), "1000".into()),
    ];
    let body = api_get("books/by_title/v3", params, CHAPTERS_FIXTURE);
    let response = serde_json::from_str::<ChapterResponse>(&body).unwrap_or_default();
    let book_ids = response
        .books
        .iter()
        .map(|book| book.id)
        .collect::<Vec<_>>();
    let owned = fetch_owned_book_ids(&book_ids);
    response
        .books
        .into_iter()
        .filter(|book| !hide_locked || !book.is_locked_for(owned.as_ref()))
        .map(|book| book_to_chapter(book, owned.as_ref()))
        .collect()
}

fn fetch_owned_book_ids(book_ids: &[i64]) -> Option<BTreeSet<i64>> {
    if book_ids.is_empty() {
        return None;
    }
    let params = book_ids
        .iter()
        .map(|id| ("book_id".to_string(), id.to_string()))
        .collect::<Vec<_>>();
    let body = client()
        .get(api_url("user_books/me/by_book/v2", &params))
        .xhr()
        .send_text()
        .ok()?;
    let response = serde_json::from_str::<OwnedResponse>(&body).ok()?;
    Some(
        response
            .user_books
            .into_iter()
            .filter(|book| book.possession_status == "OWNED")
            .map(|book| book.book_id)
            .collect(),
    )
}

fn fetch_pages(book_id: &str) -> Vec<MangaPage> {
    let body = api_get(
        "browser/bookinfo/v3",
        vec![("bookId".into(), book_id.into())],
        VIEWER_FIXTURE,
    );
    let response = serde_json::from_str::<ViewerResponse>(&body).unwrap_or_default();
    let result = response.result;
    let guardian_url = format!(
        "{}/{}",
        result.guardian_server.trim_end_matches('/'),
        result.book_data.s3_key.as_str()
    );
    if result.book_data.imaged_reflow {
        fetch_reflow_pages(&result, &guardian_url)
    } else {
        parse_manga_pages(&result, &guardian_url)
    }
}

fn fetch_reflow_pages(result: &ViewerResult, guardian_url: &str) -> Vec<MangaPage> {
    let body = client()
        .get(format!("{guardian_url}/book.json?{}", result.signed_params))
        .header("Referer", BASE_URL)
        .send_text()
        .unwrap_or_else(|_| REFLOW_FIXTURE.to_string());
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
    let key = result
        .keys
        .get(&profile.id)
        .and_then(Value::as_str)
        .unwrap_or_default();
    (0..profile.book_info.page_count)
        .map(|index| {
            page_with_key(
                guardian_url,
                &format!("{}/{}.jpg", profile.id, index + 1),
                &result.signed_params,
                key,
                index as usize,
            )
        })
        .collect()
}

fn parse_manga_pages(result: &ViewerResult, guardian_url: &str) -> Vec<MangaPage> {
    result
        .keys
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, key)| {
            page_with_key(
                guardian_url,
                &format!("{}.jpg", index + 1),
                &result.signed_params,
                key,
                index,
            )
        })
        .collect()
}

fn page_with_key(
    guardian_url: &str,
    path: &str,
    signed_params: &str,
    key: &str,
    index: usize,
) -> MangaPage {
    let mut extra = BTreeMap::new();
    if !key.is_empty() {
        extra.insert("guardianKey".into(), json!(key));
    }
    MangaPage {
        content: PageContent::Url {
            url: format!("{guardian_url}/{path}?{signed_params}"),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        extra,
        ..MangaPage::default()
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

fn parse_titles(body: &str, default_has_next: bool) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<LatestResponse>(body).unwrap_or_default();
    let has_next_page = response.offset + (response.books.len() as i64) < response.total_count;
    Paged {
        entries: response.books.into_iter().map(title_to_item).collect(),
        has_next_page: has_next_page || default_has_next,
    }
}

fn parse_details(body: &str, title_id: &str) -> CatalogItem {
    let details = serde_json::from_str::<DetailsResponse>(body).unwrap_or_default();
    let mut description = details.description.unwrap_or_default();
    if let Some(pub_info) = details.pub_info {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Publisher: ");
        description.push_str(&pub_info.name);
    }
    if details.erotic_type == Some(4) {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("18+");
    }
    CatalogItem {
        key: title_id.into(),
        title: details.name.unwrap_or_else(|| "Ameba Manga".into()),
        cover: details.image_url,
        authors: details
            .authors
            .into_iter()
            .flatten()
            .map(|author| author.name)
            .collect(),
        tags: details
            .categories
            .into_iter()
            .flatten()
            .map(|category| category.name)
            .chain(
                details
                    .meta_list
                    .into_iter()
                    .flatten()
                    .map(|meta| meta.label),
            )
            .collect(),
        description: (!description.is_empty()).then_some(description),
        status: if details.complete_flg == Some(true) {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        url: Some(manga_url(title_id)),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn title_to_item(title: TitleResponse) -> CatalogItem {
    let id = title.title_id.to_string();
    CatalogItem {
        key: id.clone(),
        title: title.title_name,
        cover: title
            .image_url
            .or_else(|| title.max_book.and_then(|book| book.image_url)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        url: Some(manga_url(&id)),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn book_to_chapter(book: Book, owned: Option<&BTreeSet<i64>>) -> MangaChapter {
    let locked = book.is_locked_for(owned);
    MangaChapter {
        key: book.id.to_string(),
        title: Some(if locked {
            format!("[Locked] {}", book.contents_name)
        } else {
            book.contents_name
        }),
        chapter_number: book.vol.map(|vol| vol as f32),
        date_uploaded: book
            .start_datetime
            .as_deref()
            .and_then(parse_datetime_with_offset),
        url: Some(format!("{BASE_URL}/reader/index.html?cid={}", book.id)),
        ..MangaChapter::default()
    }
}

fn manga_url(title_id: &str) -> String {
    format!("{BASE_URL}/series_list/series_id={title_id}")
}

fn title_id_from_key(key: &str) -> String {
    if let Some(id) = key.split("series_id=").nth(1) {
        return id.split(['&', '/', '?']).next().unwrap_or("1").to_string();
    }
    key.trim_matches('/')
        .split('/')
        .next()
        .unwrap_or("1")
        .to_string()
}

fn chapter_id_from_key(key: &str) -> String {
    if let Some(id) = key.split("cid=").nth(1) {
        return id.split(['&', '/', '?']).next().unwrap_or("1").to_string();
    }
    key.trim_matches('/')
        .split('/')
        .next()
        .unwrap_or("1")
        .to_string()
}

fn key_from_url(input: &str) -> Option<String> {
    let rest = input.strip_prefix(BASE_URL)?;
    if rest.contains("series_id=") {
        return Some(title_id_from_key(rest));
    }
    if rest.contains("cid=") {
        return Some(format!("cid={}", chapter_id_from_key(rest)));
    }
    None
}

fn append_filter(
    request: &Value,
    params: &mut Vec<(String, String)>,
    filter_id: &str,
    api_id: &str,
) {
    if let Some(value) = filter_string(request, filter_id).filter(|value| !value.is_empty()) {
        params.push((api_id.into(), value));
    }
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .and_then(Value::as_object)
        .and_then(|preferences| preferences.get(id))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn parse_datetime_with_offset(value: &str) -> Option<i64> {
    let (date, time_with_zone) = value.split_once('T')?;
    let day_start = dates::parse_ymd(date)?;
    let time = &time_with_zone[..8.min(time_with_zone.len())];
    let mut parts = time.split(':');
    let hour = parts.next()?.parse::<i64>().ok()?;
    let minute = parts.next()?.parse::<i64>().ok()?;
    let second = parts.next()?.parse::<i64>().ok()?;
    Some(day_start + hour * 3600 + minute * 60 + second - 9 * 3600)
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RankingResponse {
    #[serde(default)]
    total_count: i64,
    #[serde(default)]
    title_rank_responses: Vec<TitleResponse>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LatestResponse {
    #[serde(default)]
    total_count: i64,
    #[serde(default)]
    offset: i64,
    #[serde(default, alias = "results")]
    books: Vec<TitleResponse>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TitleResponse {
    title_id: i64,
    title_name: String,
    #[serde(default)]
    image_url: Option<String>,
    #[serde(default)]
    max_book: Option<MaxBook>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MaxBook {
    #[serde(default)]
    image_url: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailsResponse {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    image_url: Option<String>,
    #[serde(default)]
    categories: Option<Vec<Named>>,
    #[serde(default, rename = "pub")]
    pub_info: Option<Named>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    authors: Option<Vec<Named>>,
    #[serde(default)]
    complete_flg: Option<bool>,
    #[serde(default)]
    meta_list: Option<Vec<Meta>>,
    #[serde(default)]
    erotic_type: Option<i64>,
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct Meta {
    label: String,
}

#[derive(Default, Deserialize)]
struct ChapterResponse {
    #[serde(default)]
    books: Vec<Book>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Book {
    id: i64,
    contents_name: String,
    #[serde(default)]
    vol: Option<i64>,
    #[serde(default)]
    discount: Option<Discount>,
    #[serde(default)]
    start_datetime: Option<String>,
}

impl Book {
    fn is_locked_for(&self, owned: Option<&BTreeSet<i64>>) -> bool {
        if let Some(owned) = owned {
            !owned.contains(&self.id)
        } else {
            self.discount
                .as_ref()
                .is_some_and(|discount| discount.kind != "FREE")
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Discount {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OwnedResponse {
    #[serde(default)]
    user_books: Vec<UserBook>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserBook {
    book_id: i64,
    possession_status: String,
}

#[derive(Default, Deserialize)]
struct ViewerResponse {
    #[serde(default)]
    result: ViewerResult,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ViewerResult {
    #[serde(default)]
    guardian_server: String,
    #[serde(default)]
    signed_params: String,
    #[serde(default)]
    book_data: BookData,
    #[serde(default)]
    keys: Value,
}

#[derive(Default, Deserialize)]
struct BookData {
    #[serde(default, rename = "s3_key")]
    s3_key: String,
    #[serde(default, rename = "imaged_reflow", alias = "imagedReflow")]
    imaged_reflow: bool,
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

const RANKING_FIXTURE: &str = r#"{"totalCount":1,"offset":0,"titleRankResponses":[{"titleId":1,"titleName":"Sample Ameba","imageUrl":"https://img.example/cover.jpg"}]}"#;
const LATEST_FIXTURE: &str = r#"{"totalCount":1,"offset":0,"results":[{"titleId":1,"titleName":"Sample Ameba","imageUrl":"https://img.example/cover.jpg"}]}"#;
const TITLES_FIXTURE: &str = LATEST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"{"name":"Sample Ameba","imageUrl":"https://img.example/cover.jpg","categories":[{"name":"Drama"}],"pub":{"name":"Publisher"},"description":"Summary","authors":[{"name":"Author"}],"completeFlg":false,"metaList":[{"label":"Tag"}],"eroticType":0}"#;
const CHAPTERS_FIXTURE: &str = r#"{"books":[{"id":10,"contentsName":"Volume 1","vol":1,"discount":{"type":"FREE"},"startDatetime":"2024-01-01T00:00:00+09:00"}]}"#;
const VIEWER_FIXTURE: &str = r#"{"result":{"guardianServer":"https://guardian.example","signedParams":"token=1","bookData":{"s3_key":"book","imaged_reflow":false},"keys":["seed"]}}"#;
const REFLOW_FIXTURE: &str =
    r#"{"reflowData":{"profiles":[{"id":"mincho_small","bookInfo":{"page_count":1}}]}}"#;

export_manga_source!(SOURCE);
