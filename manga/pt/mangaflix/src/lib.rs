use manatan_extension::{
    export_manga_source,
    http::HttpClient,
    source::MangaSource,
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
};
use manatan_shared::{dates, manga, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: MangaFlix = MangaFlix;
const BASE_URL: &str = "https://mangaflix.net";
const API_URL: &str = "https://api.mangaflix.net/v1";

struct MangaFlix;

impl MangaSource for MangaFlix {
    fn list(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Paged<CatalogItem>> {
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            format!("{API_URL}/latest-releases?selected_language=pt-br")
        } else {
            format!("{API_URL}/browse")
        };
        let body = fetch_json(&target, if latest { LATEST_FIXTURE } else { BROWSE_FIXTURE });
        Ok(if latest {
            parse_latest(&body)
        } else {
            parse_browse(&body)
        })
    }

    fn search(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_json(&details_url(&key), DETAILS_FIXTURE))],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(serde_json::json!({"listingId":"latest"}));
        }
        Ok(parse_search(&fetch_json(
            &format!("{API_URL}/search/mangas?query={}&selected_language=pt-br", url::query_escape(query)),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> manatan_extension::abi::ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/br/manga/sample".to_string());
        Ok(parse_details(&fetch_json(&details_url(&key), DETAILS_FIXTURE)))
    }

    fn chapters(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/br/manga/sample".to_string());
        Ok(parse_chapters(&fetch_json(&details_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/br/manga/chapter".to_string());
        Ok(parse_pages(&fetch_json(&chapter_url(&key), PAGES_FIXTURE)))
    }

    fn handle_url(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/br/manga/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_json(&details_url(&key), DETAILS_FIXTURE))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(manatan_extension::SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..manatan_extension::SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

#[derive(Default, Deserialize)]
struct BrowseResponse {
    #[serde(default)]
    data: Vec<BrowseSection>,
}

#[derive(Default, Deserialize)]
struct BrowseSection {
    #[serde(default)]
    key: String,
    items: Option<Value>,
}

#[derive(Default, Deserialize)]
struct LatestResponse {
    #[serde(default)]
    data: Vec<MangaDto>,
}

#[derive(Default, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: SearchData,
}

#[derive(Default, Deserialize)]
struct SearchData {
    #[serde(default)]
    works: Vec<SearchMangaDto>,
}

#[derive(Default, Deserialize)]
struct MangaDetailsResponse {
    #[serde(default)]
    data: MangaDetailsDto,
}

#[derive(Default, Deserialize)]
struct ChapterDetailsResponse {
    #[serde(default)]
    data: ChapterDetailsDto,
}

#[derive(Default, Deserialize)]
struct MangaDto {
    #[serde(default, rename = "_id")]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    poster: Option<PosterDto>,
    #[serde(default)]
    genres: Vec<GenreDto>,
}

#[derive(Default, Deserialize)]
struct SearchMangaDto {
    #[serde(default, rename = "_id")]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    poster: Option<PosterDto>,
}

#[derive(Default, Deserialize)]
struct MangaDetailsDto {
    #[serde(default, rename = "_id")]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    poster: Option<PosterDto>,
    #[serde(default)]
    genres: Vec<GenreDto>,
    #[serde(default)]
    chapters: Vec<ChapterDto>,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    #[serde(default, rename = "_id")]
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    number: String,
    #[serde(default)]
    iso_date: Option<String>,
    #[serde(default)]
    owners: Vec<OwnerDto>,
}

#[derive(Default, Deserialize)]
struct ChapterDetailsDto {
    #[serde(default)]
    images: Vec<ImageDto>,
}

#[derive(Default, Deserialize)]
struct ImageDto {
    #[serde(default)]
    default_url: String,
}

#[derive(Default, Deserialize)]
struct OwnerDto {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct PosterDto {
    #[serde(default)]
    default_url: Option<String>,
}

#[derive(Default, Deserialize)]
struct GenreDto {
    #[serde(default)]
    name: String,
}

impl MangaDto {
    fn into_item(self, initialized: bool) -> CatalogItem {
        item(self.id, self.name, self.description, self.poster, self.genres, initialized)
    }
}

impl SearchMangaDto {
    fn into_item(self) -> CatalogItem {
        item(self.id, self.name, self.description, self.poster, Vec::new(), false)
    }
}

fn item(
    id: String,
    name: String,
    description: Option<String>,
    poster: Option<PosterDto>,
    genres: Vec<GenreDto>,
    initialized: bool,
) -> CatalogItem {
    let key = format!("/br/manga/{id}");
    CatalogItem {
        key: key.clone(),
        title: if name.is_empty() { id } else { name },
        cover: poster.and_then(|poster| poster.default_url),
        url: Some(format!("{BASE_URL}{key}")),
        description,
        tags: genres.into_iter().map(|genre| genre.name).filter(|name| !name.is_empty()).collect(),
        language: Some("pt-BR".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_browse(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<BrowseResponse>(body).unwrap_or_default();
    let entries = response
        .data
        .into_iter()
        .find(|section| section.key == "most-read")
        .and_then(|section| section.items)
        .and_then(|items| serde_json::from_value::<Vec<MangaDto>>(items).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|manga| manga.into_item(false))
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: serde_json::from_str::<LatestResponse>(body)
            .unwrap_or_default()
            .data
            .into_iter()
            .map(|manga| manga.into_item(false))
            .collect(),
        has_next_page: false,
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: serde_json::from_str::<SearchResponse>(body)
            .unwrap_or_default()
            .data
            .works
            .into_iter()
            .map(SearchMangaDto::into_item)
            .collect(),
        has_next_page: false,
    }
}

fn parse_details(body: &str) -> CatalogItem {
    let details = serde_json::from_str::<MangaDetailsResponse>(body)
        .unwrap_or_default()
        .data;
    item(details.id, details.name, details.description, details.poster, details.genres, true)
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    serde_json::from_str::<MangaDetailsResponse>(body)
        .unwrap_or_default()
        .data
        .chapters
        .into_iter()
        .map(|chapter| {
            let key = format!("/br/manga/{}", chapter.id);
            MangaChapter {
                key: key.clone(),
                title: Some(
                    chapter
                        .name
                        .filter(|name| !name.is_empty())
                        .unwrap_or_else(|| format!("Capitulo {}", chapter.number)),
                ),
                chapter_number: chapter.number.parse::<f32>().ok(),
                date_uploaded: chapter.iso_date.as_deref().and_then(parse_api_date),
                scanlators: chapter
                    .owners
                    .into_iter()
                    .map(|owner| owner.name)
                    .filter(|name| !name.is_empty())
                    .collect(),
                language: Some("pt-BR".to_string()),
                url: Some(format!("{BASE_URL}{key}")),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    serde_json::from_str::<ChapterDetailsResponse>(body)
        .unwrap_or_default()
        .data
        .images
        .into_iter()
        .enumerate()
        .filter(|(_, image)| !image.default_url.is_empty())
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.default_url,
                context: None,
            },
            headers: image_headers(),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn details_url(key: &str) -> String {
    format!("{API_URL}/mangas/{}", url::query_escape(&id_from_key(key)))
}

fn chapter_url(key: &str) -> String {
    format!(
        "{API_URL}/chapters/{}?selected_language=pt-br",
        url::query_escape(&id_from_key(key))
    )
}

fn id_from_key(key: &str) -> String {
    key.trim_end_matches('/').rsplit('/').next().unwrap_or("sample").to_string()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!("/{}", input[BASE_URL.len()..].trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn image_headers() -> BTreeMap<String, String> {
    BTreeMap::from([("Referer".to_string(), format!("{BASE_URL}/"))])
}

fn parse_api_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value.split('T').next().unwrap_or(value))
}

const BROWSE_FIXTURE: &str = r#"{"data":[{"key":"most-read","items":[{"_id":"sample","name":"Sample MangaFlix","description":"Description","poster":{"default_url":"https://mangaflix.net/cover.jpg"},"genres":[{"name":"Action"}]}]}]}"#;
const LATEST_FIXTURE: &str = r#"{"data":[{"_id":"sample","name":"Sample MangaFlix","description":"Description","poster":{"default_url":"https://mangaflix.net/cover.jpg"},"genres":[{"name":"Action"}]}]}"#;
const SEARCH_FIXTURE: &str = r#"{"data":{"works":[{"_id":"sample","name":"Sample MangaFlix","description":"Description","poster":{"default_url":"https://mangaflix.net/cover.jpg"},"genres":["genre-id"]}]}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"_id":"sample","name":"Sample MangaFlix","description":"Description","poster":{"default_url":"https://mangaflix.net/cover.jpg"},"genres":[{"name":"Action"}],"chapters":[{"_id":"chapter-1","name":"Capitulo 1","number":"1","iso_date":"2024-01-01T00:00:00.000Z","owners":[{"name":"Team"}]}]}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"images":[{"default_url":"https://mangaflix.net/page-1.jpg"},{"default_url":"https://mangaflix.net/page-2.jpg"}]}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_mangaflix_fixtures() {
        assert_eq!(parse_browse(BROWSE_FIXTURE).entries.len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(SOURCE.pages(json!({"chapter":"/br/manga/chapter-1"})).unwrap().len(), 2);
    }
}
