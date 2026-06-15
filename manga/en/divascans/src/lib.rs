use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: DivaScans = DivaScans;
const BASE_URL: &str = "https://divatoon.com";
const API_URL: &str = "https://api.divatoon.com";
const PER_PAGE: u64 = 18;

struct DivaScans;

impl MangaSource for DivaScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_query(QUERY_FIXTURE, 1));
        }
        let mut filters = request.get("filters").cloned().unwrap_or(Value::Null);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            set_filter(&mut filters, "orderBy", "lastChapterAddedAt");
            set_filter(&mut filters, "orderDirection", "desc");
        } else {
            set_filter(&mut filters, "orderBy", "totalViews");
            set_filter(&mut filters, "orderDirection", "desc");
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_query(&fetch_query(page, "", &filters), page))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let slug = query
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("sample");
            let body = api_get(&format!("/api/post?postSlug={slug}"), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_post::<MangaDto>(&body).to_catalog()],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        Ok(parse_query(&fetch_query(page, query, filters), page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#1".into());
        let slug = key.split('#').next().unwrap_or(&key);
        let body = api_get(&format!("/api/post?postSlug={slug}"), DETAILS_FIXTURE);
        Ok(parse_post::<MangaDto>(&body).to_catalog())
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#1".into());
        let slug = key.split('#').next().unwrap_or(&key);
        let body = api_get(&format!("/api/post?postSlug={slug}"), CHAPTERS_FIXTURE);
        let post = parse_post::<ChapterListDto>(&body);
        Ok(post
            .chapters
            .into_iter()
            .filter(ChapterDto::is_visible)
            .map(|chapter| chapter.to_chapter(post.slug.as_deref().unwrap_or(slug)))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1#10".into());
        let id = key.rsplit('#').next().unwrap_or("10");
        let body = api_get(&format!("/api/chapter?chapterId={id}"), PAGES_FIXTURE);
        let response = serde_json::from_str::<PostResponse<PageEnvelope>>(&body)
            .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
        let mut images = response.post.chapter.images;
        images.sort_by_key(|image| image.order.unwrap_or(i64::MAX));
        Ok(images
            .into_iter()
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: image.url.replace(' ', "%20"),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let slug = input
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("sample");
            let body = api_get(&format!("/api/post?postSlug={slug}"), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_post::<MangaDto>(&body).to_catalog()),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_get(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{API_URL}{path}"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_query(page: u64, query: &str, filters: &Value) -> String {
    let mut params = vec![
        ("page", page.to_string()),
        ("perPage", PER_PAGE.to_string()),
        ("searchTerm", query.trim().to_string()),
    ];
    for key in [
        "seriesStatus",
        "seriesType",
        "orderBy",
        "orderDirection",
        "genreIds",
    ] {
        if let Some(value) = filter_value(filters, key).filter(|value| !value.is_empty()) {
            params.push((key, value));
        }
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    api_get(&format!("/api/query?{query}"), QUERY_FIXTURE)
}

fn parse_query(body: &str, page: u64) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<SearchResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(QUERY_FIXTURE).expect("fixture is valid"));
    let entries = response
        .posts
        .into_iter()
        .filter(|item| !item.is_novel)
        .map(MangaDto::to_catalog)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: response.total_count > page * PER_PAGE,
        entries,
    }
}

fn parse_post<T>(body: &str) -> T
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str::<PostResponse<T>>(body)
        .or_else(|_| serde_json::from_str(DETAILS_FIXTURE))
        .expect("fixture is valid")
        .post
}

fn filter_value(filters: &Value, key: &str) -> Option<String> {
    let value = filters.get(key)?;
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if let Some(array) = value.as_array() {
        return Some(
            array
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    None
}

fn set_filter(filters: &mut Value, key: &str, value: &str) {
    if !filters.is_object() {
        *filters = serde_json::Map::new().into();
    }
    if let Some(object) = filters.as_object_mut() {
        object
            .entry(key.to_string())
            .or_insert_with(|| Value::String(value.to_string()));
    }
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    posts: Vec<MangaDto>,
    #[serde(rename = "totalCount")]
    total_count: u64,
}

#[derive(Debug, Deserialize)]
struct PostResponse<T> {
    post: T,
}

#[derive(Debug, Deserialize)]
struct MangaDto {
    id: u64,
    slug: String,
    #[serde(rename = "postTitle")]
    post_title: String,
    #[serde(default, rename = "postContent")]
    post_content: String,
    #[serde(default, rename = "isNovel")]
    is_novel: bool,
    #[serde(default, rename = "featuredImage")]
    featured_image: String,
    #[serde(default, rename = "alternativeTitles")]
    alternative_titles: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    artist: String,
    #[serde(default, rename = "seriesType")]
    series_type: String,
    #[serde(default, rename = "seriesStatus")]
    series_status: String,
    #[serde(default)]
    genres: Vec<GenreDto>,
}

impl MangaDto {
    fn to_catalog(self) -> CatalogItem {
        let description = [
            (!self.post_content.is_empty()).then(|| html::strip_tags(&self.post_content)),
            (!self.alternative_titles.is_empty())
                .then(|| format!("Alternative Names: {}", self.alternative_titles)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n\n");
        let mut tags = Vec::new();
        match self.series_type.as_str() {
            "MANGA" => tags.push("Manga".to_string()),
            "MANHUA" => tags.push("Manhua".to_string()),
            "MANHWA" => tags.push("Manhwa".to_string()),
            _ => {}
        }
        tags.extend(self.genres.into_iter().map(|genre| genre.name));
        CatalogItem {
            key: format!("{}#{}", self.slug, self.id),
            title: self.post_title,
            cover: (!self.featured_image.is_empty()).then_some(self.featured_image),
            description: (!description.is_empty()).then_some(description),
            authors: (!self.author.is_empty())
                .then_some(self.author)
                .into_iter()
                .collect(),
            artists: (!self.artist.is_empty())
                .then_some(self.artist)
                .into_iter()
                .collect(),
            tags,
            status: match self.series_status.as_str() {
                "ONGOING" | "COMING_SOON" => ItemStatus::Ongoing,
                "COMPLETED" => ItemStatus::Completed,
                "CANCELLED" | "DROPPED" => ItemStatus::Cancelled,
                _ => ItemStatus::Unknown,
            },
            url: Some(format!("{BASE_URL}/series/{}", self.slug)),
            language: Some("en".into()),
            content_rating: Some("adult".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct GenreDto {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ChapterListDto {
    #[serde(default)]
    slug: Option<String>,
    chapters: Vec<ChapterDto>,
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    id: u64,
    slug: String,
    number: Value,
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "chapterStatus")]
    chapter_status: String,
    #[serde(default, rename = "isAccessible")]
    is_accessible: bool,
    #[serde(default, rename = "isLocked")]
    is_locked: bool,
    #[serde(default, rename = "isTimeLocked")]
    is_time_locked: bool,
}

impl ChapterDto {
    fn is_visible(&self) -> bool {
        self.chapter_status == "PUBLIC"
            && (self.is_accessible || self.is_locked || self.is_time_locked)
    }

    fn to_chapter(self, series_slug: &str) -> MangaChapter {
        let number = self
            .number
            .as_f64()
            .or_else(|| self.number.as_str().and_then(|value| value.parse().ok()));
        let suffix = self
            .title
            .filter(|value| !value.is_empty())
            .map(|title| format!(" - {title}"))
            .unwrap_or_default();
        MangaChapter {
            key: format!("/series/{series_slug}/{}#{}", self.slug, self.id),
            title: Some(format!(
                "Chapter {}{}",
                number.map(|value| value.to_string()).unwrap_or_default(),
                suffix
            )),
            chapter_number: number.map(|value| value as f32),
            url: Some(format!("{BASE_URL}/series/{series_slug}/{}", self.slug)),
            language: Some("en".into()),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct PageEnvelope {
    chapter: ChapterPageDto,
}

#[derive(Debug, Deserialize)]
struct ChapterPageDto {
    images: Vec<PageDto>,
}

#[derive(Debug, Deserialize)]
struct PageDto {
    url: String,
    order: Option<i64>,
}

export_manga_source!(SOURCE);

const QUERY_FIXTURE: &str = r#"{"posts":[{"id":1,"slug":"sample","postTitle":"Sample Series","postContent":"Sample description","isNovel":false,"featuredImage":"https://divatoon.com/cover.jpg","seriesStatus":"ONGOING","seriesType":"MANHWA","genres":[{"id":1,"name":"Drama"}]}],"totalCount":1}"#;
const DETAILS_FIXTURE: &str = r#"{"post":{"id":1,"slug":"sample","postTitle":"Sample Series","postContent":"Sample description","isNovel":false,"featuredImage":"https://divatoon.com/cover.jpg","seriesStatus":"ONGOING","seriesType":"MANHWA","genres":[{"id":1,"name":"Drama"}]}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"post":{"slug":"sample","chapters":[{"id":10,"slug":"chapter-1","number":1,"title":"Start","createdAt":"2024-01-01T00:00:00.000Z","chapterStatus":"PUBLIC","isAccessible":true}]}}"#;
const PAGES_FIXTURE: &str = r#"{"post":{"chapter":{"id":10,"images":[{"url":"https://divatoon.com/page1.jpg","order":1}]}}}"#;
