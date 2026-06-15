use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ComicLand = ComicLand;
const BASE_URL: &str = "https://comicland.org";
const API_URL: &str = "https://api.comicland.org/api";
const LIMIT: u64 = 20;

struct ComicLand;

impl MangaSource for ComicLand {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_page(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let offset = (page.saturating_sub(1)) * LIMIT;
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("/comics?offset={offset}&limit={LIMIT}&status=ongoing")
        } else {
            format!("/comics/popular?offset={offset}&limit={LIMIT}")
        };
        Ok(parse_page(&fetch_api_or_fixture(&path, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let offset = (page.saturating_sub(1)) * LIMIT;
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let slug = query
                .split("/comic/")
                .nth(1)
                .and_then(|part| part.split('/').next())
                .unwrap_or("sample");
            return Ok(Paged {
                entries: vec![parse_details(&fetch_api_or_fixture(
                    &format!("/comic/detail?slug={slug}"),
                    DETAILS_FIXTURE,
                ))],
                has_next_page: false,
            });
        }
        let path = if !query.is_empty() {
            format!(
                "/comic/search?q={}&offset={offset}&limit={LIMIT}",
                url::query_escape(query)
            )
        } else {
            let (endpoint, status) = category_endpoint(request.get("filters"));
            let status_query = status
                .map(|status| format!("&status={status}"))
                .unwrap_or_default();
            format!("{endpoint}?offset={offset}&limit={LIMIT}{status_query}")
        };
        Ok(parse_page(&fetch_api_or_fixture(&path, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_details(&fetch_api_or_fixture(
            &format!("/comic/detail?slug={}", key.trim_matches('/')),
            DETAILS_FIXTURE,
        )))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let details = parse_detail_response(&fetch_api_or_fixture(
            &format!("/comic/detail?slug={}", key.trim_matches('/')),
            DETAILS_FIXTURE,
        ));
        Ok(details
            .data
            .into_iter()
            .flat_map(|detail| {
                let slug = detail.slug.clone();
                detail
                    .chapters
                    .unwrap_or_default()
                    .into_iter()
                    .rev()
                    .map(move |chapter| chapter.into_chapter(&slug))
            })
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".to_string());
        let (slug, index) = key.split_once('/').unwrap_or(("sample", "1"));
        Ok(parse_pages(&fetch_api_or_fixture(
            &format!("/chapter/pages_by_index?slug={slug}&index={index}"),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let slug = input
                .split("/comic/")
                .nth(1)
                .and_then(|part| part.split('/').next())
                .unwrap_or("sample");
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_api_or_fixture(
                    &format!("/comic/detail?slug={slug}"),
                    DETAILS_FIXTURE,
                ))),
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

fn fetch_api_or_fixture(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{API_URL}{path}"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn category_endpoint(filters: Option<&Value>) -> (&'static str, Option<&'static str>) {
    match filters
        .and_then(|filters| filters.get("category"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
    {
        "official" => ("/comics/official", None),
        "ongoing" => ("/comics", Some("ongoing")),
        "popular" => ("/comics/popular", None),
        _ => ("/comics", None),
    }
}

fn parse_page(body: &str) -> Paged<CatalogItem> {
    let response: ApiResponse<PageData> = serde_json::from_str(body).unwrap_or_default();
    let data = response.data.unwrap_or_default();
    Paged {
        entries: data
            .comics()
            .into_iter()
            .map(ComicDto::into_catalog)
            .collect(),
        has_next_page: data
            .has_more
            .unwrap_or_else(|| data.comics().len() >= LIMIT as usize),
    }
}

fn parse_details(body: &str) -> CatalogItem {
    parse_detail_response(body)
        .data
        .map(ComicDetailDto::into_catalog)
        .unwrap_or_else(|| CatalogItem {
            key: "sample".to_string(),
            title: "ComicLand".to_string(),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: true,
            ..CatalogItem::default()
        })
}

fn parse_detail_response(body: &str) -> ApiResponse<ComicDetailDto> {
    serde_json::from_str(body).unwrap_or_default()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response: ApiResponse<PagesData> = serde_json::from_str(body).unwrap_or_default();
    response
        .data
        .unwrap_or_default()
        .pages
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

#[derive(Default, Deserialize)]
struct ApiResponse<T> {
    data: Option<T>,
}

#[derive(Default, Deserialize)]
struct PageData {
    #[serde(default)]
    list: Vec<ComicDto>,
    #[serde(default)]
    items: Vec<ComicDto>,
    #[serde(default)]
    has_more: Option<bool>,
}

impl PageData {
    fn comics(&self) -> Vec<ComicDto> {
        if self.list.is_empty() {
            self.items.clone()
        } else {
            self.list.clone()
        }
    }
}

#[derive(Clone, Default, Deserialize)]
struct ComicDto {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    cover_url: String,
}

impl ComicDto {
    fn into_catalog(self) -> CatalogItem {
        CatalogItem {
            key: self.slug.clone(),
            title: if self.title.is_empty() {
                self.slug.clone()
            } else {
                self.title
            },
            cover: (!self.cover_url.is_empty()).then_some(self.cover_url),
            url: Some(format!("{BASE_URL}/comic/{}", self.slug)),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct ComicDetailDto {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    cover_url: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    authors: Vec<NameDto>,
    #[serde(default)]
    artists: Vec<NameDto>,
    #[serde(default)]
    genres: Vec<NameDto>,
    #[serde(default)]
    chapters: Option<Vec<ChapterDto>>,
}

impl ComicDetailDto {
    fn into_catalog(self) -> CatalogItem {
        CatalogItem {
            key: self.slug.clone(),
            title: if self.title.is_empty() {
                self.slug.clone()
            } else {
                self.title
            },
            cover: (!self.cover_url.is_empty()).then_some(self.cover_url),
            description: self.description,
            authors: self.authors.into_iter().map(|item| item.name).collect(),
            artists: self.artists.into_iter().map(|item| item.name).collect(),
            tags: self.genres.into_iter().map(|item| item.name).collect(),
            status: ItemStatus::Unknown,
            url: Some(format!("{BASE_URL}/comic/{}", self.slug)),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct NameDto {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    #[serde(default)]
    chapter_index: f32,
    #[serde(default)]
    title: String,
}

impl ChapterDto {
    fn into_chapter(self, slug: &str) -> MangaChapter {
        let index = number_key(self.chapter_index);
        MangaChapter {
            key: format!("{slug}/{index}"),
            title: Some(if self.title.is_empty() {
                format!("Chapter {index}")
            } else {
                self.title
            }),
            chapter_number: Some(self.chapter_index),
            url: Some(format!("{BASE_URL}/comic/{slug}/chapter/{index}")),
            ..MangaChapter::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct PagesData {
    #[serde(default)]
    pages: Vec<String>,
}

fn number_key(value: f32) -> String {
    let mut text = value.to_string();
    if text.ends_with(".0") {
        text.truncate(text.len() - 2);
    }
    text
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":{"list":[{"slug":"sample","title":"Sample ComicLand","cover_url":"https://img.example/cover.jpg"}],"has_more":false}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"slug":"sample","title":"Sample ComicLand","cover_url":"https://img.example/cover.jpg","description":"A sample.","authors":[{"name":"Author"}],"artists":[{"name":"Artist"}],"genres":[{"name":"Action"}],"chapters":[{"chapter_index":1.0,"title":"Chapter 1"}]}}"#;
const PAGES_FIXTURE: &str =
    r#"{"data":{"pages":["https://img.example/page1.jpg","https://img.example/page2.jpg"]}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_comicland_api() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].key, "sample");
        let chapters = SOURCE.chapters(json!({"manga":"sample"})).unwrap();
        assert_eq!(chapters[0].key, "sample/1");
        let pages = SOURCE.pages(json!({"chapter":"sample/1"})).unwrap();
        assert_eq!(pages.len(), 2);
    }
}
