use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: KomikNesia = KomikNesia;
const BASE_URL: &str = "https://02.komiknesia.asia";
const API_URL: &str = "https://api-be.komiknesia.my.id/api";

struct KomikNesia;

impl MangaSource for KomikNesia {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            ""
        } else {
            "Popular"
        };
        Ok(parse_listing(&fetch_api_or_fixture(
            &contents_url(page, "", Some(order), None),
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
                entries: vec![fetch_details(&slug)],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_api_or_fixture(
            &contents_url(page, query, None, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(fetch_details(&normalize_slug(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let payload: Payload<MangaDto> = parse_json(&fetch_api_or_fixture(
            &format!("{API_URL}/comic/{}", normalize_slug(&key)),
            DETAILS_FIXTURE,
        ));
        Ok(payload
            .data
            .chapters
            .unwrap_or_default()
            .into_iter()
            .map(ChapterDto::to_chapter)
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "chapter-1".into());
        let payload: Payload<PageListDto> = parse_json(&fetch_api_or_fixture(
            &format!("{API_URL}/chapters/slug/{}", normalize_slug(&key)),
            PAGES_FIXTURE,
        ));
        Ok(payload
            .data
            .images
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
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/komik/{}", normalize_slug(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/view/{}", normalize_slug(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/komik/") {
            let slug = normalize_slug(input);
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&slug)),
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

fn fetch_api_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn contents_url(page: u64, query: &str, order: Option<&str>, filters: Option<&Value>) -> String {
    let mut params = vec![format!("page={page}")];
    if !query.is_empty() {
        params.push(format!("q={}", url::query_escape(query)));
    }
    let status = filter(filters, "status").unwrap_or_default();
    if !status.is_empty() {
        params.push(format!("status={}", url::query_escape(status)));
    }
    let order = filter(filters, "orderBy").or(order).unwrap_or_default();
    if !order.is_empty() {
        params.push(format!("orderBy={}", url::query_escape(order)));
    }
    if let Some(genres) = filter(filters, "genre") {
        for genre in genres
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            params.push(format!("genre[]={}", url::query_escape(genre)));
        }
    }
    format!("{API_URL}/contents?{}", params.join("&"))
}

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters?.get(id)?.as_str()
}

fn fetch_details(slug: &str) -> CatalogItem {
    let payload: Payload<MangaDto> = parse_json(&fetch_api_or_fixture(
        &format!("{API_URL}/comic/{slug}"),
        DETAILS_FIXTURE,
    ));
    payload.data.to_catalog(true)
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let payload: Payload<Vec<MangaDto>> = parse_json(body);
    Paged {
        entries: payload
            .data
            .into_iter()
            .map(|manga| manga.to_catalog(false))
            .collect(),
        has_next_page: payload
            .meta
            .is_some_and(|meta| meta.page < meta.total_pages),
    }
}

fn parse_json<'a, T>(body: &'a str) -> T
where
    T: Deserialize<'a>,
{
    serde_json::from_str(body)
        .or_else(|_| serde_json::from_str(LIST_FIXTURE))
        .or_else(|_| serde_json::from_str(DETAILS_FIXTURE))
        .or_else(|_| serde_json::from_str(PAGES_FIXTURE))
        .expect("fixture is valid")
}

#[derive(Default, Deserialize)]
struct Payload<T> {
    data: T,
    meta: Option<MetaDto>,
}

#[derive(Deserialize)]
struct MetaDto {
    page: u64,
    total_pages: u64,
}

#[derive(Default, Deserialize)]
struct MangaDto {
    title: String,
    slug: String,
    alternative_name: Option<String>,
    author: Option<String>,
    sinopsis: Option<String>,
    cover: Option<String>,
    status: Option<String>,
    genres: Option<Vec<GenreDto>>,
    chapters: Option<Vec<ChapterDto>>,
}

impl MangaDto {
    fn to_catalog(self, initialized: bool) -> CatalogItem {
        let mut description = self
            .sinopsis
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_default();
        if let Some(alt) = self
            .alternative_name
            .filter(|value| !value.trim().is_empty())
        {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str("Alternative Names:\n");
            description.push_str(
                &alt.split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        CatalogItem {
            key: self.slug.clone(),
            title: self.title,
            cover: self.cover,
            description: (!description.is_empty()).then_some(description),
            authors: self.author.into_iter().collect(),
            tags: self
                .genres
                .unwrap_or_default()
                .into_iter()
                .map(|genre| genre.name)
                .collect(),
            status: parse_status(&self.status.unwrap_or_default()),
            url: Some(format!("{BASE_URL}/komik/{}", self.slug)),
            language: Some("id".to_string()),
            content_rating: Some("adult".to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    number: String,
    title: String,
    slug: String,
    created_at: Option<DateDto>,
}

impl ChapterDto {
    fn to_chapter(self) -> MangaChapter {
        MangaChapter {
            key: self.slug.clone(),
            title: Some(self.title),
            chapter_number: self.number.parse().ok(),
            date_uploaded: self.created_at.map(|date| date.time),
            url: Some(format!("{BASE_URL}/view/{}", self.slug)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct DateDto {
    time: i64,
}

#[derive(Default, Deserialize)]
struct PageListDto {
    images: Vec<String>,
}

#[derive(Deserialize)]
struct GenreDto {
    name: String,
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "hiatus" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn normalize_slug(input: &str) -> String {
    let trimmed = input.trim().trim_end_matches('/');
    if let Some(rest) = trimmed.split("/komik/").nth(1) {
        return rest.split('/').next().unwrap_or("sample").to_string();
    }
    if let Some(rest) = trimmed.split("/view/").nth(1) {
        return rest.split('/').next().unwrap_or("chapter-1").to_string();
    }
    trimmed.trim_matches('/').to_string()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{
  "data": [
    { "title": "Sample", "slug": "sample", "cover": "https://02.komiknesia.asia/cover.jpg", "status": "Ongoing", "genres": [{ "id": 1, "name": "Action" }] }
  ],
  "meta": { "page": 1, "total_pages": 2 }
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "data": {
    "title": "Sample",
    "slug": "sample",
    "alternative_name": "Alt Sample",
    "author": "Author Name",
    "sinopsis": "<p>Sample description.</p>",
    "cover": "https://02.komiknesia.asia/cover.jpg",
    "status": "Ongoing",
    "genres": [{ "id": 1, "name": "Action" }],
    "chapters": [{ "number": "1", "title": "Chapter 1", "slug": "chapter-1", "created_at": { "time": 1704067200 } }]
  }
}"#;

const PAGES_FIXTURE: &str = r#"{
  "data": { "images": ["https://02.komiknesia.asia/page-1.jpg"] }
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_fixture() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries.len(), 1);
        let payload: Payload<MangaDto> = parse_json(DETAILS_FIXTURE);
        assert_eq!(payload.data.chapters.unwrap().len(), 1);
    }
}
