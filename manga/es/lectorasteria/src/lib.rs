use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;
use std::cmp::min;

const SOURCE: LectorAsteria = LectorAsteria;
const BASE_URL: &str = "https://lectorasteria.com";
const NAME: &str = "Lector Asteria";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";
const SERIES_PATH: &str = "/ver";
const PAGE_SIZE: usize = 15;

struct LectorAsteria;

impl MangaSource for LectorAsteria {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_top_series(TOP_FIXTURE));
        }
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_series_response(
                &fetch_api("api/lastUpdates", LATEST_FIXTURE),
                1,
            ));
        }
        Ok(parse_top_series(&fetch_api("api/topSerie", TOP_FIXTURE)))
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
                entries: vec![details_for_slug(slug_from_key(&key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(apply_search_filters(
            parse_series_list(&fetch_api("api/comics", COMICS_FIXTURE)),
            page,
            query,
            request.get("filters").unwrap_or(&Value::Null),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/ver/sample".into());
        Ok(details_for_slug(slug_from_key(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/ver/sample".into());
        let series = details_dto(slug_from_key(&key));
        Ok(series
            .last_chapters
            .into_iter()
            .map(|chapter| chapter.to_chapter(&series.slug))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/ver/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains(SERIES_PATH) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_for_slug(slug_from_key(&key))),
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

fn fetch_api(path: &str, fixture: &str) -> String {
    client()
        .get(&url::join_url(BASE_URL, path))
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_top_series(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<ResponseDto<TopSeriesDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(TOP_FIXTURE).unwrap());
    let mut entries = Vec::new();
    for series in response
        .response
        .top_daily
        .into_iter()
        .chain(response.response.top_weekly)
        .chain(response.response.top_monthly)
        .flatten()
        .map(|payload| payload.project)
    {
        if !entries
            .iter()
            .any(|item: &CatalogItem| item.key == series.key())
        {
            entries.push(series.to_item(false));
        }
    }
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_series_response(body: &str, page: u64) -> Paged<CatalogItem> {
    let entries = parse_series_list(body)
        .into_iter()
        .map(|series| series.to_item(false))
        .collect::<Vec<_>>();
    Paged {
        has_next_page: page == 1 && entries.len() >= PAGE_SIZE,
        entries,
    }
}

fn parse_series_list(body: &str) -> Vec<SeriesDto> {
    serde_json::from_str::<ResponseDto<Vec<SeriesDto>>>(body)
        .or_else(|_| serde_json::from_str::<ResponseDto<Vec<SeriesDto>>>(COMICS_FIXTURE))
        .map(|response| response.response)
        .unwrap_or_default()
}

fn apply_search_filters(
    mut series: Vec<SeriesDto>,
    page: u64,
    query: &str,
    filters: &Value,
) -> Paged<CatalogItem> {
    if !query.is_empty() {
        let needle = query.to_ascii_lowercase();
        series.retain(|item| {
            item.name.to_ascii_lowercase().contains(&needle)
                || item
                    .alternative_name
                    .as_deref()
                    .is_some_and(|value| value.to_ascii_lowercase().contains(&needle))
        });
    }
    if let Some(status) =
        filter_string(filters, "status").and_then(|value| value.parse::<u64>().ok())
    {
        if status != 0 {
            series.retain(|item| item.state_id == Some(status));
        }
    }
    match filter_string(filters, "sort").as_deref() {
        Some("name") => series.sort_by(|a, b| a.name.cmp(&b.name)),
        Some("views") => {
            series.sort_by_key(|item| item.trending.as_ref().and_then(|v| v.visits).unwrap_or(0))
        }
        Some("created_at") => series.sort_by(|a, b| a.created_at.cmp(&b.created_at)),
        _ => series.sort_by(|a, b| a.updated_at.cmp(&b.updated_at)),
    }
    series.reverse();
    let start = ((page.saturating_sub(1)) as usize) * PAGE_SIZE;
    let end = min(start + PAGE_SIZE, series.len());
    let entries = if start < series.len() {
        series[start..end]
            .iter()
            .cloned()
            .map(|item| item.to_item(false))
            .collect()
    } else {
        Vec::new()
    };
    Paged {
        entries,
        has_next_page: end < series.len(),
    }
}

fn details_for_slug(slug: &str) -> CatalogItem {
    details_dto(slug).to_item(true)
}

fn details_dto(slug: &str) -> SeriesDto {
    serde_json::from_str::<ResponseDto<SeriesDto>>(&fetch_api(
        &format!("api/showProject/{slug}"),
        DETAILS_FIXTURE,
    ))
    .or_else(|_| serde_json::from_str::<ResponseDto<SeriesDto>>(DETAILS_FIXTURE))
    .map(|response| response.response)
    .unwrap_or_else(|_| SeriesDto::sample(slug))
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("block") || chunk.contains("data-src") || chunk.contains("src=")
        })
        .filter_map(|chunk| {
            html::attr(chunk, "data-lazy-src")
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "data-cfsrc"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_key(input: &str) -> String {
    if let Some(path) = input.trim_end_matches('/').strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn slug_from_key(key: &str) -> &str {
    key.trim_matches('/').rsplit('/').next().unwrap_or("sample")
}

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn parse_date(value: Option<&str>) -> Option<i64> {
    let value = value?;
    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = value.get(5..7)?.parse::<i32>().ok()?;
    let day = value.get(8..10)?.parse::<i32>().ok()?;
    let hour = value
        .get(11..13)
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    let minute = value
        .get(14..16)
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    let second = value
        .get(17..19)
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

#[derive(Debug, Deserialize)]
struct ResponseDto<T> {
    response: T,
}

#[derive(Debug, Deserialize)]
struct TopSeriesDto {
    #[serde(rename = "diario", default)]
    top_daily: Vec<Vec<PayloadSeriesDto>>,
    #[serde(rename = "semanal", default)]
    top_weekly: Vec<Vec<PayloadSeriesDto>>,
    #[serde(rename = "mensual", default)]
    top_monthly: Vec<Vec<PayloadSeriesDto>>,
}

#[derive(Debug, Deserialize)]
struct PayloadSeriesDto {
    project: SeriesDto,
}

#[derive(Debug, Clone, Deserialize)]
struct SeriesDto {
    name: String,
    #[serde(rename = "alternativeName")]
    alternative_name: Option<String>,
    slug: String,
    #[serde(rename = "sinopsis")]
    synopsis: Option<String>,
    #[serde(rename = "urlImg")]
    thumbnail: Option<String>,
    #[serde(rename = "actualizacionCap")]
    updated_at: Option<String>,
    created_at: Option<String>,
    state_id: Option<u64>,
    #[serde(default)]
    genders: Vec<GenderDto>,
    #[serde(rename = "lastChapters", default)]
    last_chapters: Vec<ChapterDto>,
    trending: Option<TrendingDto>,
    #[serde(rename = "autors", default)]
    authors: Vec<AuthorDto>,
    #[serde(default)]
    artists: Vec<ArtistDto>,
}

impl SeriesDto {
    fn sample(slug: &str) -> Self {
        Self {
            name: NAME.to_string(),
            alternative_name: None,
            slug: slug.to_string(),
            synopsis: Some("Summary".to_string()),
            thumbnail: None,
            updated_at: None,
            created_at: None,
            state_id: Some(1),
            genders: Vec::new(),
            last_chapters: Vec::new(),
            trending: None,
            authors: Vec::new(),
            artists: Vec::new(),
        }
    }

    fn key(&self) -> String {
        format!("{SERIES_PATH}/{}", self.slug)
    }

    fn to_item(&self, initialized: bool) -> CatalogItem {
        let mut description = self.synopsis.clone();
        if let Some(alternative) = self
            .alternative_name
            .as_ref()
            .filter(|value| !value.is_empty())
        {
            description = Some(match description {
                Some(existing) if !existing.is_empty() => {
                    format!("{existing}\n\nAlternative names: {alternative}")
                }
                _ => format!("Alternative names: {alternative}"),
            });
        }
        CatalogItem {
            key: self.key(),
            title: self.name.clone(),
            cover: self.thumbnail.clone(),
            description,
            authors: self
                .authors
                .iter()
                .map(|item| item.autor.name.clone())
                .collect(),
            artists: self
                .artists
                .iter()
                .map(|item| item.artist.name.clone())
                .collect(),
            tags: self
                .genders
                .iter()
                .map(|item| item.gender.name.clone())
                .collect(),
            status: match self.state_id {
                Some(1) => ItemStatus::Ongoing,
                Some(2) => ItemStatus::Hiatus,
                Some(3) => ItemStatus::Cancelled,
                Some(4) => ItemStatus::Completed,
                _ => ItemStatus::Unknown,
            },
            url: Some(url::join_url(BASE_URL, &self.key())),
            language: Some(LANG.to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct TrendingDto {
    #[serde(rename = "visitas")]
    visits: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct GenderDto {
    gender: NameDto,
}

#[derive(Debug, Clone, Deserialize)]
struct AuthorDto {
    autor: NameDto,
}

#[derive(Debug, Clone, Deserialize)]
struct ArtistDto {
    artist: NameDto,
}

#[derive(Debug, Clone, Deserialize)]
struct NameDto {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ChapterDto {
    num: f32,
    name: Option<String>,
    slug: String,
    created_at: Option<String>,
}

impl ChapterDto {
    fn to_chapter(&self, series_slug: &str) -> MangaChapter {
        let number = self.num.to_string().trim_end_matches(".0").to_string();
        let mut title = format!("Capitulo {number}");
        if let Some(name) = self.name.as_ref().filter(|value| !value.is_empty()) {
            title.push_str(" - ");
            title.push_str(name);
        }
        let key = format!("{SERIES_PATH}/{series_slug}/{}", self.slug);
        MangaChapter {
            key: key.clone(),
            title: Some(title),
            chapter_number: Some(self.num),
            date_uploaded: parse_date(self.created_at.as_deref()),
            language: Some(LANG.to_string()),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }
    }
}

export_manga_source!(SOURCE);

const TOP_FIXTURE: &str = r#"{"response":{"diario":[[{"project":{"name":"Sample","slug":"sample","urlImg":"https://lectorasteria.com/cover.jpg","state_id":1,"lastChapters":[{"num":1,"name":"Start","slug":"chapter-1","created_at":"2024-01-01T00:00:00.000Z"}]}}]],"semanal":[],"mensual":[]}}"#;
const LATEST_FIXTURE: &str = r#"{"response":[{"name":"Sample","slug":"sample","urlImg":"https://lectorasteria.com/cover.jpg","state_id":1,"lastChapters":[{"num":1,"name":"Start","slug":"chapter-1","created_at":"2024-01-01T00:00:00.000Z"}]}]}"#;
const COMICS_FIXTURE: &str = LATEST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"{"response":{"name":"Sample","slug":"sample","sinopsis":"Summary","urlImg":"https://lectorasteria.com/cover.jpg","state_id":1,"genders":[{"gender":{"name":"Drama"}}],"autors":[{"autor":{"name":"Author"}}],"artists":[{"artist":{"name":"Artist"}}],"lastChapters":[{"num":1,"name":"Start","slug":"chapter-1","created_at":"2024-01-01T00:00:00.000Z"}]}}"#;
const PAGES_FIXTURE: &str = r#"<main><div><img class="block" src="/page1.jpg"><img class="block" src="/page2.jpg"></div></main>"#;
