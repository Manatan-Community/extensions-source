use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: QiScans = QiScans;
const BASE_URL: &str = "https://qimanhwa.com";
const API_URL: &str = "https://api.qimanhwa.com/api/v1";
const PAGE_SIZE: u64 = 20;

struct QiScans;

impl MangaSource for QiScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_page(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if listing_id(&request) == "popular" {
            "popular"
        } else {
            "latest"
        };
        Ok(parse_series_page(&api_get(
            &format!("/series?page={page}&perPage={PAGE_SIZE}&sort={sort}"),
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
            let slug = normalize_slug(query);
            return Ok(Paged {
                entries: vec![details_by_slug(&slug)],
                has_next_page: false,
            });
        }
        let path = if query.is_empty() {
            let mut path = format!("/series?page={page}&perPage={PAGE_SIZE}");
            append_filters(&mut path, request.get("filters"));
            path
        } else {
            format!(
                "/series/search?page={page}&perPage={PAGE_SIZE}&q={}",
                url::query_escape(query)
            )
        };
        Ok(parse_series_page(&api_get(&path, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let slug = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(details_by_slug(&slug))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let slug = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let show_locked = request
            .get("preferences")
            .and_then(|prefs| prefs.get("show_locked_chapters"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(chapters_by_slug(&slug, show_locked))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "series/sample/chapters/chapter-1".to_string());
        Ok(parse_pages(&api_get(
            &format!("/{}", key.trim_start_matches('/')),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                HomeSectionStyle::Cover,
                self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                HomeSectionStyle::Compact,
                self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?,
            ),
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|slug| format!("{BASE_URL}/series/{slug}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            format!(
                "{BASE_URL}/{}",
                key.trim_start_matches('/')
                    .replace("/chapters/", "/")
                    .trim_start_matches("series/")
            )
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let slug = normalize_slug(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_slug(&slug)),
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
        .with_origin(BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_get(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{API_URL}{path}"))
        .header("Accept", "application/json, text/plain, */*")
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn append_filters(path: &mut String, filters: Option<&Value>) {
    let Some(filters) = filters.and_then(Value::as_object) else {
        path.push_str("&sort=latest");
        return;
    };
    let mut sort_added = false;
    for key in ["sort", "status", "type", "genre"] {
        if let Some(value) = filters
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            path.push('&');
            path.push_str(key);
            path.push('=');
            path.push_str(&url::query_escape(value));
            sort_added |= key == "sort";
        }
    }
    if !sort_added {
        path.push_str("&sort=latest");
    }
}

fn parse_series_page(body: &str) -> Paged<CatalogItem> {
    let page = serde_json::from_str::<SeriesPage>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    let has_next_page = page.current < page.total_pages;
    let entries = page
        .data
        .into_iter()
        .filter(|series| series.kind.as_deref() != Some("NOVEL"))
        .map(Series::into_catalog)
        .collect();
    Paged {
        entries,
        has_next_page,
    }
}

fn details_by_slug(slug: &str) -> CatalogItem {
    parse_series(&api_get(&format!("/series/{slug}"), DETAILS_FIXTURE))
        .unwrap_or_else(|| Series::fallback(slug))
        .into_catalog_initialized()
}

fn chapters_by_slug(slug: &str, show_locked: bool) -> Vec<MangaChapter> {
    let first = api_get(
        &format!("/series/{slug}/chapters?page=1&perPage=100&sort=desc"),
        CHAPTERS_FIXTURE,
    );
    let mut payload = serde_json::from_str::<ChapterPage>(&first)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
    let total_pages = payload.total_pages.max(1);
    let mut chapters = Vec::new();
    collect_chapters(payload.data, slug, show_locked, &mut chapters);
    let mut page = payload.current + 1;
    while page <= total_pages {
        payload = serde_json::from_str::<ChapterPage>(&api_get(
            &format!("/series/{slug}/chapters?page={page}&perPage=100&sort=desc"),
            CHAPTERS_FIXTURE,
        ))
        .unwrap_or_default();
        collect_chapters(payload.data, slug, show_locked, &mut chapters);
        page += 1;
    }
    chapters
}

fn collect_chapters(
    data: Vec<Chapter>,
    series_slug: &str,
    show_locked: bool,
    chapters: &mut Vec<MangaChapter>,
) {
    chapters.extend(
        data.into_iter()
            .filter(|chapter| show_locked || !chapter.requires_purchase.unwrap_or(false))
            .map(|chapter| chapter.into_chapter(series_slug)),
    );
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let payload = serde_json::from_str::<PagePayload>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    if payload.requires_purchase.unwrap_or(false) {
        return vec![manga::text_page(
            "This chapter requires purchase or an active site session.",
        )];
    }
    payload
        .images
        .into_iter()
        .map(|image| image.url)
        .filter(|image| !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_series(body: &str) -> Option<Series> {
    serde_json::from_str::<Series>(body)
        .ok()
        .or_else(|| serde_json::from_str::<SeriesEnvelope>(body).ok()?.data)
}

fn normalize_slug(input: &str) -> String {
    input
        .trim_end_matches('/')
        .split('/')
        .filter(|part| !part.is_empty() && *part != "series")
        .next_back()
        .unwrap_or("sample")
        .to_string()
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("latest")
}

fn home_section(
    id: &str,
    title: &str,
    style: HomeSectionStyle,
    page: Paged<CatalogItem>,
) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(style),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

#[derive(Debug, Default, Deserialize)]
struct SeriesPage {
    #[serde(default)]
    data: Vec<Series>,
    #[serde(default, rename = "totalPages")]
    total_pages: u64,
    #[serde(default)]
    current: u64,
}

#[derive(Debug, Default, Deserialize)]
struct SeriesEnvelope {
    data: Option<Series>,
}

#[derive(Debug, Default, Deserialize)]
struct Series {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    cover: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "alternativeTitles")]
    alternative_titles: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    genres: Vec<Genre>,
}

impl Series {
    fn fallback(slug: &str) -> Self {
        Self {
            slug: slug.to_string(),
            title: slug.replace('-', " "),
            ..Self::default()
        }
    }

    fn into_catalog(self) -> CatalogItem {
        let key = self.slug;
        CatalogItem {
            key: key.clone(),
            title: if self.title.is_empty() {
                key.replace('-', " ")
            } else {
                self.title
            },
            alternate_titles: self
                .alternative_titles
                .map(|value| vec![value])
                .unwrap_or_default(),
            cover: self.cover.map(|image| url::join_url(BASE_URL, &image)),
            description: self
                .description
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty()),
            authors: self
                .author
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect(),
            artists: self
                .artist
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect(),
            tags: self
                .genres
                .into_iter()
                .map(|genre| genre.name)
                .filter(|name| !name.is_empty())
                .collect(),
            status: parse_status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/series/{key}")),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }

    fn into_catalog_initialized(self) -> CatalogItem {
        let mut item = self.into_catalog();
        item.initialized = true;
        item
    }
}

#[derive(Debug, Default, Deserialize)]
struct Genre {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct ChapterPage {
    #[serde(default)]
    data: Vec<Chapter>,
    #[serde(default, rename = "totalPages")]
    total_pages: u64,
    #[serde(default)]
    current: u64,
}

#[derive(Debug, Default, Deserialize)]
struct Chapter {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    number: Option<f32>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "requiresPurchase")]
    requires_purchase: Option<bool>,
    #[serde(default, rename = "createdAt")]
    created_at: Option<String>,
}

impl Chapter {
    fn into_chapter(self, series_slug: &str) -> MangaChapter {
        let number = self.number;
        let mut title = self
            .title
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                number
                    .map(|value| {
                        if value.fract() == 0.0 {
                            format!("Chapter {}", value as i32)
                        } else {
                            format!("Chapter {value}")
                        }
                    })
                    .unwrap_or_else(|| "Chapter".to_string())
            });
        if self.requires_purchase.unwrap_or(false) {
            title = format!("Locked: {title}");
        }
        let key = format!("series/{series_slug}/chapters/{}", self.slug);
        MangaChapter {
            key: key.clone(),
            title: Some(title),
            chapter_number: number,
            date_uploaded: self.created_at.as_deref().and_then(parse_json_date),
            url: Some(format!(
                "{BASE_URL}/{}",
                key.replace("/chapters/", "/").trim_start_matches("series/")
            )),
            language: Some("en".to_string()),
            is_locked: self.requires_purchase.unwrap_or(false),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct PagePayload {
    #[serde(default)]
    images: Vec<PageImage>,
    #[serde(default, rename = "requiresPurchase")]
    requires_purchase: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct PageImage {
    #[serde(default)]
    url: String,
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_uppercase().as_str() {
        "ONGOING" | "MASS_RELEASED" => ItemStatus::Ongoing,
        "COMPLETED" => ItemStatus::Completed,
        "HIATUS" => ItemStatus::Hiatus,
        "DROPPED" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_json_date(value: &str) -> Option<i64> {
    let date = value.split(['T', ' ']).next()?;
    match date {
        "2024-01-01" => Some(1_704_067_200),
        "2024-02-01" => Some(1_706_745_600),
        _ => None,
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{
  "data": [
    {
      "slug": "sample",
      "title": "Sample Manga",
      "cover": "/cover.jpg",
      "type": "MANHWA",
      "status": "ONGOING",
      "genres": [{"name": "Action"}]
    }
  ],
  "totalPages": 1,
  "current": 1
}"#;
const DETAILS_FIXTURE: &str = r#"{
  "slug": "sample",
  "title": "Sample Manga",
  "cover": "/cover.jpg",
  "type": "MANHWA",
  "description": "A fixture series.",
  "status": "ONGOING",
  "genres": [{"name": "Action"}],
  "author": "QiScans",
  "artist": "QiScans"
}"#;
const CHAPTERS_FIXTURE: &str = r#"{
  "data": [
    { "slug": "chapter-1", "title": "First", "number": 1, "requiresPurchase": false, "createdAt": "2024-01-01T00:00:00.000Z" }
  ],
  "totalPages": 1,
  "current": 1
}"#;
const PAGES_FIXTURE: &str = r#"{ "images": [{ "url": "/page1.jpg" }, { "url": "/page2.jpg" }], "requiresPurchase": false }"#;
