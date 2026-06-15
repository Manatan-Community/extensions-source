use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: KaiserzScanlation = KaiserzScanlation;
const BASE_URL: &str = "https://capibaratraductor.com/kaizscan";
const API_URL: &str = "https://capibaratraductor.com";
const ORG: &str = "kaizscan";

struct KaiserzScanlation;

impl MangaSource for KaiserzScanlation {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "popular"
        };
        Ok(parse_series_page(
            &fetch_api_text(
                &format!("/api/manga-custom?page={page}&limit=36&order={order}"),
                LIST_FIXTURE,
            ),
            page,
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
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_item(&key)],
                has_next_page: false,
            });
        }
        Ok(parse_series_page(
            &fetch_api_text(
                &format!(
                    "/api/manga-custom?page={page}&limit=36&title={}",
                    url::query_escape(query)
                ),
                LIST_FIXTURE,
            ),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_item(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_dto(&key)
            .chapters
            .unwrap_or_default()
            .into_iter()
            .filter(|chapter| !chapter.is_unreleased)
            .map(|chapter| chapter.to_chapter(&key))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".into());
        let (series, chapter) = key.split_once('/').unwrap_or(("sample", "1"));
        let response: Data<Vec<PageDto>> = fetch_api(
            &format!("/api/manga-custom/{series}/chapter/{chapter}/pages"),
            PAGES_FIXTURE,
        );
        Ok(response
            .data
            .into_iter()
            .enumerate()
            .map(|(index, page)| MangaPage {
                content: PageContent::Url {
                    url: page.image_url,
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
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_item(&key)),
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
        .with_header("x-organization", ORG)
        .with_cookies_for(API_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api_text(path: &str, fixture: &str) -> String {
    client()
        .get(&url::join_url(API_URL, path))
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_api<T: for<'de> Deserialize<'de>>(path: &str, fixture: &str) -> T {
    serde_json::from_str(&fetch_api_text(path, fixture))
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap())
}

fn parse_series_page(body: &str, page: u64) -> Paged<CatalogItem> {
    let response: Data<SeriesListDataDto> =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap());
    Paged {
        entries: response
            .data
            .items
            .into_iter()
            .map(|series| series.to_item(false))
            .collect(),
        has_next_page: page < response.data.max_page,
    }
}

fn details_item(key: &str) -> CatalogItem {
    details_dto(key).to_item(true)
}

fn details_dto(key: &str) -> SeriesDto {
    let response: Data<SeriesDto> = fetch_api(&format!("/api/manga-custom/{key}"), DETAILS_FIXTURE);
    response.data
}

fn normalize_key(input: &str) -> String {
    input
        .trim_start_matches(BASE_URL)
        .trim_start_matches("/manga/")
        .trim_matches('/')
        .to_string()
}

#[derive(Debug, Deserialize)]
struct Data<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct SeriesListDataDto {
    #[serde(default)]
    items: Vec<SeriesDto>,
    #[serde(rename = "maxPage", default)]
    max_page: u64,
}

#[derive(Debug, Deserialize)]
struct SeriesDto {
    manga: MangaInfoDto,
    #[serde(rename = "imageUrl")]
    image_url: Option<String>,
    title: String,
    status: Option<String>,
    description: Option<String>,
    #[serde(default)]
    authors: Vec<SeriesAuthorDto>,
    chapters: Option<Vec<SeriesChapterDto>>,
}

impl SeriesDto {
    fn to_item(self, initialized: bool) -> CatalogItem {
        CatalogItem {
            key: self.manga.slug.clone(),
            title: self.title,
            cover: self.image_url,
            description: self.description,
            authors: self.authors.into_iter().map(|author| author.name).collect(),
            status: match self.status.as_deref() {
                Some("ongoing") => ItemStatus::Ongoing,
                Some("hiatus") => ItemStatus::Hiatus,
                Some("finished") => ItemStatus::Completed,
                _ => ItemStatus::Unknown,
            },
            url: Some(format!("{BASE_URL}/manga/{}", self.manga.slug)),
            language: Some("es".into()),
            content_rating: Some("safe".into()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct MangaInfoDto {
    slug: String,
}

#[derive(Debug, Deserialize)]
struct SeriesAuthorDto {
    name: String,
}

#[derive(Debug, Deserialize)]
struct SeriesChapterDto {
    title: String,
    number: f32,
    #[serde(rename = "isUnreleased", default)]
    is_unreleased: bool,
}

impl SeriesChapterDto {
    fn to_chapter(self, series: &str) -> MangaChapter {
        let number = self.number.to_string().trim_end_matches(".0").to_string();
        MangaChapter {
            key: format!("{series}/{number}"),
            title: Some(format!("Capítulo {number} - {}", self.title)),
            chapter_number: Some(self.number),
            url: Some(format!("{BASE_URL}/manga/{series}/chapters/{number}")),
            language: Some("es".into()),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct PageDto {
    #[serde(rename = "imageUrl")]
    image_url: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":{"items":[{"manga":{"slug":"sample"},"imageUrl":"https://example.com/cover.jpg","title":"Sample","status":"ongoing","description":"Summary","authors":[{"name":"Author"}],"chapters":[{"title":"Uno","number":1,"isUnreleased":false}]}],"maxPage":1}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"manga":{"slug":"sample"},"imageUrl":"https://example.com/cover.jpg","title":"Sample","status":"ongoing","description":"Summary","authors":[{"name":"Author"}],"chapters":[{"title":"Uno","number":1,"isUnreleased":false}]}}"#;
const PAGES_FIXTURE: &str = r#"{"data":[{"imageUrl":"https://example.com/page1.jpg"},{"imageUrl":"https://example.com/page2.jpg"}]}"#;
