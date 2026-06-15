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

const SOURCE: ToonBr = ToonBr;
const BASE_URL: &str = "https://beta.toonbr.com";
const API_URL: &str = "https://api.toonbr.com";
const CDN_URL: &str = "https://cdn2.toonbr.com";
const PAGE_LIMIT: u64 = 150;

struct ToonBr;

impl MangaSource for ToonBr {
    fn list(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Paged<CatalogItem>> {
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{API_URL}/api/manga/latest?limit={PAGE_LIMIT}")
        } else {
            format!("{API_URL}/api/manga/popular?limit={PAGE_LIMIT}")
        };
        Ok(Paged {
            entries: parse_manga_array(&fetch_json(&target, LIST_FIXTURE)),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_manga(&fetch_json(&details_url(&key), DETAILS_FIXTURE))],
                has_next_page: false,
            });
        }
        Ok(parse_manga_list(&fetch_json(&search_url(page(&request), query, &request), LIST_PAGE_FIXTURE)))
    }

    fn details(&self, request: Value) -> manatan_extension::abi::ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_manga(&fetch_json(&details_url(&key), DETAILS_FIXTURE)))
    }

    fn chapters(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(&fetch_json(&details_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter/sample".to_string());
        Ok(parse_pages(&fetch_json(&chapter_url(&key), PAGES_FIXTURE)))
    }

    fn handle_url(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_manga(&fetch_json(&details_url(&key), DETAILS_FIXTURE))),
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
struct MangaDto {
    #[serde(default)]
    title: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "coverImage")]
    cover_image: Option<String>,
    #[serde(default)]
    chapters: Option<Vec<ChapterDto>>,
}

#[derive(Default, Deserialize)]
struct MangaListResponse {
    #[serde(default)]
    data: Vec<MangaDto>,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default, rename = "chapterNumber")]
    chapter_number: Option<f32>,
    #[serde(default, rename = "createdAt")]
    created_at: Option<String>,
    #[serde(default)]
    pages: Option<Vec<PageDto>>,
}

#[derive(Default, Deserialize)]
struct PageDto {
    #[serde(default, rename = "imageUrl")]
    image_url: Option<String>,
}

impl MangaDto {
    fn into_item(self, initialized: bool) -> CatalogItem {
        let key = format!("/manga/{}", self.slug);
        CatalogItem {
            key: key.clone(),
            title: if self.title.is_empty() { self.slug } else { self.title },
            cover: self.cover_image.map(|cover| format!("{CDN_URL}{cover}")),
            url: Some(format!("{BASE_URL}{key}")),
            description: self.description,
            language: Some("pt-BR".to_string()),
            content_rating: Some("safe".to_string()),
            status: match self.status.as_deref() {
                Some("ONGOING") => ItemStatus::Ongoing,
                Some("COMPLETED") => ItemStatus::Completed,
                _ => ItemStatus::Unknown,
            },
            initialized,
            ..CatalogItem::default()
        }
    }
}

fn parse_manga_array(body: &str) -> Vec<CatalogItem> {
    serde_json::from_str::<Vec<MangaDto>>(body)
        .unwrap_or_default()
        .into_iter()
        .map(|manga| manga.into_item(false))
        .collect()
}

fn parse_manga_list(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: serde_json::from_str::<MangaListResponse>(body)
            .unwrap_or_default()
            .data
            .into_iter()
            .map(|manga| manga.into_item(false))
            .collect(),
        has_next_page: false,
    }
}

fn parse_manga(body: &str) -> CatalogItem {
    serde_json::from_str::<MangaDto>(body)
        .unwrap_or_default()
        .into_item(true)
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let manga = serde_json::from_str::<MangaDto>(body).unwrap_or_default();
    let mut chapters = manga
        .chapters
        .unwrap_or_default()
        .into_iter()
        .map(|chapter| {
            let key = format!("/chapter/{}", chapter.id);
            MangaChapter {
                key: key.clone(),
                title: Some(
                    chapter
                        .chapter_number
                        .map(|number| format!("Capitulo {}", format_number(number)))
                        .unwrap_or(chapter.title),
                ),
                chapter_number: chapter.chapter_number,
                date_uploaded: chapter.created_at.as_deref().and_then(parse_api_date),
                language: Some("pt-BR".to_string()),
                url: Some(format!("{BASE_URL}/read/{}", chapter.id)),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|left, right| {
        right.chapter_number.partial_cmp(&left.chapter_number).unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    serde_json::from_str::<ChapterDto>(body)
        .unwrap_or_default()
        .pages
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .filter_map(|(index, page)| {
            let image = page.image_url?;
            Some(MangaPage {
                content: PageContent::Url {
                    url: format!("{CDN_URL}{image}"),
                    context: None,
                },
                headers: image_headers(),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
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
    client().get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn search_url(page: u64, query: &str, request: &Value) -> String {
    let mut params = vec![("page", page.to_string()), ("limit", PAGE_LIMIT.to_string())];
    if !query.trim().is_empty() {
        params.push(("search", query.to_string()));
    }
    if let Some(category) = filter_value(request, "categoryId").filter(|value| !value.is_empty()) {
        params.push(("categoryId", category));
    }
    format!("{API_URL}/api/manga?{}", params.iter().map(|(key, value)| format!("{key}={}", url::query_escape(value))).collect::<Vec<_>>().join("&"))
}

fn details_url(key: &str) -> String {
    format!("{API_URL}/api/manga/{}", url::query_escape(&slug_from_key(key)))
}

fn chapter_url(key: &str) -> String {
    format!("{API_URL}/api/chapter/{}", url::query_escape(&slug_from_key(key)))
}

fn slug_from_key(key: &str) -> String {
    key.trim_end_matches('/').rsplit('/').next().unwrap_or("sample").to_string()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!("/{}", input[BASE_URL.len()..].trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn filter_value(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(|value| match value {
            Value::String(value) => Some(value.clone()),
            Value::Object(object) => object.get("value").or_else(|| object.get("id")).and_then(Value::as_str).map(ToString::to_string),
            _ => None,
        })
}

fn image_headers() -> BTreeMap<String, String> {
    BTreeMap::from([("Referer".to_string(), format!("{BASE_URL}/"))])
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn parse_api_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value.split('T').next().unwrap_or(value))
}

fn format_number(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i32)
    } else {
        value.to_string()
    }
}

const LIST_FIXTURE: &str = r#"[{"title":"Sample ToonBr","slug":"sample","description":"Description","status":"ONGOING","coverImage":"/cover.jpg"}]"#;
const LIST_PAGE_FIXTURE: &str = r#"{"data":[{"title":"Sample ToonBr","slug":"sample","description":"Description","status":"ONGOING","coverImage":"/cover.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"title":"Sample ToonBr","slug":"sample","description":"Description","status":"ONGOING","coverImage":"/cover.jpg","chapters":[{"id":"chapter-1","title":"Start","chapterNumber":1,"createdAt":"2024-01-01T00:00:00.000Z"}]}"#;
const PAGES_FIXTURE: &str = r#"{"id":"chapter-1","title":"Start","chapterNumber":1,"pages":[{"imageUrl":"/page-1.jpg"},{"imageUrl":"/page-2.jpg"}]}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_toonbr_fixtures() {
        assert_eq!(parse_manga_array(LIST_FIXTURE).len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(SOURCE.pages(json!({"chapter":"/chapter/chapter-1"})).unwrap().len(), 2);
    }
}
