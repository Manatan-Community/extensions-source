use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    html, manga,
    sdk::{FilterValue, SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: TaimuMangas = TaimuMangas;
const BASE_URL: &str = "https://beta.taimumangas.com";
const API_URL: &str = "https://apiv2.taimumangas.com/api/v1/reader";
const PAGE_SIZE: u64 = 24;
const CHAPTER_PAGE_SIZE: u64 = 100;

struct TaimuMangas;

impl MangaSource for TaimuMangas {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_library(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_updates(&fetch_api_or_fixture(
                &updates_path(page),
                UPDATES_FIXTURE,
            )));
        }
        Ok(parse_library(&fetch_api_or_fixture(
            &library_path(page, "", &ParsedFilters::popular()),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_library(&fetch_api_or_fixture(
            &library_path(page, query, &parse_filters(request.get("filters"))),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let identifier = normalize_key(&key);
        let mut chapters = Vec::new();
        let mut page = 1;
        loop {
            let payload = serde_json::from_str::<ChapterListResponse>(&fetch_api_or_fixture(
                &chapters_path(&identifier, page),
                CHAPTERS_FIXTURE,
            ))
            .unwrap_or_default();
            chapters.extend(payload.items.into_iter().map(ChapterSummary::into_chapter));
            if !payload.has_more {
                break;
            }
            page = payload.page + 1;
            if page > 50 {
                break;
            }
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "chapter-1".into());
        Ok(parse_pages(&fetch_api_or_fixture(
            &chapter_path(&normalize_key(&key)),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/series/{}", normalize_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/reader/{}", normalize_key(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/series/") {
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
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn library_path(page: u64, query: &str, filters: &ParsedFilters) -> String {
    let mut params = vec![
        ("page", page.to_string()),
        ("per_page", PAGE_SIZE.to_string()),
        ("adult", "true".to_string()),
    ];
    if !query.is_empty() {
        params.push(("q", url::query_escape(query)));
    }
    if !filters.sort.is_empty() {
        params.push(("sort", url::query_escape(&filters.sort)));
    }
    if !filters.order.is_empty() {
        params.push(("order", url::query_escape(&filters.order)));
    }
    if !filters.status.is_empty() {
        params.push(("status", url::query_escape(&filters.status)));
    }
    if !filters.kind.is_empty() {
        params.push(("type", url::query_escape(&filters.kind)));
    }
    if !filters.genres.is_empty() {
        params.push(("genres", filters.genres.join(",")));
    }
    format!(
        "/library?{}",
        params
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn updates_path(page: u64) -> String {
    format!("/updates?page={page}&per_page={PAGE_SIZE}&adult_mode=true")
}

fn chapters_path(identifier: &str, page: u64) -> String {
    format!("/series/{identifier}/chapters?page={page}&per_page={CHAPTER_PAGE_SIZE}&order=desc")
}

fn chapter_path(identifier: &str) -> String {
    format!("/chapters/{identifier}?adult=true")
}

#[derive(Default)]
struct ParsedFilters {
    status: String,
    kind: String,
    sort: String,
    order: String,
    genres: Vec<String>,
}

impl ParsedFilters {
    fn popular() -> Self {
        Self {
            sort: "rating".to_string(),
            order: "desc".to_string(),
            ..Self::default()
        }
    }
}

fn parse_filters(filters: Option<&Value>) -> ParsedFilters {
    let mut parsed = ParsedFilters {
        sort: "updated".to_string(),
        order: "desc".to_string(),
        ..ParsedFilters::default()
    };
    for filter in filters_to_values(filters) {
        match filter.id.as_str() {
            "status" => parsed.status = string_value(&filter.value),
            "type" => parsed.kind = string_value(&filter.value),
            "sort" => parsed.sort = string_value(&filter.value),
            "order" => parsed.order = string_value(&filter.value),
            "genres" => {
                parsed.genres = string_values(&filter.value)
                    .into_iter()
                    .map(|value| url::query_escape(&value))
                    .collect()
            }
            _ => {}
        }
    }
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

fn string_values(value: &Value) -> Vec<String> {
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    string_value(value)
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_library(body: &str) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<LibraryResponse>(body).unwrap_or_default();
    Paged {
        entries: payload
            .items
            .into_iter()
            .map(SeriesSummary::into_catalog)
            .collect(),
        has_next_page: payload.page * payload.per_page < payload.total,
    }
}

fn parse_updates(body: &str) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<UpdatesResponse>(body).unwrap_or_default();
    Paged {
        entries: payload
            .items
            .into_iter()
            .map(UpdateSummary::into_catalog)
            .collect(),
        has_next_page: payload.has_more,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    serde_json::from_str::<SeriesDetail>(&fetch_api_or_fixture(
        &format!("/series/{}", normalize_key(key)),
        DETAILS_FIXTURE,
    ))
    .unwrap_or_default()
    .into_catalog()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    serde_json::from_str::<ChapterDetailResponse>(body)
        .unwrap_or_default()
        .pages
        .into_iter()
        .map(PageInfo::into_page)
        .collect()
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("sample")
        .to_string()
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "finished" => ItemStatus::Completed,
        "hiatus" => ItemStatus::Hiatus,
        "dropped" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: Option<&str>) -> Option<i64> {
    value
        .and_then(|date| date.split(['T', ' ']).next())
        .and_then(manatan_shared::dates::parse_ymd)
}

#[derive(Default, Deserialize)]
struct LibraryResponse {
    #[serde(default)]
    items: Vec<SeriesSummary>,
    #[serde(default)]
    page: u64,
    #[serde(default, rename = "per_page")]
    per_page: u64,
    #[serde(default)]
    total: u64,
}

#[derive(Default, Deserialize)]
struct SeriesSummary {
    identifier: String,
    title: String,
    cover: Option<String>,
    status: Option<String>,
}

impl SeriesSummary {
    fn into_catalog(self) -> CatalogItem {
        CatalogItem {
            key: self.identifier.clone(),
            title: self.title,
            cover: self.cover,
            status: parse_status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/series/{}", self.identifier)),
            language: Some("pt-BR".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct UpdatesResponse {
    #[serde(default)]
    items: Vec<UpdateSummary>,
    #[serde(default, rename = "has_more")]
    has_more: bool,
}

#[derive(Default, Deserialize)]
struct UpdateSummary {
    #[serde(rename = "series_identifier")]
    series_identifier: String,
    #[serde(rename = "series_title")]
    series_title: String,
    #[serde(rename = "series_cover")]
    series_cover: Option<String>,
}

impl UpdateSummary {
    fn into_catalog(self) -> CatalogItem {
        CatalogItem {
            key: self.series_identifier.clone(),
            title: self.series_title,
            cover: self.series_cover,
            url: Some(format!("{BASE_URL}/series/{}", self.series_identifier)),
            language: Some("pt-BR".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct SeriesDetail {
    identifier: String,
    title: String,
    #[serde(default)]
    adult: bool,
    #[serde(default)]
    artists: Vec<NameId>,
    #[serde(default)]
    authors: Vec<NameId>,
    cover: Option<String>,
    #[serde(default)]
    genres: Vec<Genre>,
    group: Option<GroupInfo>,
    status: Option<String>,
    synopsis: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

impl SeriesDetail {
    fn into_catalog(self) -> CatalogItem {
        let mut description = String::new();
        if let Some(synopsis) = self.synopsis.filter(|value| !value.trim().is_empty()) {
            description.push_str(&html::strip_tags(&synopsis));
        }
        if let Some(kind) = self.kind.filter(|value| !value.trim().is_empty()) {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str(&format!("Tipo: {kind}"));
        }
        if let Some(group) = self
            .group
            .and_then(|group| (!group.name.trim().is_empty()).then_some(group.name))
        {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str(&format!("Scanlator: {group}"));
        }
        if self.adult {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str("Conteúdo adulto");
        }
        CatalogItem {
            key: self.identifier.clone(),
            title: self.title,
            cover: self.cover,
            description: (!description.is_empty()).then_some(description),
            authors: self
                .authors
                .into_iter()
                .map(|author| author.name)
                .filter(|value| !value.is_empty())
                .collect(),
            artists: self
                .artists
                .into_iter()
                .map(|artist| artist.name)
                .filter(|value| !value.is_empty())
                .collect(),
            tags: self
                .genres
                .into_iter()
                .map(|genre| genre.name)
                .filter(|value| !value.is_empty())
                .collect(),
            status: parse_status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/series/{}", self.identifier)),
            language: Some("pt-BR".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct NameId {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct GroupInfo {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct Genre {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct ChapterListResponse {
    #[serde(default)]
    items: Vec<ChapterSummary>,
    #[serde(default, rename = "has_more")]
    has_more: bool,
    #[serde(default)]
    page: u64,
}

#[derive(Default, Deserialize)]
struct ChapterSummary {
    identifier: String,
    number: Value,
    #[serde(rename = "published_at")]
    published_at: Option<String>,
}

impl ChapterSummary {
    fn into_chapter(self) -> MangaChapter {
        let number = json_number_text(&self.number);
        MangaChapter {
            key: self.identifier.clone(),
            title: Some(format!("Capitulo {number}")),
            chapter_number: number.parse::<f32>().ok(),
            date_uploaded: parse_date(self.published_at.as_deref()),
            url: Some(format!("{BASE_URL}/reader/{}", self.identifier)),
            ..MangaChapter::default()
        }
    }
}

fn json_number_text(value: &Value) -> String {
    if let Some(number) = value.as_f64() {
        let text = number.to_string();
        return text.trim_end_matches(".0").to_string();
    }
    value
        .as_str()
        .unwrap_or_default()
        .trim_end_matches(".0")
        .to_string()
}

#[derive(Default, Deserialize)]
struct ChapterDetailResponse {
    #[serde(default)]
    pages: Vec<PageInfo>,
}

#[derive(Deserialize)]
struct PageInfo {
    url: String,
    #[serde(default)]
    number: u32,
}

impl PageInfo {
    fn into_page(self) -> MangaPage {
        MangaPage {
            content: PageContent::Url {
                url: self.url,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", self.number)),
            ..MangaPage::default()
        }
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"items":[{"identifier":"sample","title":"Sample Taimu","cover":"https://beta.taimumangas.com/cover.jpg","status":"ongoing"}],"page":1,"per_page":24,"total":25}"#;
const UPDATES_FIXTURE: &str = r#"{"items":[{"series_identifier":"sample","series_title":"Sample Taimu","series_cover":"https://beta.taimumangas.com/cover.jpg"}],"has_more":false}"#;
const DETAILS_FIXTURE: &str = r#"{"identifier":"sample","title":"Sample Taimu","adult":true,"artists":[{"name":"Artist"}],"authors":[{"name":"Author"}],"cover":"https://beta.taimumangas.com/cover.jpg","genres":[{"name":"Ação"}],"group":{"name":"Taimu"},"status":"ongoing","synopsis":"Summary","type":"manhwa"}"#;
const CHAPTERS_FIXTURE: &str = r#"{"items":[{"identifier":"chapter-1","number":"1","published_at":"2024-01-01T00:00:00.000Z"}],"has_more":false,"page":1}"#;
const PAGES_FIXTURE: &str = r#"{"pages":[{"url":"https://beta.taimumangas.com/page1.jpg","number":1},{"url":"https://beta.taimumangas.com/page2.jpg","number":2}]}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_taimu_fixtures() {
        let list = SOURCE.list(json!({})).unwrap();
        assert_eq!(list.entries[0].title, "Sample Taimu");
        assert!(list.has_next_page);

        let details = serde_json::from_str::<SeriesDetail>(DETAILS_FIXTURE)
            .unwrap()
            .into_catalog();
        assert_eq!(details.title, "Sample Taimu");
        assert_eq!(details.status, ItemStatus::Ongoing);

        let chapters = serde_json::from_str::<ChapterListResponse>(CHAPTERS_FIXTURE)
            .unwrap()
            .items
            .into_iter()
            .map(ChapterSummary::into_chapter)
            .collect::<Vec<_>>();
        assert_eq!(chapters[0].key, "chapter-1");

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
