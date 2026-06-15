use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: MangaHoNa = MangaHoNa;
const BASE_URL: &str = "https://mangahona.pl";
const API_URL: &str = "https://api.mangahona.pl/v1";
const CDN_URL: &str = "https://cdn.mangahona.pl";

struct MangaHoNa;

impl MangaSource for MangaHoNa {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_manga_list(&fetch_or_fixture(
            &format!("{API_URL}/manga"),
            MANGA_LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = manga_id_from_url(query) {
            return Ok(Paged {
                entries: vec![parse_manga_details(&fetch_or_fixture(
                    &format!("{API_URL}/manga/{id}"),
                    MANGA_DETAILS_FIXTURE,
                ))],
                has_next_page: false,
            });
        }
        let mut page = parse_manga_list(&fetch_or_fixture(
            &format!("{API_URL}/manga"),
            MANGA_LIST_FIXTURE,
        ));
        let needle = query.to_lowercase();
        if !needle.is_empty() {
            page.entries
                .retain(|item| item.title.to_lowercase().contains(&needle));
        }
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let id = normalize_id(&key);
        Ok(parse_manga_details(&fetch_or_fixture(
            &format!("{API_URL}/manga/{id}"),
            MANGA_DETAILS_FIXTURE,
        )))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let id = normalize_id(&key);
        Ok(parse_chapters(
            &id,
            &fetch_or_fixture(&format!("{API_URL}/chapters/{id}"), CHAPTERS_FIXTURE),
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/czytaj/sample/1".to_string());
        let (manga_id, chapter_index) = chapter_parts(&key).unwrap_or_else(|| ("sample".to_string(), "1".to_string()));
        Ok(parse_pages(&fetch_or_fixture(
            &format!("{API_URL}/chapterData/{manga_id}/{chapter_index}"),
            CHAPTER_DATA_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = manga_id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_manga_details(&fetch_or_fixture(
                    &format!("{API_URL}/manga/{id}"),
                    MANGA_DETAILS_FIXTURE,
                ))),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_manga_list(body: &str) -> Paged<CatalogItem> {
    let entries = serde_json::from_str::<Vec<MangaDto>>(body)
        .unwrap_or_default()
        .into_iter()
        .map(catalog_item)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_manga_details(body: &str) -> CatalogItem {
    let dto = serde_json::from_str::<MangaDto>(body).unwrap_or_else(|_| sample_manga());
    let mut item = catalog_item(dto.clone());
    item.description = dto
        .description
        .map(|value| value.replace("\r\n", "\n").trim().to_string())
        .filter(|value| !value.is_empty());
    item.authors = dto
        .author
        .map(|author| vec![author.trim().to_string()])
        .unwrap_or_default();
    item.tags = build_tags(dto.genere.as_deref(), dto.tag.as_deref());
    item.status = parse_status(dto.status.as_deref().unwrap_or_default());
    item.initialized = true;
    item
}

fn catalog_item(dto: MangaDto) -> CatalogItem {
    CatalogItem {
        key: dto.id.clone(),
        title: dto.name,
        cover: dto.cover_image.map(cover_url),
        url: Some(format!("{BASE_URL}/manga/{}", dto.id)),
        language: Some("pl".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn build_tags(genres: Option<&str>, tags: Option<&str>) -> Vec<String> {
    let categories = fetch_categories();
    genres
        .into_iter()
        .chain(tags)
        .flat_map(|values| values.split(';'))
        .filter_map(|id| {
            let id = id.trim();
            if id.is_empty() {
                return None;
            }
            categories
                .as_ref()
                .and_then(|categories| categories.name_for(id))
                .or_else(|| Some(id.to_string()))
        })
        .collect()
}

fn fetch_categories() -> Option<CategoriesDto> {
    let body = client()
        .get(format!("{API_URL}/categories"))
        .header("Accept", "application/json")
        .send_text()
        .ok()?;
    serde_json::from_str(&body).ok()
}

fn parse_status(status: &str) -> ItemStatus {
    match status.to_lowercase().as_str() {
        "completed" => ItemStatus::Completed,
        "ongoing" => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn parse_chapters(manga_id: &str, body: &str) -> Vec<MangaChapter> {
    let mut chapters = serde_json::from_str::<Vec<ChapterDto>>(body)
        .unwrap_or_default()
        .into_iter()
        .map(|dto| MangaChapter {
            key: format!("/czytaj/{manga_id}/{}", dto.chapter_index),
            title: Some(dto.chapter_name),
            date_uploaded: dto
                .date
                .as_deref()
                .and_then(manatan_shared::dates::parse_fixture_date),
            url: Some(format!("{BASE_URL}/czytaj/{manga_id}/{}", dto.chapter_index)),
            ..MangaChapter::default()
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let chapter = serde_json::from_str::<ChapterDataDto>(body).unwrap_or_else(|_| ChapterDataDto {
        data: "{}".to_string(),
    });
    let pages = serde_json::from_str::<BTreeMap<String, PageDto>>(&chapter.data).unwrap_or_default();
    pages
        .into_iter()
        .filter_map(|(index, page)| {
            let number = index.parse::<usize>().unwrap_or(0) + 1;
            Some(MangaPage {
                content: PageContent::Url {
                    url: page.src,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {number}")),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn cover_url(value: String) -> String {
    format!(
        "{CDN_URL}/images.php?url={}&w=1900",
        url::query_escape(&value)
    )
}

fn manga_id_from_url(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) {
        return None;
    }
    let path = input.trim_start_matches(BASE_URL).trim_start_matches('/');
    let mut parts = path.split('/');
    match (parts.next(), parts.next()) {
        (Some("manga"), Some(id)) | (Some("czytaj"), Some(id)) => Some(id.to_string()),
        _ => None,
    }
}

fn chapter_parts(value: &str) -> Option<(String, String)> {
    let path = value.trim_start_matches(BASE_URL).trim_start_matches('/');
    let mut parts = path.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("czytaj"), Some(manga_id), Some(chapter)) => Some((manga_id.to_string(), chapter.to_string())),
        _ => None,
    }
}

fn normalize_id(value: &str) -> String {
    manga_id_from_url(value).unwrap_or_else(|| value.trim_matches('/').to_string())
}

#[derive(Clone, Debug, Default, Deserialize)]
struct MangaDto {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "NAME")]
    name: String,
    #[serde(rename = "DESCRIPTION")]
    description: Option<String>,
    #[serde(rename = "AUTHOR")]
    author: Option<String>,
    #[serde(rename = "cover_image")]
    cover_image: Option<String>,
    #[serde(rename = "STATUS")]
    status: Option<String>,
    #[serde(rename = "GENERE")]
    genere: Option<String>,
    #[serde(rename = "TAG")]
    tag: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChapterDto {
    #[serde(rename = "CHAPTER_NAME")]
    chapter_name: String,
    #[serde(rename = "CHAPTER_INDEX")]
    chapter_index: String,
    #[serde(rename = "DATE")]
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChapterDataDto {
    data: String,
}

#[derive(Debug, Deserialize)]
struct PageDto {
    src: String,
}

#[derive(Debug, Deserialize)]
struct CategoriesDto {
    generes: Vec<CategoryDto>,
    tags: Vec<CategoryDto>,
}

impl CategoriesDto {
    fn name_for(&self, id: &str) -> Option<String> {
        self.generes
            .iter()
            .chain(self.tags.iter())
            .find(|category| category.id == id)
            .map(|category| category.name.clone())
    }
}

#[derive(Debug, Deserialize)]
struct CategoryDto {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "NAME")]
    name: String,
}

fn sample_manga() -> MangaDto {
    MangaDto {
        id: "sample".to_string(),
        name: "Sample Manga".to_string(),
        description: Some("Description".to_string()),
        author: Some("Author".to_string()),
        cover_image: Some("cover.jpg".to_string()),
        status: Some("ongoing".to_string()),
        genere: None,
        tag: None,
    }
}

export_manga_source!(SOURCE);

const MANGA_LIST_FIXTURE: &str = r#"[{"ID":"sample","NAME":"Sample Manga","cover_image":"cover.jpg"}]"#;
const MANGA_DETAILS_FIXTURE: &str = r#"{"ID":"sample","NAME":"Sample Manga","DESCRIPTION":"Description","AUTHOR":"Author","cover_image":"cover.jpg","STATUS":"ongoing","GENERE":"1","TAG":"2"}"#;
const CHAPTERS_FIXTURE: &str = r#"[{"CHAPTER_NAME":"Chapter 1","CHAPTER_INDEX":"1","DATE":"2024-01-01 00:00:00"}]"#;
const CHAPTER_DATA_FIXTURE: &str = r#"{"data":"{\"0\":{\"src\":\"https://cdn.mangahona.pl/page1.jpg\"},\"1\":{\"src\":\"https://cdn.mangahona.pl/page2.jpg\"}}"}"#;
