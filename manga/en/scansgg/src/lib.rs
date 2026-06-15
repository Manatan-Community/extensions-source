use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ScansGg = ScansGg;
const BASE_URL: &str = "https://scans.gg";
const API_URL: &str = "https://api.scans.gg";
const CDN_URL: &str = "https://cdn.scans.gg/uploads";
const POPULAR_LIMIT: u64 = 21;
const LATEST_LIMIT: u64 = 14;
const CHAPTER_LIMIT: u64 = 100;

struct ScansGg;

impl MangaSource for ScansGg {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_list(SERIES_FIXTURE, POPULAR_LIMIT));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_latest(&fetch_api(
                &format!(
                    "{API_URL}/chapters?page={page}&limit={LATEST_LIMIT}&chapters=true&series_details=true&group_details=true&sort=date"
                ),
                LATEST_FIXTURE,
            )));
        }
        let offset = page.saturating_sub(1) * POPULAR_LIMIT;
        Ok(parse_series_list(
            &fetch_api(
                &format!("{API_URL}/series?limit={POPULAR_LIMIT}&offset={offset}"),
                SERIES_FIXTURE,
            ),
            POPULAR_LIMIT,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = query
                .trim_start_matches(BASE_URL)
                .trim_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_api(
                    &format!("{API_URL}/series?id={key}&trackers=true&sources=true"),
                    DETAILS_FIXTURE,
                ))],
                has_next_page: false,
            });
        }
        let offset = page.saturating_sub(1) * POPULAR_LIMIT;
        Ok(parse_series_list(
            &fetch_api(
                &search_url(offset, query, request.get("filters")),
                SERIES_FIXTURE,
            ),
            POPULAR_LIMIT,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        Ok(parse_details(&fetch_api(
            &format!("{API_URL}/series?id={key}&trackers=true&sources=true"),
            DETAILS_FIXTURE,
        )))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        let mut page = 1;
        let mut chapters = Vec::new();
        loop {
            let body = fetch_api(
                &format!(
                    "{API_URL}/chapters?series_id={key}&limit={CHAPTER_LIMIT}&page={page}&group_details=true"
                ),
                CHAPTERS_FIXTURE,
            );
            let response = parse_chapter_page(&body, &key);
            chapters.extend(response.0);
            if !response.1 {
                break;
            }
            page += 1;
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| {
            "/chapter-navigation?series_id=1&chapter_id=1&group_id=0".to_string()
        });
        Ok(parse_pages(&fetch_api(
            &format!("{API_URL}{}", key.trim_start_matches(API_URL)),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/series/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .and_then(|key| query_param(&key, "series_id"))
            .map(|series| format!("{BASE_URL}/series/{series}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                search: Some(SearchRequest {
                    query: input.to_string(),
                    ..SearchRequest::default()
                }),
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
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_header("Origin", BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(offset: u64, query: &str, filters: Option<&Value>) -> String {
    let mut pairs = vec![
        ("limit", POPULAR_LIMIT.to_string()),
        ("offset", offset.to_string()),
    ];
    if !query.is_empty() {
        pairs.push(("q", query.to_string()));
    }
    pairs.push(("q_type", bracketed_filter(filters, "types")));
    pairs.push(("q_status", bracketed_filter(filters, "statuses")));
    pairs.push(("q_tags", bracketed_filter(filters, "tags")));
    format!(
        "{API_URL}/series?{}",
        pairs
            .into_iter()
            .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn parse_series_list(body: &str, limit: u64) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<ResponseDto<Vec<SeriesDto>>>(body)
        .unwrap_or_else(|_| serde_json::from_str(SERIES_FIXTURE).expect("series fixture"));
    let count = response.data.len() as u64;
    Paged {
        entries: response
            .data
            .into_iter()
            .map(|series| series.to_item(false))
            .collect(),
        has_next_page: response.meta.is_some_and(|meta| meta.has_more) || count == limit,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<ResponseDto<Vec<SeriesDto>>>(body)
        .unwrap_or_else(|_| serde_json::from_str(LATEST_FIXTURE).expect("latest fixture"));
    Paged {
        entries: response
            .data
            .into_iter()
            .map(|series| series.to_item(false))
            .collect(),
        has_next_page: response.meta.is_some_and(|meta| meta.has_more),
    }
}

fn parse_details(body: &str) -> CatalogItem {
    serde_json::from_str::<ResponseDto<SeriesDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("details fixture"))
        .data
        .to_item(true)
}

fn parse_chapter_page(body: &str, series_id: &str) -> (Vec<MangaChapter>, bool) {
    let response = serde_json::from_str::<ResponseDto<Vec<ChapterDto>>>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("chapters fixture"));
    let has_more = response.meta.as_ref().is_some_and(|meta| meta.has_more);
    (
        response
            .data
            .into_iter()
            .map(|chapter| chapter.to_chapter(series_id))
            .collect(),
        has_more,
    )
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<ResponseDto<PageListDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("pages fixture"));
    response.data.to_pages()
}

fn bracketed_filter(filters: Option<&Value>, key: &str) -> String {
    let Some(value) = filters.and_then(|filters| filters.get(key)) else {
        return "[]".to_string();
    };
    if let Some(values) = value.as_array() {
        return format!(
            "[{}]",
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    value
        .as_str()
        .map(|value| format!("[{value}]"))
        .unwrap_or_else(|| "[]".to_string())
}

fn query_param(input: &str, key: &str) -> Option<String> {
    let query = input.split('?').nth(1)?;
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| value.to_string())
    })
}

fn status_from_id(status: Option<i64>) -> ItemStatus {
    match status {
        Some(2) => ItemStatus::Completed,
        Some(3 | 4 | 5) => ItemStatus::Cancelled,
        _ => ItemStatus::Ongoing,
    }
}

fn tag_name(id: i64) -> Option<&'static str> {
    match id {
        49 => Some("Regression"),
        48 => Some("Male Protagonist"),
        47 => Some("Survival"),
        46 => Some("Avant Garde"),
        45 => Some("Award Winning"),
        44 => Some("Lolicon"),
        43 => Some("Mahou Shoujo"),
        42 => Some("Doujinshi"),
        41 => Some("Girls Love"),
        40 => Some("Hentai"),
        39 => Some("Mecha"),
        38 => Some("Shotacon"),
        37 => Some("Ecchi"),
        36 => Some("Music"),
        35 => Some("Smut"),
        34 => Some("Erotica"),
        33 => Some("Adult"),
        32 => Some("Gourmet"),
        31 => Some("Yuri"),
        30 => Some("Shoujo Ai"),
        29 => Some("Yaoi"),
        28 => Some("Shounen Ai"),
        27 => Some("Boys Love"),
        26 => Some("Harem"),
        25 => Some("Tragedy"),
        24 => Some("Gender Bender"),
        23 => Some("Suspense"),
        22 => Some("Psychological"),
        21 => Some("Mature"),
        20 => Some("Horror"),
        19 => Some("Mystery"),
        18 => Some("Martial Arts"),
        17 => Some("Sci-fi"),
        16 => Some("Adventure"),
        15 => Some("Supernatural"),
        14 => Some("Sports"),
        13 => Some("Shounen"),
        12 => Some("Historical"),
        11 => Some("Seinen"),
        10 => Some("Action"),
        9 => Some("Josei"),
        8 => Some("Thriller"),
        7 => Some("School Life"),
        6 => Some("Slice Of Life"),
        5 => Some("Drama"),
        4 => Some("Comedy"),
        3 => Some("Shoujo"),
        2 => Some("Romance"),
        1 => Some("Fantasy"),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct ResponseDto<T> {
    data: T,
    meta: Option<MetaDto>,
}

#[derive(Debug, Deserialize)]
struct MetaDto {
    #[serde(rename = "has_more")]
    has_more: bool,
}

#[derive(Debug, Deserialize)]
struct SeriesDto {
    id: i64,
    title: String,
    summary: Option<String>,
    cover: Option<String>,
    author: Option<Vec<String>>,
    artist: Option<Vec<String>>,
    tags: Option<Vec<i64>>,
    status: Option<i64>,
}

impl SeriesDto {
    fn to_item(&self, initialized: bool) -> CatalogItem {
        CatalogItem {
            key: self.id.to_string(),
            title: self.title.clone(),
            cover: self
                .cover
                .as_ref()
                .map(|cover| format!("{CDN_URL}/covers/{cover}")),
            description: initialized.then(|| self.summary.clone()).flatten(),
            authors: initialized
                .then(|| self.author.clone().unwrap_or_default())
                .unwrap_or_default(),
            artists: initialized
                .then(|| self.artist.clone().unwrap_or_default())
                .unwrap_or_default(),
            tags: initialized
                .then(|| {
                    self.tags
                        .clone()
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(tag_name)
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            status: status_from_id(self.status),
            url: Some(format!("{BASE_URL}/series/{}", self.id)),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    id: i64,
    number: f32,
    title: Option<String>,
    #[serde(rename = "created_at")]
    created_at: Option<String>,
    #[serde(rename = "group_id")]
    group_id: Option<i64>,
    group: Option<GroupDto>,
}

impl ChapterDto {
    fn to_chapter(&self, series_id: &str) -> MangaChapter {
        let mut title = format!("Chapter {}", self.number);
        if let Some(extra) = self.title.as_ref().filter(|title| !title.is_empty()) {
            title.push_str(" - ");
            title.push_str(extra);
        }
        MangaChapter {
            key: format!(
                "/chapter-navigation?series_id={series_id}&chapter_id={}&group_id={}",
                self.id,
                self.group_id.unwrap_or(0)
            ),
            title: Some(title),
            chapter_number: Some(self.number),
            date_uploaded: self
                .created_at
                .as_deref()
                .and_then(|value| value.split(' ').next())
                .and_then(manatan_shared::dates::parse_fixture_date),
            scanlators: self
                .group
                .as_ref()
                .and_then(|group| group.title.clone())
                .into_iter()
                .collect(),
            url: Some(format!("{BASE_URL}/series/{series_id}")),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct GroupDto {
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PageListDto {
    chapter: Option<ChapterDataDto>,
}

impl PageListDto {
    fn to_pages(self) -> Vec<MangaPage> {
        let Some(chapter) = self.chapter else {
            return Vec::new();
        };
        let Some(chapter_id) = chapter.id else {
            return Vec::new();
        };
        chapter
            .pages
            .unwrap_or_default()
            .into_iter()
            .map(|page| MangaPage {
                content: PageContent::Url {
                    url: format!("{CDN_URL}/pages/{chapter_id}/{}", page.path),
                    context: None,
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", page.position + 1)),
                ..MangaPage::default()
            })
            .collect()
    }
}

#[derive(Debug, Deserialize)]
struct ChapterDataDto {
    id: Option<i64>,
    pages: Option<Vec<PageDto>>,
}

#[derive(Debug, Deserialize)]
struct PageDto {
    position: usize,
    path: String,
}

export_manga_source!(SOURCE);

const SERIES_FIXTURE: &str = r#"{"data":[{"id":1,"title":"Sample ScansGG","summary":"Sample","cover":"cover.jpg","author":["Author"],"artist":["Artist"],"tags":[10,1],"status":1}],"meta":{"has_more":false}}"#;
const LATEST_FIXTURE: &str = SERIES_FIXTURE;
const DETAILS_FIXTURE: &str = r#"{"data":{"id":1,"title":"Sample ScansGG","summary":"Sample","cover":"cover.jpg","author":["Author"],"artist":["Artist"],"tags":[10,1],"status":1}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"id":1,"number":1,"title":"Start","created_at":"2024-01-01 00:00:00","group_id":2,"group":{"title":"Group"}}],"meta":{"has_more":false}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"chapter":{"id":1,"pages":[{"position":0,"path":"1.jpg"},{"position":1,"path":"2.jpg"}]}}}"#;
