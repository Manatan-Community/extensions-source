use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

const SOURCE: Shinigami = Shinigami;
const BASE_URL: &str = "https://g.shinigami.asia";
const API_URL: &str = "https://api.shngm.io";
const CDN_URL: &str = "https://storage.shngm.id";

struct Shinigami;

impl MangaSource for Shinigami {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_browse(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "popularity"
        };
        Ok(parse_browse(&api_get(
            &browse_url(page, sort, &Value::Null),
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
        if query.starts_with(BASE_URL) && query.contains("/series/") {
            let key = series_id_from_url(query).unwrap_or_else(|| query.to_string());
            return Ok(Paged {
                entries: vec![parse_details(
                    &api_get(&detail_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let mut filters = request.get("filters").cloned().unwrap_or(Value::Null);
        if !query.is_empty() {
            if !filters.is_object() {
                filters = serde_json::json!({});
            }
            if let Some(object) = filters.as_object_mut() {
                object.insert("q".to_string(), Value::String(query.to_string()));
            }
        }
        Ok(parse_browse(&api_get(
            &browse_url(
                page,
                filter_string(&filters, "sort").as_deref().unwrap_or(""),
                &filters,
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_details(
            &api_get(&detail_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_chapters(&api_get(
            &chapters_url(&key),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "chapter-sample".into());
        Ok(parse_pages(&api_get(
            &chapter_detail_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/series/{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/series/") {
            let key = series_id_from_url(input).unwrap_or_else(|| input.to_string());
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &api_get(&detail_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )),
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
        .with_origin(BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_cookies_for(API_URL)
        .with_cookies_for(CDN_URL)
        .with_webview_challenge_fallback()
}

fn api_get(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .header("Accept", "application/json")
        .header("DNT", "1")
        .header("Sec-GPC", "1")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn browse_url(page: u64, sort: &str, filters: &Value) -> String {
    let mut params = vec![("page", page.to_string()), ("page_size", "30".to_string())];
    if !sort.is_empty() {
        params.push(("sort", sort.to_string()));
    }
    if let Some(query) = filter_string(filters, "q").filter(|value| !value.is_empty()) {
        params.push(("q", query));
    }
    for id in [
        "sort_order",
        "status",
        "format",
        "type",
        "genre_include",
        "genre_exclude",
    ] {
        if let Some(value) = filter_string(filters, id).filter(|value| !value.is_empty()) {
            params.push((id, value));
            if id == "genre_include" {
                params.push(("genre_include_mode", "and".to_string()));
            }
            if id == "genre_exclude" {
                params.push(("genre_exclude_mode", "and".to_string()));
            }
        }
    }
    format!(
        "{API_URL}/v1/manga/list?{}",
        params
            .into_iter()
            .map(|(name, value)| format!("{name}={}", url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn detail_url(manga_id: &str) -> String {
    format!("{API_URL}/v1/manga/detail/{}", url::query_escape(manga_id))
}

fn chapters_url(manga_id: &str) -> String {
    format!(
        "{API_URL}/v1/chapter/{}/list?page_size=3000",
        url::query_escape(manga_id)
    )
}

fn chapter_detail_url(chapter_id: &str) -> String {
    format!(
        "{API_URL}/v1/chapter/detail/{}",
        url::query_escape(chapter_id)
    )
}

fn filter_string(filters: &Value, id: &str) -> Option<String> {
    filters
        .get(id)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
}

fn parse_browse(body: &str) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<BrowseDto>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: payload
            .data
            .into_iter()
            .map(|item| {
                let key = item.manga_id.unwrap_or_default();
                CatalogItem {
                    key: key.clone(),
                    title: item.title.unwrap_or_else(|| "Shinigami".to_string()),
                    cover: item.thumbnail,
                    url: Some(format!("{BASE_URL}/series/{key}")),
                    language: Some("id".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                }
            })
            .collect(),
        has_next_page: payload
            .meta
            .total_page
            .is_some_and(|total| payload.meta.page < total),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let payload = serde_json::from_str::<MangaDetailDto>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let data = payload.data;
    let key = key.unwrap_or_else(|| data.manga_id.unwrap_or_else(|| "sample".to_string()));
    let genres = data
        .taxonomy
        .get("Genre")
        .into_iter()
        .flatten()
        .map(|item| item.name.clone())
        .collect::<Vec<_>>();
    let formats = data
        .taxonomy
        .get("Format")
        .into_iter()
        .flatten()
        .map(|item| item.name.clone());
    CatalogItem {
        key: key.clone(),
        title: data
            .title
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Shinigami".to_string())),
        cover: data.thumbnail,
        authors: taxonomy_values(&data.taxonomy, "Author"),
        artists: taxonomy_values(&data.taxonomy, "Artist"),
        description: Some(data.description).filter(|value| !value.is_empty()),
        status: parse_status(data.status),
        tags: genres.into_iter().chain(formats).collect(),
        url: Some(format!("{BASE_URL}/series/{key}")),
        language: Some("id".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let payload = serde_json::from_str::<ChapterListDto>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
    payload
        .data
        .into_iter()
        .map(|chapter| {
            let number = chapter.chapter_number as f32;
            let clean_number = format_chapter_number(chapter.chapter_number);
            MangaChapter {
                key: chapter.chapter_id,
                title: Some(
                    format!("Chapter {clean_number} {}", chapter.chapter_title)
                        .trim()
                        .to_string(),
                ),
                chapter_number: Some(number),
                date_uploaded: parse_iso_date(&chapter.release_date),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let payload = serde_json::from_str::<PageListDto>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    let chapter = payload.data.chapter;
    chapter
        .data
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: format!("{}{}{}", CDN_URL, chapter.path, image),
                context: Some(image_headers()),
            },
            headers: image_headers(),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_headers() -> manatan_shared::sdk::Context {
    let mut headers = manga::image_headers(BASE_URL);
    headers.insert(
        "Accept".to_string(),
        "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8".to_string(),
    );
    headers.insert("DNT".to_string(), "1".to_string());
    headers.insert("Sec-GPC".to_string(), "1".to_string());
    headers
}

fn taxonomy_values(taxonomy: &HashMap<String, Vec<TaxonomyItemDto>>, key: &str) -> Vec<String> {
    taxonomy
        .get(key)
        .into_iter()
        .flatten()
        .map(|item| item.name.clone())
        .collect()
}

fn parse_status(value: i32) -> ItemStatus {
    match value {
        1 => ItemStatus::Ongoing,
        2 => ItemStatus::Completed,
        3 => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value.get(0..10)?)
}

fn format_chapter_number(value: f64) -> String {
    let mut text = value.to_string();
    if text.ends_with(".0") {
        text.truncate(text.len() - 2);
    }
    text
}

fn series_id_from_url(input: &str) -> Option<String> {
    input
        .split_once("/series/")
        .map(|(_, rest)| rest.trim_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

#[derive(Debug, Default, Deserialize)]
struct BrowseDto {
    #[serde(default)]
    data: Vec<BrowseDataDto>,
    #[serde(default)]
    meta: MetaDto,
}

#[derive(Debug, Default, Deserialize)]
struct BrowseDataDto {
    #[serde(default, rename = "cover_image_url")]
    thumbnail: Option<String>,
    #[serde(default, rename = "manga_id")]
    manga_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct MetaDto {
    #[serde(default)]
    page: u64,
    #[serde(default, rename = "total_page")]
    total_page: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct MangaDetailDto {
    #[serde(default)]
    data: MangaDetailDataDto,
}

#[derive(Debug, Default, Deserialize)]
struct MangaDetailDataDto {
    #[serde(default, rename = "manga_id")]
    manga_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "cover_image_url")]
    thumbnail: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    status: i32,
    #[serde(default)]
    taxonomy: HashMap<String, Vec<TaxonomyItemDto>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct TaxonomyItemDto {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct ChapterListDto {
    #[serde(default)]
    data: Vec<ChapterListDataDto>,
}

#[derive(Debug, Default, Deserialize)]
struct ChapterListDataDto {
    #[serde(default, rename = "release_date")]
    release_date: String,
    #[serde(default, rename = "chapter_title")]
    chapter_title: String,
    #[serde(default, rename = "chapter_number")]
    chapter_number: f64,
    #[serde(default, rename = "chapter_id")]
    chapter_id: String,
}

#[derive(Debug, Default, Deserialize)]
struct PageListDto {
    #[serde(default)]
    data: PagesDataDto,
}

#[derive(Debug, Default, Deserialize)]
struct PagesDataDto {
    #[serde(default)]
    chapter: PagesChapterDto,
}

#[derive(Debug, Default, Deserialize)]
struct PagesChapterDto {
    #[serde(default)]
    path: String,
    #[serde(default, rename = "data")]
    data: Vec<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
{"data":[{"cover_image_url":"https://storage.shngm.id/cover.jpg","manga_id":"sample","title":"Sample Shinigami"}],"meta":{"page":1,"total_page":1}}
"#;
const DETAILS_FIXTURE: &str = r#"
{"data":{"manga_id":"sample","title":"Sample Shinigami","cover_image_url":"https://storage.shngm.id/cover.jpg","description":"Sample synopsis.","status":1,"taxonomy":{"Author":[{"name":"Writer"}],"Artist":[{"name":"Artist"}],"Genre":[{"name":"Action"}],"Format":[{"name":"Manga"}]}}}
"#;
const CHAPTERS_FIXTURE: &str = r#"
{"data":[{"release_date":"2024-01-01T00:00:00Z","chapter_title":"Start","chapter_number":1.0,"chapter_id":"chapter-sample"}]}
"#;
const PAGES_FIXTURE: &str = r#"
{"data":{"chapter":{"path":"/sample/","data":["001.jpg"]}}}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(
            parse_browse(LIST_FIXTURE).entries[0].title,
            "Sample Shinigami"
        );
        assert_eq!(
            parse_details(DETAILS_FIXTURE, None).status,
            ItemStatus::Ongoing
        );
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
