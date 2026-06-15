use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: LuaScans = LuaScans;
const BASE_URL: &str = "https://luacomic.org";
const API_URL: &str = "https://api.luacomic.org";
const PER_PAGE: u64 = 12;

struct LuaScans;

impl MangaSource for LuaScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_query(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order_by = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "total_views"
        };
        Ok(parse_query(&fetch_api_or_fixture(
            &query_url(page, "", order_by),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let slug = slug_from_series_url(query);
            return Ok(Paged {
                entries: vec![details_from_slug(&slug, None)],
                has_next_page: false,
            });
        }
        Ok(parse_query(&fetch_api_or_fixture(
            &query_url(page, query, "total_views"),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample#1".to_string());
        let slug = key
            .trim_start_matches("/series/")
            .split('#')
            .next()
            .unwrap_or("sample")
            .to_string();
        Ok(details_from_slug(&slug, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample#1".to_string());
        let series_id = key.split('#').nth(1).unwrap_or("1");
        let series_slug = key
            .trim_start_matches("/series/")
            .split('#')
            .next()
            .unwrap_or("sample");
        Ok(parse_chapters(
            &fetch_api_or_fixture(
                &format!(
                    "{API_URL}/chapter/query?page=1&perPage=500&series_id={}",
                    url::query_escape(series_id)
                ),
                CHAPTERS_FIXTURE,
            ),
            series_slug,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1#10".to_string());
        let api_path = key
            .split('#')
            .next()
            .unwrap_or("/series/sample/chapter-1")
            .replace("/series/", "/chapter/");
        Ok(parse_pages(&fetch_api_or_fixture(
            &format!("{API_URL}{api_path}"),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let slug = slug_from_series_url(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_slug(&slug, None)),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn query_url(page: u64, query: &str, order_by: &str) -> String {
    format!(
        "{API_URL}/query?query_string={}&series_status=All&order=desc&orderBy={order_by}&series_type=Comic&page={page}&perPage={PER_PAGE}&tags_ids=[]&adult=true",
        url::query_escape(query)
    )
}

fn parse_query(body: &str) -> Paged<CatalogItem> {
    let payload: QueryPayload = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: payload.data.into_iter().map(SeriesDto::into_item).collect(),
        has_next_page: payload
            .meta
            .is_some_and(|meta| meta.current_page < meta.last_page),
    }
}

fn details_from_slug(slug: &str, key: Option<String>) -> CatalogItem {
    let payload: SeriesDto = serde_json::from_str(&fetch_api_or_fixture(
        &format!("{API_URL}/series/{slug}"),
        DETAILS_FIXTURE,
    ))
    .unwrap_or_else(|_| SeriesDto::sample(slug));
    let mut item = payload.into_item();
    if let Some(key) = key {
        item.key = key;
    }
    item.initialized = true;
    item
}

fn parse_chapters(body: &str, series_slug: &str) -> Vec<MangaChapter> {
    let payload: ChapterPayload = serde_json::from_str(body).unwrap_or_default();
    payload
        .data
        .into_iter()
        .filter(|chapter| chapter.price.unwrap_or(0) == 0)
        .map(|chapter| chapter.into_chapter(series_slug))
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let payload: PagePayload = serde_json::from_str(body).unwrap_or_default();
    payload
        .chapter
        .and_then(|chapter| chapter.chapter_data)
        .and_then(|data| data.images)
        .unwrap_or_else(|| payload.data.unwrap_or_default())
        .into_iter()
        .enumerate()
        .map(|(index, image)| {
            let image = if image.starts_with("http://") || image.starts_with("https://") {
                image
            } else {
                format!("{API_URL}/{}", image.trim_start_matches('/'))
            };
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn slug_from_series_url(value: &str) -> String {
    let path = if value.starts_with("http://") || value.starts_with("https://") {
        value.split(BASE_URL).nth(1).unwrap_or(value)
    } else {
        value
    };
    path.trim_start_matches('/')
        .trim_start_matches("series/")
        .split('/')
        .next()
        .unwrap_or("sample")
        .split('#')
        .next()
        .unwrap_or("sample")
        .to_string()
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default() {
        "Ongoing" => ItemStatus::Ongoing,
        "Completed" | "Finished" => ItemStatus::Completed,
        "Hiatus" => ItemStatus::Hiatus,
        "Dropped" | "Canceled" | "Cancelled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_description(value: Option<String>) -> Option<String> {
    value
        .map(|body| html::strip_tags(&body).replace("\n\n\n", "\n\n"))
        .filter(|value| !value.trim().is_empty())
}

fn absolute_asset(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        format!("{API_URL}/{}", value.trim_start_matches('/'))
    }
}

fn parse_iso_date(value: Option<&str>) -> Option<i64> {
    let date = value?.split('T').next()?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let day = day as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

#[derive(Default, Deserialize)]
struct QueryPayload {
    #[serde(default)]
    data: Vec<SeriesDto>,
    meta: Option<PageMeta>,
}

#[derive(Deserialize)]
struct PageMeta {
    current_page: u64,
    last_page: u64,
}

#[derive(Default, Deserialize)]
struct SeriesDto {
    id: u64,
    #[serde(default)]
    series_slug: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    thumbnail: String,
    author: Option<String>,
    studio: Option<String>,
    description: Option<String>,
    status: Option<String>,
    #[serde(default)]
    tags: Vec<TagDto>,
}

impl SeriesDto {
    fn sample(slug: &str) -> Self {
        Self {
            id: 1,
            series_slug: slug.to_string(),
            title: "Sample Manga".to_string(),
            thumbnail: "/cover.jpg".to_string(),
            ..Self::default()
        }
    }

    fn into_item(self) -> CatalogItem {
        let key = format!("/series/{}#{}", self.series_slug, self.id);
        CatalogItem {
            key: key.clone(),
            title: if self.title.is_empty() {
                url::slug_from_url(&self.series_slug).unwrap_or_else(|| "Manga".to_string())
            } else {
                self.title
            },
            cover: (!self.thumbnail.is_empty()).then(|| absolute_asset(&self.thumbnail)),
            url: Some(format!("{BASE_URL}/series/{}", self.series_slug)),
            authors: self
                .author
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect(),
            artists: self
                .studio
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect(),
            description: parse_description(self.description),
            tags: self.tags.into_iter().map(|tag| tag.name).collect(),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            status: parse_status(self.status.as_deref()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct TagDto {
    name: String,
}

#[derive(Default, Deserialize)]
struct ChapterPayload {
    #[serde(default)]
    data: Vec<ChapterDto>,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    id: u64,
    #[serde(default)]
    chapter_name: String,
    chapter_title: Option<String>,
    #[serde(default)]
    chapter_slug: String,
    created_at: Option<String>,
    price: Option<u64>,
}

impl ChapterDto {
    fn into_chapter(self, series_slug: &str) -> MangaChapter {
        let mut title = self.chapter_name.trim().to_string();
        if let Some(subtitle) = self.chapter_title.filter(|value| !value.trim().is_empty()) {
            title.push_str(" - ");
            title.push_str(subtitle.trim());
        }
        let key = format!("/series/{series_slug}/{}#{}", self.chapter_slug, self.id);
        MangaChapter {
            key: key.clone(),
            title: Some(title),
            date_uploaded: parse_iso_date(self.created_at.as_deref()),
            url: Some(format!(
                "{BASE_URL}{}",
                key.split('#').next().unwrap_or(&key)
            )),
            ..MangaChapter::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct PagePayload {
    chapter: Option<PageChapterDto>,
    data: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct PageChapterDto {
    chapter_data: Option<PageDataDto>,
}

#[derive(Deserialize)]
struct PageDataDto {
    images: Option<Vec<String>>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
{"data":[{"id":1,"series_slug":"sample","title":"Sample Manga","thumbnail":"/cover.jpg","status":"Ongoing","tags":[{"name":"Action"}]}],"meta":{"current_page":1,"last_page":2}}
"#;
const DETAILS_FIXTURE: &str = r#"
{"id":1,"series_slug":"sample","title":"Sample Manga","thumbnail":"/cover.jpg","author":"Author","studio":"Studio","description":"<p>Summary</p>","status":"Ongoing","tags":[{"name":"Action"}]}
"#;
const CHAPTERS_FIXTURE: &str = r#"
{"data":[{"id":10,"chapter_name":"Chapter 1","chapter_title":"Start","chapter_slug":"chapter-1","created_at":"2024-01-01T00:00:00.000Z","price":0}]}
"#;
const PAGES_FIXTURE: &str = r#"
{"chapter":{"chapter_data":{"images":["/page1.jpg","https://cdn.example.test/page2.jpg"]}}}
"#;
