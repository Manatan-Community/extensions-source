use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    manga,
    sdk::{FilterValue, SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: DreamTeamsScans = DreamTeamsScans;
const BASE_URL: &str = "https://dreamteams.space";
const API_URL: &str = "https://api.dreamteams.space/api";
const LIMIT: u64 = 20;

struct DreamTeamsScans;

impl MangaSource for DreamTeamsScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        Ok(parse_list(&fetch_api_or_fixture(
            &search_path(page, "", sort, &[]),
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
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let filters = parse_filters(request.get("filters"));
        Ok(parse_list(&fetch_api_or_fixture(
            &search_path(page, query, &filters.sort, &filters.params()),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let details = serde_json::from_str::<MangaDetailsDto>(&fetch_api_or_fixture(
            &format!("/series/comic{}", normalize_key(&key)),
            DETAILS_FIXTURE,
        ))
        .unwrap_or_default();
        Ok(details
            .units
            .into_iter()
            .map(|chapter| chapter.into_chapter(&details.slug))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comic/sample/chapter/chapter-1".into());
        Ok(parse_pages(&fetch_api_or_fixture(
            &format!("/series{}", normalize_chapter_key(&key)),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| {
            format!(
                "{}/comic{}",
                BASE_URL.trim_end_matches('/'),
                normalize_key(&key)
            )
        }))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}{}", normalize_chapter_key(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/comic/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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
        .with_webview_challenge_fallback()
}

fn fetch_api_or_fixture(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{API_URL}{path}"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_path(page: u64, query: &str, sort: &str, extra: &[(&str, String)]) -> String {
    let mut params = vec![
        ("type", "COMIC".to_string()),
        ("limit", LIMIT.to_string()),
        ("page", page.to_string()),
        ("sort", sort.to_string()),
        ("order", "desc".to_string()),
    ];
    if !query.is_empty() {
        params.push(("q", url::query_escape(query)));
    }
    for (key, value) in extra {
        if !value.is_empty() {
            params.push((key, value.clone()));
        }
    }
    format!(
        "/search?{}",
        params
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&")
    )
}

#[derive(Default)]
struct ParsedFilters {
    sort: String,
    genre: String,
    status: String,
    comic_type: String,
    color_format: String,
    reading_format: String,
}

impl ParsedFilters {
    fn params(&self) -> Vec<(&str, String)> {
        vec![
            ("genre", self.genre.clone()),
            ("status", self.status.clone()),
            ("comic_type", self.comic_type.clone()),
            ("color_format", self.color_format.clone()),
            ("reading_format", self.reading_format.clone()),
        ]
    }
}

fn parse_filters(filters: Option<&Value>) -> ParsedFilters {
    let mut parsed = ParsedFilters {
        sort: "popular".to_string(),
        ..ParsedFilters::default()
    };
    for filter in filters_to_values(filters) {
        let value = string_value(&filter.value);
        match filter.id.as_str() {
            "sort" if !value.is_empty() => parsed.sort = value,
            "genre" => parsed.genre = url::query_escape(&value),
            "status" => parsed.status = url::query_escape(&value),
            "comic_type" => parsed.comic_type = url::query_escape(&value),
            "color_format" => parsed.color_format = url::query_escape(&value),
            "reading_format" => parsed.reading_format = url::query_escape(&value),
            _ => {}
        }
    }
    parsed.sort = url::query_escape(&parsed.sort);
    parsed
}

fn filters_to_values(filters: Option<&Value>) -> Vec<FilterValue> {
    let Some(filters) = filters else {
        return Vec::new();
    };
    if let Ok(values) = serde_json::from_value::<Vec<FilterValue>>(filters.clone()) {
        return values;
    }
    filters
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(id, value)| FilterValue {
                    id: id.clone(),
                    value: value.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string_value(value: &Value) -> String {
    value.as_str().unwrap_or_default().trim().to_string()
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<MangaListDto>(body).unwrap_or_default();
    Paged {
        entries: payload
            .data
            .into_iter()
            .map(MangaDto::into_catalog)
            .collect(),
        has_next_page: payload.page < payload.total_pages,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    serde_json::from_str::<MangaDetailsDto>(&fetch_api_or_fixture(
        &format!("/series/comic{}", normalize_key(key)),
        DETAILS_FIXTURE,
    ))
    .unwrap_or_default()
    .into_catalog()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    serde_json::from_str::<PageListDto>(body)
        .unwrap_or_default()
        .chapter
        .pages
        .into_iter()
        .map(|page| MangaPage {
            content: PageContent::Url {
                url: page.image_url,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", page.page_number)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_key(input: &str) -> String {
    let value = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .strip_prefix("/comic/")
        .unwrap_or(input)
        .trim_matches('/');
    format!("/{}", value.split('/').next().unwrap_or("sample"))
}

fn normalize_chapter_key(input: &str) -> String {
    if input.starts_with("/comic/") {
        return input.to_string();
    }
    format!("/comic/{}", input.trim_matches('/'))
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_uppercase().as_str() {
        "ONGOING" => ItemStatus::Ongoing,
        "COMPLETED" => ItemStatus::Completed,
        "HIATUS" => ItemStatus::Hiatus,
        "CANCELLED" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: Option<&str>) -> Option<i64> {
    let date = value?.split(['T', ' ']).next()?;
    manatan_shared::dates::parse_ymd(date)
}

#[derive(Debug, Default, Deserialize)]
struct MangaListDto {
    #[serde(default)]
    data: Vec<MangaDto>,
    #[serde(default)]
    page: u64,
    #[serde(default)]
    total_pages: u64,
}

#[derive(Debug, Default, Deserialize)]
struct MangaDto {
    title: String,
    slug: String,
    poster_image_url: Option<String>,
}

impl MangaDto {
    fn into_catalog(self) -> CatalogItem {
        let key = format!("/{}", self.slug);
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: self.poster_image_url,
            url: Some(format!("{BASE_URL}/comic{}", key)),
            language: Some("id".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct MangaDetailsDto {
    title: String,
    slug: String,
    synopsis: Option<String>,
    poster_image_url: Option<String>,
    author_name: Option<String>,
    artist_name: Option<String>,
    comic_status: Option<String>,
    primary_genre: Option<String>,
    #[serde(default)]
    genres: Vec<GenreDto>,
    #[serde(default)]
    units: Vec<ChapterDto>,
}

impl MangaDetailsDto {
    fn into_catalog(self) -> CatalogItem {
        let key = format!("/{}", self.slug);
        let tags = self
            .primary_genre
            .into_iter()
            .chain(self.genres.into_iter().map(|genre| genre.name))
            .collect::<Vec<_>>();
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: self.poster_image_url,
            description: self.synopsis,
            authors: self.author_name.into_iter().collect(),
            artists: self.artist_name.into_iter().collect(),
            tags,
            status: parse_status(self.comic_status.as_deref()),
            url: Some(format!("{BASE_URL}/comic{}", key)),
            language: Some("id".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct GenreDto {
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct ChapterDto {
    number: String,
    slug: String,
    title: Option<String>,
    created_at: Option<String>,
}

impl ChapterDto {
    fn into_chapter(self, manga_slug: &str) -> MangaChapter {
        let number = self.number.trim_end_matches(".00").to_string();
        let title = if let Some(extra) = self.title.filter(|title| !title.trim().is_empty()) {
            format!("Chapter {number} - {}", extra.trim())
        } else {
            format!("Chapter {number}")
        };
        let key = format!("/comic/{manga_slug}/chapter/{}", self.slug);
        MangaChapter {
            key: key.clone(),
            title: Some(title),
            chapter_number: number.parse::<f32>().ok(),
            date_uploaded: parse_date(self.created_at.as_deref()),
            url: Some(format!("{BASE_URL}{key}")),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct PageListDto {
    #[serde(default)]
    chapter: ChapterPageDto,
}

#[derive(Debug, Default, Deserialize)]
struct ChapterPageDto {
    #[serde(default)]
    pages: Vec<PageDto>,
}

#[derive(Debug, Deserialize)]
struct PageDto {
    page_number: u32,
    image_url: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":[{"title":"Sample Dream","slug":"sample","poster_image_url":"https://img.example/cover.jpg"}],"page":1,"total_pages":2}"#;
const DETAILS_FIXTURE: &str = r#"{"title":"Sample Dream","slug":"sample","synopsis":"Summary","poster_image_url":"https://img.example/cover.jpg","author_name":"Author","artist_name":"Artist","comic_status":"ONGOING","primary_genre":"Action","genres":[{"name":"Romance"}],"units":[{"number":"1.00","slug":"chapter-1","title":"Start","created_at":"2024-01-01T00:00:00.000Z"}]}"#;
const PAGES_FIXTURE: &str = r#"{"chapter":{"pages":[{"page_number":1,"image_url":"https://img.example/page1.jpg"},{"page_number":2,"image_url":"https://img.example/page2.jpg"}]}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_dreamteams_api() {
        let list = SOURCE.list(json!({})).unwrap();
        assert_eq!(list.entries[0].title, "Sample Dream");
        assert!(list.has_next_page);
        assert_eq!(
            SOURCE.chapters(json!({"manga":"/sample"})).unwrap().len(),
            1
        );
        assert_eq!(
            SOURCE
                .pages(json!({"chapter":"/comic/sample/chapter/chapter-1"}))
                .unwrap()
                .len(),
            2
        );
    }
}
