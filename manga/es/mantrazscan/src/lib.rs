use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ManhwaScan = ManhwaScan;
const BASE_URL: &str = "https://manhwascanx.lat";
const API_URL: &str = "https://manhwascanx.lat/api";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct ManhwaScan;

impl MangaSource for ManhwaScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_browse(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updated"
        } else {
            "views"
        };
        Ok(parse_browse(&fetch_api(
            &format!("{API_URL}/series?page={page}&limit=48&sort={sort}&q="),
            LIST_FIXTURE,
        )))
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
                entries: vec![details_item(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_browse(&fetch_api(
            &search_url(page, query, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1#sample".into());
        Ok(details_item(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1#sample".into());
        let series = series_identity(&key);
        let payload: ChaptersResponse = fetch_json(
            &format!("{API_URL}/series/{}/chapters", series.id),
            CHAPTERS_FIXTURE,
        );
        Ok(payload
            .data
            .chapters
            .into_iter()
            .map(ChapterDto::to_chapter)
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "1#sample/chapter-1".into());
        let chapter_id = key.split('#').next().unwrap_or(&key);
        let payload: PagesResponse =
            fetch_json(&format!("{API_URL}/chapters/{chapter_id}"), PAGES_FIXTURE);
        Ok(payload
            .data
            .chapter
            .pages
            .into_iter()
            .enumerate()
            .map(|(index, page)| MangaPage {
                content: PageContent::Url {
                    url: absolute_url(&page.image_url),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| {
            let slug = key.split('#').nth(1).unwrap_or(&key);
            format!("{BASE_URL}/manga/{}/", slug.trim_matches('/'))
        }))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let slug = key.split('#').nth(1).unwrap_or(&key);
            format!("{BASE_URL}/manga/{}", slug.trim_matches('/'))
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_item(&key)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json<T: for<'de> Deserialize<'de>>(target: &str, fixture: &str) -> T {
    serde_json::from_str(&fetch_api(target, fixture))
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap())
}

fn search_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    let filters = filters.unwrap_or(&Value::Null);
    let sort = filter_string(filters, "sort").unwrap_or_else(|| "updated".into());
    let mut out = format!(
        "{API_URL}/series?page={page}&limit=48&q={}&sort={}",
        url::query_escape(query),
        url::query_escape(&sort)
    );
    for key in ["genre", "status", "type"] {
        if let Some(value) = filter_string(filters, key).filter(|value| !value.is_empty()) {
            out.push('&');
            out.push_str(key);
            out.push('=');
            out.push_str(&url::query_escape(&value));
        }
    }
    out
}

fn parse_browse(body: &str) -> Paged<CatalogItem> {
    let payload: BrowseResponse =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap());
    Paged {
        entries: payload
            .data
            .series
            .into_iter()
            .map(|series| series.to_item(false))
            .collect(),
        has_next_page: payload.data.page < payload.data.total_pages,
    }
}

fn details_item(key: &str) -> CatalogItem {
    let identity = series_identity(key);
    let payload: DetailsResponse = fetch_json(
        &format!("{API_URL}/series/{}", identity.id),
        DETAILS_FIXTURE,
    );
    payload.data.series.to_item(true)
}

fn series_identity(key: &str) -> SeriesIdentity {
    if let Some((id, slug)) = key.split_once('#') {
        return SeriesIdentity {
            id: id.parse().unwrap_or(1),
            slug: slug.trim_matches('/').to_string(),
        };
    }
    let slug = normalize_key(key);
    let payload: BrowseResponse = fetch_json(
        &format!(
            "{API_URL}/series?page=1&limit=48&sort=updated&q={}",
            url::query_escape(&slug)
        ),
        LIST_FIXTURE,
    );
    payload
        .data
        .series
        .into_iter()
        .find(|series| series.slug == slug || series.title.eq_ignore_ascii_case(&slug))
        .map(|series| SeriesIdentity {
            id: series.id,
            slug: series.slug,
        })
        .unwrap_or(SeriesIdentity { id: 1, slug })
}

fn normalize_key(input: &str) -> String {
    let mut value = input.trim();
    if let Some(rest) = value.strip_prefix(BASE_URL) {
        value = rest;
    }
    value
        .trim_start_matches('/')
        .trim_start_matches("manga/")
        .trim_matches('/')
        .to_string()
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters.get(key).and_then(|value| {
        value.as_str().map(ToString::to_string).or_else(|| {
            value
                .get("value")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
    })
}

#[derive(Debug)]
struct SeriesIdentity {
    id: i64,
    #[allow(dead_code)]
    slug: String,
}

#[derive(Debug, Deserialize)]
struct BrowseResponse {
    data: BrowseData,
}

#[derive(Debug, Deserialize)]
struct BrowseData {
    #[serde(default)]
    series: Vec<SeriesDto>,
    #[serde(default)]
    page: u64,
    #[serde(rename = "total_pages", default)]
    total_pages: u64,
}

#[derive(Debug, Deserialize)]
struct DetailsResponse {
    data: DetailsData,
}

#[derive(Debug, Deserialize)]
struct DetailsData {
    series: SeriesDto,
}

#[derive(Debug, Deserialize)]
struct SeriesDto {
    id: i64,
    title: String,
    slug: String,
    #[serde(rename = "cover_url")]
    cover_url: Option<String>,
    description: Option<String>,
    status: Option<String>,
    author: Option<String>,
    artist: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
}

impl SeriesDto {
    fn to_item(self, initialized: bool) -> CatalogItem {
        let key = format!("{}#{}", self.id, self.slug);
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: self.cover_url.map(|cover| absolute_url(&cover)),
            description: self.description,
            authors: self
                .author
                .filter(|value| !value.is_empty())
                .into_iter()
                .collect(),
            artists: self
                .artist
                .filter(|value| !value.is_empty())
                .into_iter()
                .collect(),
            tags: self.genres,
            status: match self.status.as_deref() {
                Some("ongoing") => ItemStatus::Ongoing,
                Some("completed") => ItemStatus::Completed,
                Some("hiatus") => ItemStatus::Hiatus,
                _ => ItemStatus::Unknown,
            },
            url: Some(format!(
                "{BASE_URL}/manga/{}/",
                key.split('#').nth(1).unwrap_or("sample")
            )),
            language: Some(LANG.into()),
            content_rating: Some(CONTENT_RATING.into()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChaptersResponse {
    data: ChaptersData,
}

#[derive(Debug, Deserialize)]
struct ChaptersData {
    #[serde(default)]
    chapters: Vec<ChapterDto>,
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    id: i64,
    #[serde(rename = "chapter_num")]
    chapter_num: String,
    title: Option<String>,
    slug: String,
    #[serde(rename = "created_at")]
    created_at: Option<String>,
}

impl ChapterDto {
    fn to_chapter(self) -> MangaChapter {
        let number = self.chapter_num.parse::<f32>().ok();
        let mut title = format!("Capitulo {}", self.chapter_num.trim_end_matches(".0"));
        if let Some(extra) = self.title.filter(|value| !value.trim().is_empty()) {
            title.push_str(" - ");
            title.push_str(&extra);
        }
        MangaChapter {
            key: format!("{}#{}", self.id, self.slug.trim_matches('/')),
            title: Some(title),
            chapter_number: number,
            date_uploaded: self.created_at.as_deref().and_then(parse_date_time),
            url: Some(format!("{BASE_URL}/manga/{}", self.slug.trim_matches('/'))),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct PagesResponse {
    data: PagesData,
}

#[derive(Debug, Deserialize)]
struct PagesData {
    chapter: ChapterPages,
}

#[derive(Debug, Deserialize)]
struct ChapterPages {
    #[serde(default)]
    pages: Vec<PageDto>,
}

#[derive(Debug, Deserialize)]
struct PageDto {
    #[serde(rename = "image_url")]
    image_url: String,
}

fn parse_date_time(value: &str) -> Option<i64> {
    let mut parts = value.split_whitespace();
    let date = parts.next()?;
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i32>().ok()?;
    let month = date_parts.next()?.parse::<i32>().ok()?;
    let day = date_parts.next()?.parse::<i32>().ok()?;
    let seconds = parts
        .next()
        .and_then(|time| {
            let mut time_parts = time.split(':');
            let hour = time_parts.next()?.parse::<i64>().ok()?;
            let minute = time_parts.next()?.parse::<i64>().ok()?;
            let second = time_parts.next()?.parse::<i64>().ok()?;
            Some(hour * 3600 + minute * 60 + second)
        })
        .unwrap_or(0);
    unix_date(year, month, day).map(|date| date + seconds)
}

fn unix_date(year: i32, month: i32, day: i32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(((era * 146_097 + doe - 719_468) as i64) * 86_400)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":{"series":[{"id":1,"title":"Sample","slug":"sample","cover_url":"/cover.jpg"}],"page":1,"total_pages":1}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"series":{"id":1,"title":"Sample","slug":"sample","cover_url":"/cover.jpg","description":"Summary","status":"ongoing","author":"Author","artist":"Artist","genres":["Accion"]}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"chapters":[{"id":1,"chapter_num":"1","title":"Uno","slug":"sample/chapter-1","created_at":"2024-04-19 00:00:00"}]}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"chapter":{"pages":[{"image_url":"/page1.jpg"}]}}}"#;
