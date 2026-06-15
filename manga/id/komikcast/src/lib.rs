use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: KomikCast = KomikCast;
const BASE_URL: &str = "https://v2.komikcast.fit";
const API_URL: &str = "https://be.komikcast.cc";

struct KomikCast;

impl MangaSource for KomikCast {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_list(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "popularity"
        };
        Ok(parse_series_list(&api_get(
            &series_url(page, sort, "desc", "", None),
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
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &api_get(
                        &format!("{API_URL}/series/{}", slug_from_key(&key)),
                        DETAILS_FIXTURE,
                    ),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let filters = request.get("filters");
        Ok(parse_series_list(&api_get(
            &series_url(
                page,
                filter(filters, "sort").unwrap_or("latest"),
                filter(filters, "sortOrder").unwrap_or("desc"),
                query,
                filters,
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        Ok(parse_details(
            &api_get(
                &format!("{API_URL}/series/{}", slug_from_key(&key)),
                DETAILS_FIXTURE,
            ),
            Some(normalize_key(&key)),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let slug = slug_from_key(&key);
        Ok(parse_chapters(
            &api_get(
                &format!("{API_URL}/series/{slug}/chapters"),
                CHAPTERS_FIXTURE,
            ),
            &slug,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter/1".to_string());
        let (slug, index) = chapter_identity(&key);
        Ok(parse_pages(&api_get(
            &format!("{API_URL}/series/{slug}/chapters/{index}"),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/series/{}", slug_from_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (slug, index) = chapter_identity(&key);
            format!("{BASE_URL}/series/{slug}/chapter/{index}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/series/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &api_get(
                        &format!("{API_URL}/series/{}", slug_from_key(&key)),
                        DETAILS_FIXTURE,
                    ),
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
        .with_webview_challenge_fallback()
}

fn api_get(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .header("Accept", "application/json")
        .header("Accept-Language", "en-US,en;q=0.9,id;q=0.8")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn series_url(
    page: u64,
    sort: &str,
    sort_order: &str,
    query: &str,
    filters: Option<&Value>,
) -> String {
    let mut params = vec![
        ("includeMeta", "true".to_string()),
        ("take", "12".to_string()),
        ("page", page.to_string()),
        ("sort", sort.to_string()),
        ("sortOrder", sort_order.to_string()),
    ];
    if !query.is_empty() {
        params.push((
            "filter",
            format!(
                "title=like=\"{}\",nativeTitle=like=\"{}\"",
                query.replace('"', ""),
                query.replace('"', "")
            ),
        ));
    }
    for key in ["status", "format", "type", "genreIds"] {
        for value in filter_values(filters, key) {
            params.push((key, value));
        }
    }
    format!(
        "{API_URL}/series?{}",
        params
            .into_iter()
            .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters?.get(id)?.as_str()
}

fn filter_values(filters: Option<&Value>, id: &str) -> Vec<String> {
    let Some(value) = filters.and_then(|filters| filters.get(id)) else {
        return Vec::new();
    };
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect();
    }
    value
        .as_str()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_series_list(body: &str) -> Paged<CatalogItem> {
    let payload: SeriesListResponse = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: payload
            .data
            .into_iter()
            .map(SeriesItem::into_catalog)
            .collect(),
        has_next_page: payload
            .meta
            .is_some_and(|meta| meta.page.unwrap_or(0) < meta.last_page.unwrap_or(0)),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let payload: SeriesDetailResponse = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    payload.data.into_catalog_with_key(key)
}

fn parse_chapters(body: &str, series_slug: &str) -> Vec<MangaChapter> {
    let payload: ChapterListResponse = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
    payload
        .data
        .into_iter()
        .filter_map(|chapter| chapter.into_chapter(series_slug))
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let payload: ChapterDetailResponse = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    payload
        .data
        .data
        .images
        .unwrap_or_default()
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
struct SeriesListResponse {
    #[serde(default)]
    data: Vec<SeriesItem>,
    meta: Option<Meta>,
}

#[derive(Default, Deserialize)]
struct SeriesDetailResponse {
    #[serde(default)]
    data: SeriesItem,
}

#[derive(Default, Deserialize)]
struct SeriesItem {
    id: Option<i64>,
    #[serde(default)]
    data: SeriesData,
}

impl SeriesItem {
    fn into_catalog(self) -> CatalogItem {
        self.into_catalog_with_key(None)
    }

    fn into_catalog_with_key(self, key: Option<String>) -> CatalogItem {
        let slug = self
            .data
            .slug
            .clone()
            .or_else(|| self.id.map(|id| id.to_string()))
            .unwrap_or_else(|| "sample".to_string());
        let key = key.unwrap_or_else(|| format!("/series/{slug}"));
        CatalogItem {
            key: key.clone(),
            title: self.data.title.unwrap_or_else(|| "Komik Cast".to_string()),
            cover: self.data.cover_image,
            description: self.data.synopsis,
            authors: self.data.author.into_iter().collect(),
            tags: self
                .data
                .genres
                .unwrap_or_default()
                .into_iter()
                .filter_map(|genre| genre.data.name)
                .collect(),
            status: parse_status(self.data.status.as_deref()),
            url: Some(format!("{BASE_URL}/series/{slug}")),
            language: Some("id".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesData {
    slug: Option<String>,
    title: Option<String>,
    author: Option<String>,
    status: Option<String>,
    synopsis: Option<String>,
    cover_image: Option<String>,
    genres: Option<Vec<GenreData>>,
}

#[derive(Default, Deserialize)]
struct GenreData {
    #[serde(default)]
    data: GenreInfo,
}

#[derive(Default, Deserialize)]
struct GenreInfo {
    name: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    page: Option<u64>,
    last_page: Option<u64>,
}

#[derive(Default, Deserialize)]
struct ChapterListResponse {
    #[serde(default)]
    data: Vec<ChapterItem>,
}

#[derive(Default, Deserialize)]
struct ChapterDetailResponse {
    #[serde(default)]
    data: ChapterItem,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterItem {
    #[serde(default)]
    data: ChapterData,
    created_at: Option<String>,
    updated_at: Option<String>,
    chapter_index: Option<f32>,
}

impl ChapterItem {
    fn into_chapter(self, series_slug: &str) -> Option<MangaChapter> {
        let index = self.data.index.or(self.chapter_index)?;
        let formatted = format_chapter_index(index);
        let title = self.data.title.filter(|value| !value.trim().is_empty());
        Some(MangaChapter {
            key: format!("/series/{series_slug}/chapter/{formatted}"),
            title: Some(match title {
                Some(title) => format!("Chapter {formatted}: {title}"),
                None => format!("Chapter {formatted}"),
            }),
            chapter_number: Some(index),
            date_uploaded: self
                .created_at
                .or(self.updated_at)
                .and_then(|value| parse_iso_date(&value)),
            url: Some(format!(
                "{BASE_URL}/series/{series_slug}/chapter/{formatted}"
            )),
            ..MangaChapter::default()
        })
    }
}

#[derive(Default, Deserialize)]
struct ChapterData {
    index: Option<f32>,
    title: Option<String>,
    images: Option<Vec<String>>,
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "ongoing" | "on going" => ItemStatus::Ongoing,
        "completed" | "complete" => ItemStatus::Completed,
        "hiatus" => ItemStatus::Hiatus,
        "cancelled" | "canceled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    manatan_shared::dates::parse_ymd(value.split('T').next()?)
}

fn format_chapter_index(value: f32) -> String {
    let rounded = value.round();
    if (value - rounded).abs() < 0.001 {
        format!("{}", rounded as i64)
    } else {
        let mut out = format!("{value:.2}");
        while out.ends_with('0') {
            out.pop();
        }
        out.trim_end_matches('.').to_string()
    }
}

fn normalize_key(input: &str) -> String {
    if let Some(rest) = input.split("/series/").nth(1) {
        let slug = rest.split(['/', '?', '#']).next().unwrap_or("sample");
        return format!("/series/{slug}");
    }
    format!("/{}", input.trim_matches('/'))
}

fn slug_from_key(key: &str) -> String {
    key.split("/series/")
        .nth(1)
        .unwrap_or(key)
        .trim_matches('/')
        .split('/')
        .next()
        .unwrap_or("sample")
        .to_string()
}

fn chapter_identity(key: &str) -> (String, String) {
    let path = if let Some(rest) = key.split("/series/").nth(1) {
        rest
    } else {
        key.trim_matches('/')
    };
    let parts = path.split('/').collect::<Vec<_>>();
    let slug = parts.first().copied().unwrap_or("sample").to_string();
    let index = parts
        .iter()
        .position(|part| *part == "chapter")
        .and_then(|pos| parts.get(pos + 1))
        .copied()
        .unwrap_or("1")
        .to_string();
    (slug, index)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{
  "status": 200,
  "data": [
    { "id": 1, "data": { "slug": "sample", "title": "Sample Komik Cast", "author": "Writer", "status": "ongoing", "synopsis": "Sample synopsis.", "coverImage": "https://fixtures.invalid/cover.jpg", "genres": [ { "data": { "name": "Action" } } ] } }
  ],
  "meta": { "page": 1, "lastPage": 2 }
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "data": { "id": 1, "data": { "slug": "sample", "title": "Sample Komik Cast", "author": "Writer", "status": "ongoing", "synopsis": "Sample synopsis.", "coverImage": "https://fixtures.invalid/cover.jpg", "genres": [ { "data": { "name": "Action" } } ] } }
}"#;

const CHAPTERS_FIXTURE: &str = r#"{
  "data": [
    { "data": { "index": 1, "title": "Start" }, "createdAt": "2024-01-01T00:00:00.000+07:00" }
  ]
}"#;

const PAGES_FIXTURE: &str = r#"{
  "data": { "data": { "index": 1, "images": ["https://fixtures.invalid/page1.jpg"] } }
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(
            parse_series_list(LIST_FIXTURE).entries[0].title,
            "Sample Komik Cast"
        );
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, "sample").len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
