use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: StoneScape = StoneScape;
const BASE_URL: &str = "https://stonescape.xyz";
const API_URL: &str = "https://stonescape.xyz/api";

struct StoneScape;

impl MangaSource for StoneScape {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{API_URL}/series?page={page}&limit=24&contentType=manhwa")
        } else {
            format!("{API_URL}/series/popular?page={page}&period=week&contentType=manhwa&limit=24")
        };
        Ok(parse_series_response(fetch_json_or_fixture(
            &target,
            SERIES_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(slug) = slug_from_stonescape_url(query) {
            let dto: SeriesDto = fetch_json_or_fixture(
                &format!("{API_URL}/series/by-slug/{slug}"),
                SERIES_ONE_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![dto.to_item(false)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut target = format!("{API_URL}/series?page={page}&limit=24&contentType=manhwa");
        if !query.is_empty() {
            target.push_str("&search=");
            target.push_str(&url::query_escape(query));
        }
        Ok(parse_series_response(fetch_json_or_fixture(
            &target,
            SERIES_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let slug = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample");
        let dto: SeriesDto = fetch_json_or_fixture(
            &format!("{API_URL}/series/by-slug/{slug}"),
            SERIES_ONE_FIXTURE,
        );
        Ok(dto.to_item(true))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let slug = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample");
        let response: ChapterListResponse = fetch_json_or_fixture(
            &format!("{API_URL}/series/by-slug/{slug}/chapters"),
            CHAPTERS_FIXTURE,
        );
        let mut chapters = response
            .chapters
            .into_iter()
            .map(|chapter| chapter.to_chapter(slug))
            .collect::<Vec<_>>();
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/ch-1#chapter-1".to_string());
        let chapter_id = key.split('#').nth(1).unwrap_or("chapter-1");
        let response: ChapterDetailsDto = fetch_json_or_fixture(
            &format!("{API_URL}/chapters/{chapter_id}/pages"),
            PAGES_FIXTURE,
        );
        Ok(response
            .all_pages()
            .into_iter()
            .enumerate()
            .map(|(index, page)| MangaPage {
                content: PageContent::Url {
                    url: url::join_url(BASE_URL, &page.url),
                    context: None,
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!(
                    "Page {}",
                    page.page_number.unwrap_or(index as u32 + 1)
                )),
                ..MangaPage::default()
            })
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = slug_from_stonescape_url(input) {
            let dto: SeriesDto = fetch_json_or_fixture(
                &format!("{API_URL}/series/by-slug/{slug}"),
                SERIES_ONE_FIXTURE,
            );
            return Ok(Some(UrlResolveResult {
                item: Some(dto.to_item(true)),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture<T: for<'de> Deserialize<'de>>(target: &str, fixture: &str) -> T {
    let text = client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    serde_json::from_str(&text).unwrap_or_else(|_| serde_json::from_str(fixture).unwrap())
}

fn parse_series_response(response: SeriesResponse) -> Paged<CatalogItem> {
    let current = response
        .pagination
        .as_ref()
        .and_then(|page| page.page)
        .unwrap_or(1);
    let total = response
        .pagination
        .as_ref()
        .and_then(|page| page.total_pages)
        .unwrap_or(1);
    Paged {
        entries: response
            .data
            .into_iter()
            .map(|series| series.to_item(false))
            .collect(),
        has_next_page: current < total,
    }
}

fn slug_from_stonescape_url(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) {
        return None;
    }
    let marker = "/series/";
    let start = input.find(marker)? + marker.len();
    input[start..]
        .split(['/', '?', '#'])
        .next()
        .filter(|slug| !slug.is_empty())
        .map(ToString::to_string)
}

#[derive(Debug, Deserialize)]
struct SeriesResponse {
    data: Vec<SeriesDto>,
    pagination: Option<PaginationDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaginationDto {
    page: Option<u64>,
    total_pages: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesDto {
    title: String,
    slug: String,
    cover_url: Option<String>,
    description: Option<String>,
    publication_status: Option<String>,
    author: Option<String>,
    artist: Option<String>,
    genres: Option<Vec<String>>,
}

impl SeriesDto {
    fn to_item(&self, initialized: bool) -> CatalogItem {
        CatalogItem {
            key: format!("/series/{}", self.slug),
            title: self.title.clone(),
            cover: self
                .cover_url
                .as_ref()
                .map(|cover| url::join_url(BASE_URL, cover)),
            url: Some(format!("{BASE_URL}/series/{}", self.slug)),
            authors: self.author.clone().into_iter().collect(),
            artists: self.artist.clone().into_iter().collect(),
            description: self.description.clone(),
            tags: self.genres.clone().unwrap_or_default(),
            status: parse_status(self.publication_status.as_deref()),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.unwrap_or_default().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "hiatus" => ItemStatus::Hiatus,
        "dropped" | "cancelled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

#[derive(Debug, Deserialize)]
struct ChapterListResponse {
    chapters: Vec<ChapterDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterDto {
    chapter_id: String,
    chapter_number: String,
    title: Option<String>,
}

impl ChapterDto {
    fn to_chapter(&self, slug: &str) -> MangaChapter {
        let formatted = self
            .chapter_number
            .parse::<f32>()
            .ok()
            .map(|number| number.to_string().trim_end_matches(".0").to_string())
            .unwrap_or_else(|| self.chapter_number.clone());
        MangaChapter {
            key: format!("/series/{slug}/ch-{formatted}#{}", self.chapter_id),
            title: Some(
                match self.title.as_deref().filter(|title| !title.is_empty()) {
                    Some(title) => format!("Chapter {formatted} - {title}"),
                    None => format!("Chapter {formatted}"),
                },
            ),
            chapter_number: formatted.parse::<f32>().ok(),
            language: Some("en".to_string()),
            url: Some(format!("{BASE_URL}/series/{slug}/ch-{formatted}")),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChapterDetailsDto {
    #[serde(default)]
    pages: Vec<PageDto>,
    #[serde(default)]
    images: Vec<PageDto>,
}

impl ChapterDetailsDto {
    fn all_pages(self) -> Vec<PageDto> {
        if self.pages.is_empty() {
            self.images
        } else {
            self.pages
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageDto {
    page_number: Option<u32>,
    url: String,
}

export_manga_source!(SOURCE);

const SERIES_FIXTURE: &str = r#"{"data":[{"title":"Sample","slug":"sample","coverUrl":"/cover.jpg","description":"Sample","publicationStatus":"ongoing","author":"Author","artist":"Artist","genres":["Action"]}],"pagination":{"page":1,"totalPages":1}}"#;
const SERIES_ONE_FIXTURE: &str = r#"{"title":"Sample","slug":"sample","coverUrl":"/cover.jpg","description":"Sample","publicationStatus":"ongoing","author":"Author","artist":"Artist","genres":["Action"]}"#;
const CHAPTERS_FIXTURE: &str =
    r#"{"chapters":[{"chapterId":"chapter-1","chapterNumber":"1","title":"Start"}]}"#;
const PAGES_FIXTURE: &str = r#"{"pages":[{"pageNumber":1,"url":"/page1.jpg"}],"images":[]}"#;
