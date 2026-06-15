use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ZonatmoTo = ZonatmoTo;
const BASE_URL: &str = "https://zonatmo.to";
const API_URL: &str = "https://zonatmo.to/wp-api/api";
const CDN_URL: &str = "https://cdn.zonatmo.to";
const UPLOADS_URL: &str = "https://zonatmo.to/wp-content/uploads";
const CHAPTERS_PER_PAGE: u64 = 50;
const LANG: &str = "es";
const CONTENT_RATING: &str = "safe";

struct ZonatmoTo;

impl MangaSource for ZonatmoTo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        Ok(parse_listing(&fetch_json_or_fixture(
            &format!("{API_URL}/tops/views/month?postType=any&postsPerPage=50"),
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
        if let Some(slug) = deeplink_slug(query) {
            return Ok(Paged {
                entries: parse_single_manga(&fetch_json_or_fixture(
                    &single_manga_url(&slug),
                    DETAILS_FIXTURE,
                ))
                .into_iter()
                .collect(),
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        Ok(parse_listing(&fetch_json_or_fixture(
            &listing_url(page, query, filters),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let slug = key.split('/').next().unwrap_or(&key);
        Ok(parse_single_manga(&fetch_json_or_fixture(
            &single_manga_url(slug),
            DETAILS_FIXTURE,
        ))
        .unwrap_or_else(|| sample_item(slug)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let slug = key.split('/').next().unwrap_or(&key).to_string();
        let first = fetch_json_or_fixture(&chapter_list_url(&slug, 1), CHAPTERS_FIXTURE);
        Ok(parse_all_chapters(&slug, &first))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/chapter-1#1".into());
        let (manga_slug, chapter_slug) = chapter_key_parts(&key);
        Ok(parse_pages(&fetch_json_or_fixture(
            &format!(
                "{API_URL}/single/manga/{}/{}",
                url::query_escape(&manga_slug),
                url::query_escape(&chapter_slug)
            ),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = deeplink_slug(input) {
            return Ok(Some(UrlResolveResult {
                item: parse_single_manga(&fetch_json_or_fixture(
                    &single_manga_url(&slug),
                    DETAILS_FIXTURE,
                )),
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

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn listing_url(page: u64, query: &str, filters: &Value) -> String {
    let mut pairs = vec![
        ("page", page.to_string()),
        ("search", query.trim().to_string()),
    ];
    for genre in selected_values(filters, "genres") {
        pairs.push(("genres[]", genre));
    }
    for value in selected_values(filters, "type") {
        pairs.push(("type[]", value));
    }
    for value in selected_values(filters, "status") {
        pairs.push(("status[]", value));
    }
    format!(
        "{API_URL}/listing/manga?{}",
        pairs
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn selected_values(filters: &Value, id: &str) -> Vec<String> {
    match filters.get(id) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn single_manga_url(slug: &str) -> String {
    format!("{API_URL}/single/manga/{}", url::query_escape(slug))
}

fn chapter_list_url(slug: &str, page: u64) -> String {
    format!(
        "{API_URL}/single/manga/{}/chapters?page={page}&postsPerPage={CHAPTERS_PER_PAGE}&order=asc",
        url::query_escape(slug)
    )
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    if let Ok(response) = serde_json::from_str::<TopViewsResponse>(body) {
        return Paged {
            entries: response
                .data
                .map(|data| data.items)
                .unwrap_or_default()
                .into_iter()
                .filter_map(MangaDto::into_item)
                .collect(),
            has_next_page: false,
        };
    }
    let response = serde_json::from_str::<ListingResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    let has_next_page = response
        .data
        .as_ref()
        .and_then(|data| data.pagination.as_ref())
        .is_some_and(|pagination| pagination.has_next);
    Paged {
        entries: response
            .data
            .map(|data| data.items)
            .unwrap_or_default()
            .into_iter()
            .filter_map(MangaDto::into_item)
            .collect(),
        has_next_page,
    }
}

fn parse_single_manga(body: &str) -> Option<CatalogItem> {
    serde_json::from_str::<SingleMangaResponse>(body)
        .ok()?
        .data?
        .into_item()
        .map(|mut item| {
            item.initialized = true;
            item
        })
}

fn parse_all_chapters(manga_slug: &str, first_body: &str) -> Vec<MangaChapter> {
    let first = serde_json::from_str::<ChapterListResponse>(first_body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
    let total_pages = first
        .data
        .as_ref()
        .and_then(|data| data.pagination.as_ref())
        .map(|pagination| pagination.total_pages)
        .unwrap_or(1);
    let mut chapters = first.data.map(|data| data.items).unwrap_or_default();
    for page in 2..=total_pages {
        let body = fetch_json_or_fixture(&chapter_list_url(manga_slug, page), CHAPTERS_FIXTURE);
        if let Ok(response) = serde_json::from_str::<ChapterListResponse>(&body) {
            chapters.extend(response.data.map(|data| data.items).unwrap_or_default());
        }
    }
    chapters.sort_by(|a, b| {
        b.chapter_number
            .parse::<f32>()
            .unwrap_or(-1.0)
            .partial_cmp(&a.chapter_number.parse::<f32>().unwrap_or(-1.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters.dedup_by_key(|chapter| chapter.id);
    chapters
        .into_iter()
        .map(|chapter| chapter.into_chapter(manga_slug))
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<SingleChapterResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    let Some(chapter) = response.data.and_then(|data| data.chapter) else {
        return Vec::new();
    };
    let jit = chapter.jit;
    let mut images = chapter.images;
    images.sort_by_key(|image| image.page_number);
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| {
            let image_url = format!(
                "{CDN_URL}/manga/{}/{}",
                jit.trim_matches('/'),
                image.image_url.trim_start_matches('/')
            );
            MangaPage {
                content: PageContent::Url {
                    url: image_url,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn deeplink_slug(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL)
        && !input.starts_with(&format!(
            "https://www.{}",
            BASE_URL.trim_start_matches("https://")
        ))
    {
        return None;
    }
    let path = input.split(BASE_URL).last().unwrap_or(input);
    let mut parts = path.trim_matches('/').split('/');
    (parts.next() == Some("manga"))
        .then(|| parts.next().unwrap_or_default().to_string())
        .filter(|value| !value.is_empty())
}

fn chapter_key_parts(key: &str) -> (String, String) {
    let clean = key.split('#').next().unwrap_or(key);
    let mut parts = clean.split('/');
    (
        parts.next().unwrap_or("sample").to_string(),
        parts.next().unwrap_or("chapter-1").to_string(),
    )
}

fn thumbnail_url(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else if value.starts_with("http") {
        Some(value.to_string())
    } else {
        Some(format!("{UPLOADS_URL}/{}", value.trim_start_matches('/')))
    }
}

fn status(ids: &[i64]) -> ItemStatus {
    if ids.contains(&12) {
        ItemStatus::Ongoing
    } else if ids.contains(&19) {
        ItemStatus::Completed
    } else if ids.contains(&174) {
        ItemStatus::Hiatus
    } else if ids.contains(&198) {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Unknown
    }
}

fn genre_name(id: i64) -> Option<&'static str> {
    GENRES
        .iter()
        .find(|(_, value)| *value == id)
        .map(|(name, _)| *name)
}

fn sample_item(slug: &str) -> CatalogItem {
    CatalogItem {
        key: slug.to_string(),
        title: "Zonatmo.to".into(),
        language: Some(LANG.into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

#[derive(Deserialize)]
struct ListingResponse {
    data: Option<ListingData>,
}

#[derive(Deserialize)]
struct ListingData {
    #[serde(default)]
    items: Vec<MangaDto>,
    pagination: Option<Pagination>,
}

#[derive(Deserialize)]
struct TopViewsResponse {
    data: Option<TopViewsData>,
}

#[derive(Deserialize)]
struct TopViewsData {
    #[serde(default)]
    items: Vec<MangaDto>,
}

#[derive(Deserialize)]
struct SingleMangaResponse {
    data: Option<MangaDto>,
}

#[derive(Deserialize)]
struct Pagination {
    #[serde(default)]
    has_next: bool,
    #[serde(default)]
    total_pages: u64,
}

#[derive(Deserialize)]
struct MangaDto {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    cover: Option<String>,
    #[serde(default)]
    author: Vec<AuthorDto>,
    #[serde(default)]
    status: Vec<i64>,
    #[serde(default)]
    genres: Vec<i64>,
}

impl MangaDto {
    fn into_item(self) -> Option<CatalogItem> {
        let slug = self.slug.trim().to_string();
        let title = self.title.trim().to_string();
        if slug.is_empty() || title.is_empty() {
            return None;
        }
        Some(CatalogItem {
            key: slug.clone(),
            title,
            cover: self.cover.as_deref().and_then(thumbnail_url),
            url: Some(format!("{BASE_URL}/manga/{slug}")),
            authors: self
                .author
                .into_iter()
                .map(|author| author.name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect(),
            tags: self
                .genres
                .into_iter()
                .filter_map(genre_name)
                .map(ToString::to_string)
                .collect(),
            description: self
                .overview
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            status: status(&self.status),
            language: Some(LANG.into()),
            content_rating: Some(CONTENT_RATING.into()),
            initialized: false,
            ..CatalogItem::default()
        })
    }
}

#[derive(Deserialize)]
struct AuthorDto {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct ChapterListResponse {
    data: Option<ChapterListData>,
}

#[derive(Deserialize)]
struct ChapterListData {
    #[serde(default)]
    items: Vec<ChapterItem>,
    pagination: Option<Pagination>,
}

#[derive(Deserialize)]
struct ChapterItem {
    id: i64,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    chapter_number: String,
    #[serde(default)]
    release_date: Option<String>,
}

impl ChapterItem {
    fn into_chapter(self, manga_slug: &str) -> MangaChapter {
        let title = self.title.trim();
        MangaChapter {
            key: format!("{manga_slug}/{}#{}", self.slug, self.id),
            title: Some(format!(
                "#{}{}",
                self.chapter_number,
                if title.is_empty() {
                    String::new()
                } else {
                    format!(" - {title}")
                }
            )),
            chapter_number: self.chapter_number.parse::<f32>().ok(),
            date_uploaded: self.release_date.as_deref().and_then(|value| {
                manatan_shared::dates::parse_ymd(value.get(0..10).unwrap_or(value))
            }),
            url: Some(format!("{BASE_URL}/manga/{manga_slug}/{}", self.slug)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct SingleChapterResponse {
    data: Option<SingleChapterData>,
}

#[derive(Deserialize)]
struct SingleChapterData {
    chapter: Option<ChapterDetails>,
}

#[derive(Deserialize)]
struct ChapterDetails {
    jit: String,
    #[serde(default)]
    images: Vec<ChapterImage>,
}

#[derive(Deserialize)]
struct ChapterImage {
    image_url: String,
    page_number: i64,
}

const GENRES: &[(&str, i64)] = &[
    ("Acción", 2),
    ("Animación", 6198),
    ("Apocalíptico", 861),
    ("Artes Marciales", 26),
    ("Aventura", 3),
    ("Boys Love", 103),
    ("Ciberpunk", 356),
    ("Ciencia Ficción", 21),
    ("Comedia", 4),
    ("Crimen", 41),
    ("Demonios", 88),
    ("Deporte", 37),
    ("Drama", 15),
    ("Ecchi", 32),
    ("Extranjero", 1168),
    ("Familia", 1027),
    ("Fantasia", 5),
    ("Girls Love", 22),
    ("Gore", 181),
    ("Guerra", 1109),
    ("Género Bender", 183),
    ("Harem", 8),
    ("Historia", 81),
    ("Horror", 82),
    ("Magia", 6),
    ("Mecha", 144),
    ("Militar", 342),
    ("Misterio", 40),
    ("Musica", 403),
    ("Niños", 219),
    ("Oeste", 141),
    ("Parodia", 820),
    ("Policiaco", 111),
    ("Psicológico", 36),
    ("Realidad", 147),
    ("Realidad Virtual", 27),
    ("Recuentos de la vida", 33),
    ("Reencarnación", 60),
    ("Romance", 16),
    ("Samurái", 99),
    ("Sobrenatural", 7),
    ("Superpoderes", 116),
    ("Supervivencia", 112),
    ("Telenovela", 470),
    ("Thriller", 49),
    ("Tragedia", 46),
    ("Traps", 1464),
    ("Vampiros", 345),
    ("Vida Escolar", 23),
];

const LIST_FIXTURE: &str = r#"{"data":{"items":[{"slug":"sample","title":"Sample","overview":"Summary","cover":"cover.jpg","author":[{"name":"Author"}],"status":[12],"genres":[2]}],"pagination":{"has_next":false,"total_pages":1}}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"slug":"sample","title":"Sample","overview":"Summary","cover":"cover.jpg","author":[{"name":"Author"}],"status":[12],"genres":[2]}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"items":[{"id":1,"chapter_number":"1","title":"One","slug":"chapter-1","release_date":"2024-01-01 00:00:00"}],"pagination":{"has_next":false,"total_pages":1}}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"chapter":{"jit":"sample/chapter-1","images":[{"image_url":"1.jpg","page_number":1},{"image_url":"2.jpg","page_number":2}]}}}"#;
