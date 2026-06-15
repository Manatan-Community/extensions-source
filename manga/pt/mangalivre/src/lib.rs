use manatan_extension::{
    export_manga_source,
    http::HttpClient,
    source::MangaSource,
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
};
use manatan_shared::{manga, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: MangaLivre = MangaLivre;
const BASE_URL: &str = "https://toonlivre.net";
const API_URL: &str = "https://toonlivre.net/api";

struct MangaLivre;

impl MangaSource for MangaLivre {
    fn list(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Paged<CatalogItem>> {
        let listing = request.get("listingId").and_then(Value::as_str).unwrap_or("popular");
        let target = match listing {
            "latest" => search_url(page(&request), "", "updated", "desc"),
            _ => search_url(page(&request), "", "popular", "desc"),
        };
        Ok(parse_wrapper(&fetch_json(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_json(&details_url(&key), DETAILS_FIXTURE))],
                has_next_page: false,
            });
        }
        let sort_by = filter_value(&request, "sortBy").unwrap_or_else(|| "popular".to_string());
        let sort_order = filter_value(&request, "sortOrder").unwrap_or_else(|| "desc".to_string());
        Ok(parse_wrapper(&fetch_json(
            &search_url(page(&request), query, &sort_by, &sort_order),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> manatan_extension::abi::ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_details(&fetch_json(&details_url(&key), DETAILS_FIXTURE)))
    }

    fn chapters(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_chapters(&fetch_json(&details_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample#chapter".to_string());
        Ok(parse_pages(&fetch_json(&pages_url(&key), PAGES_FIXTURE)))
    }

    fn handle_url(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
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
struct WrapperDto {
    #[serde(default)]
    mangas: Vec<MangaDto>,
    #[serde(default)]
    pagination: PaginationDto,
}

#[derive(Default, Deserialize)]
struct PaginationDto {
    #[serde(default, rename = "hasNextPage")]
    has_next_page: bool,
}

#[derive(Default, Deserialize)]
struct MangaDto {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default, rename = "coverUrl")]
    cover_url: Option<String>,
    #[serde(default)]
    authors: Option<Vec<String>>,
    #[serde(default)]
    artists: Option<Vec<String>>,
    #[serde(default)]
    genres: Option<Vec<String>>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "alternativeTitle")]
    alternative_title: Option<String>,
    #[serde(default)]
    chapters: Option<Vec<ChapterDto>>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    #[serde(default)]
    id: String,
    #[serde(default)]
    number: String,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Default, Deserialize)]
struct PageDto {
    #[serde(default)]
    pages: Vec<String>,
}

impl MangaDto {
    fn into_item(self, initialized: bool) -> CatalogItem {
        let mut description = self.description.unwrap_or_default();
        if let Some(alternative) = self.alternative_title.filter(|value| !value.is_empty()) {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str("Alternative title: ");
            description.push_str(&alternative);
        }
        CatalogItem {
            key: self.id.clone(),
            title: if self.title.is_empty() { self.id.clone() } else { self.title },
            cover: self.cover_url,
            url: Some(format!("{BASE_URL}/{}", self.id)),
            authors: self.authors.unwrap_or_default(),
            artists: self.artists.unwrap_or_default(),
            description: (!description.is_empty()).then_some(description),
            tags: self.genres.unwrap_or_default(),
            language: Some("pt-BR".to_string()),
            content_rating: Some("safe".to_string()),
            status: match self.status.as_deref().map(str::to_ascii_lowercase).as_deref() {
                Some("ongoing") => ItemStatus::Ongoing,
                Some("completed") => ItemStatus::Completed,
                _ => ItemStatus::Unknown,
            },
            initialized,
            ..CatalogItem::default()
        }
    }
}

fn parse_wrapper(body: &str) -> Paged<CatalogItem> {
    let wrapper = serde_json::from_str::<WrapperDto>(body).unwrap_or_default();
    Paged {
        entries: wrapper.mangas.into_iter().map(|manga| manga.into_item(false)).collect(),
        has_next_page: wrapper.pagination.has_next_page,
    }
}

fn parse_details(body: &str) -> CatalogItem {
    serde_json::from_str::<MangaDto>(body)
        .unwrap_or_default()
        .into_item(true)
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let manga = serde_json::from_str::<MangaDto>(body).unwrap_or_default();
    let manga_id = manga.id.clone();
    manga.chapters
        .unwrap_or_default()
        .into_iter()
        .map(|chapter| {
            let key = format!("{manga_id}#{}", chapter.id);
            MangaChapter {
                key: key.clone(),
                title: Some(format!("Capitulo {}", chapter.number)),
                chapter_number: chapter.number.parse::<f32>().ok(),
                date_uploaded: chapter.timestamp,
                language: Some("pt-BR".to_string()),
                url: Some(format!("{BASE_URL}/{}/{}", manga_id, chapter.number)),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    serde_json::from_str::<PageDto>(body)
        .unwrap_or_default()
        .pages
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
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
        .with_header("Accept", "*/*")
        .with_header("Accept-Language", "pt-BR,en-US;q=0.9,en;q=0.8")
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

fn search_url(page: u64, query: &str, sort_by: &str, sort_order: &str) -> String {
    let mut params = vec![
        ("page", page.to_string()),
        ("limit", "24".to_string()),
        ("sortBy", sort_by.to_string()),
        ("sortOrder", sort_order.to_string()),
    ];
    if !query.trim().is_empty() {
        params.push(("q", query.to_string()));
    }
    format!("{API_URL}/mangas/search?{}", query_string(&params))
}

fn details_url(key: &str) -> String {
    format!("{API_URL}/manga-by-slug/{}", url::query_escape(&normalize_key(key)))
}

fn pages_url(key: &str) -> String {
    let (manga_id, chapter_id) = key.split_once('#').unwrap_or((key, ""));
    format!(
        "{API_URL}/mangas/{}/chapters/{}",
        url::query_escape(manga_id),
        url::query_escape(chapter_id)
    )
}

fn query_string(params: &[(&str, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn image_headers() -> BTreeMap<String, String> {
    BTreeMap::from([("Referer".to_string(), format!("{BASE_URL}/"))])
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn filter_value(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(|value| match value {
            Value::String(value) => Some(value.clone()),
            Value::Object(object) => object
                .get("value")
                .or_else(|| object.get("id"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            _ => None,
        })
}

const LIST_FIXTURE: &str = r#"{"mangas":[{"id":"sample","title":"Sample Manga Livre","coverUrl":"https://toonlivre.net/cover.jpg","genres":["Action"],"status":"ongoing"}],"pagination":{"hasNextPage":false}}"#;
const DETAILS_FIXTURE: &str = r#"{"id":"sample","title":"Sample Manga Livre","coverUrl":"https://toonlivre.net/cover.jpg","authors":["Author"],"artists":["Artist"],"genres":["Action"],"description":"Description","alternativeTitle":"Sample Alt","status":"ongoing","chapters":[{"id":"chapter-1","number":"1","timestamp":1704067200}]}"#;
const PAGES_FIXTURE: &str = r#"{"pages":["https://toonlivre.net/page-1.jpg","https://toonlivre.net/page-2.jpg"]}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_manga_livre_fixtures() {
        assert_eq!(parse_wrapper(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(SOURCE.pages(json!({"chapter":"sample#chapter-1"})).unwrap().len(), 2);
    }
}
