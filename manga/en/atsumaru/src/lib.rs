use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Atsumaru = Atsumaru;
const BASE_URL: &str = "https://atsu.moe";

struct Atsumaru;

impl MangaSource for Atsumaru {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_browse(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let page_index = page.saturating_sub(1);
        let listing = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "recentlyUpdated"
        } else {
            "trending"
        };
        let adult = adult_query(&request);
        Ok(parse_browse(&fetch_api_or_fixture(
            &format!(
                "/api/infinite/{listing}?page={page_index}&types=Manga,Manwha,Manhua,OEL{adult}"
            ),
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
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let path = search_path(page, query, request.get("filters"), adult_enabled(&request));
        Ok(parse_search(&fetch_api_or_fixture(&path, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let mut scanlators = std::collections::BTreeMap::new();
        if let Ok(details) = serde_json::from_str::<MangaObject>(&fetch_api_or_fixture(
            &format!("/api/manga/page?id={key}"),
            DETAILS_FIXTURE,
        )) {
            for scanlator in details.manga_page.scanlators.unwrap_or_default() {
                scanlators.insert(scanlator.id, scanlator.name);
            }
        }
        let body = fetch_api_or_fixture(
            &format!("/api/manga/allChapters?mangaId={}", url::query_escape(&key)),
            CHAPTERS_FIXTURE,
        );
        let payload: AllChapters = serde_json::from_str(&body).unwrap_or_default();
        let mut chapters = payload
            .chapters
            .into_iter()
            .map(|chapter| {
                let scanlator = scanlators.get(chapter_scanlator_id(&chapter)).cloned();
                chapter.into_chapter(&key, scanlator)
            })
            .collect::<Vec<_>>();
        chapters.sort_by(|left, right| {
            right
                .chapter_number
                .partial_cmp(&left.chapter_number)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.scanlators.cmp(&right.scanlators))
                .then_with(|| right.date_uploaded.cmp(&left.date_uploaded))
        });
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "sample/chapter-1".to_string());
        let (manga_id, chapter_id) = key.split_once('/').unwrap_or(("sample", "chapter-1"));
        let body = fetch_api_or_fixture(
            &format!(
                "/api/read/chapter?mangaId={}&chapterId={}",
                url::query_escape(manga_id),
                url::query_escape(chapter_id)
            ),
            PAGES_FIXTURE,
        );
        let payload: PageObject = serde_json::from_str(&body).unwrap_or_default();
        Ok(payload
            .read_chapter
            .pages
            .into_iter()
            .enumerate()
            .map(|(index, page)| {
                let image = normalize_image(&page.image);
                MangaPage {
                    content: PageContent::Url {
                        url: image.clone(),
                        context: Some(manga::image_headers(BASE_URL)),
                    },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(format!("Page {}", index + 1)),
                    ..MangaPage::default()
                }
            })
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/manga/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (slug, name) = key.split_once('/').unwrap_or((&key, ""));
            format!("{BASE_URL}/read/{slug}/{name}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
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
        .with_referer(BASE_URL)
        .with_header("Accept", "*/*")
        .with_header("Content-Type", "application/json")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api_or_fixture(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{BASE_URL}{path}"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_browse(body: &str) -> Paged<CatalogItem> {
    let payload: BrowseManga = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: payload
            .items
            .into_iter()
            .map(MangaDto::into_catalog)
            .collect(),
        has_next_page: true,
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    if body.contains("\"hits\"") {
        let payload: SearchResults = serde_json::from_str(body).unwrap_or_default();
        let has_next_page = payload.page * payload.request_params.per_page < payload.found;
        Paged {
            entries: payload
                .hits
                .into_iter()
                .map(|hit| hit.document.into_catalog())
                .collect(),
            has_next_page,
        }
    } else {
        parse_browse(body)
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_api_or_fixture(
        &format!("/api/manga/page?id={}", url::query_escape(key)),
        DETAILS_FIXTURE,
    );
    serde_json::from_str::<MangaObject>(&body)
        .map(|payload| payload.manga_page.into_catalog_initialized())
        .unwrap_or_else(|_| fallback_catalog(key))
}

fn search_path(page: u64, query: &str, filters: Option<&Value>, adult: bool) -> String {
    let mut filter_by = vec!["hidden:!=true".to_string()];
    if !adult {
        filter_by.push("isAdult:=false".to_string());
    }
    if let Some(filters) = filters.and_then(Value::as_object) {
        if let Some(types) = filters.get("types").and_then(Value::as_array) {
            let values = types
                .iter()
                .filter_map(Value::as_str)
                .map(|value| format!("`{value}`"))
                .collect::<Vec<_>>();
            if !values.is_empty() {
                filter_by.push(format!("type:=[{}]", values.join(",")));
            }
        }
        if let Some(statuses) = filters.get("statuses").and_then(Value::as_array) {
            let values = statuses
                .iter()
                .filter_map(Value::as_str)
                .map(|value| format!("`{value}`"))
                .collect::<Vec<_>>();
            if !values.is_empty() {
                filter_by.push(format!("status:=[{}]", values.join(",")));
            }
        }
        if let Some(year) = filters.get("year").and_then(Value::as_i64) {
            filter_by.push(format!("releaseYear:=[{year}]"));
        }
        if let Some(min_chapters) = filters.get("minChapters").and_then(Value::as_i64) {
            filter_by.push(format!("chapterCount:>={min_chapters}"));
        }
    }
    filter_by.push(
        "(mbContentRating:=[`Safe`,`Suggestive`,`Erotica`] || mbContentRating:!=*)".to_string(),
    );
    filter_by.push("views:>0".to_string());

    let mut path = format!(
        "/collections/manga/documents/search?q={}&filter_by={}&page={page}&per_page=40",
        url::query_escape(if query.is_empty() { "*" } else { query }),
        url::query_escape(&filter_by.join(" && "))
    );
    if !query.is_empty() {
        path.push_str("&query_by=title,englishTitle,otherNames,authors&query_by_weights=4,3,2,1&num_typos=4,3,2,1");
    }
    path
}

fn adult_enabled(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("pref_18_mode").or_else(|| prefs.get("showAdult")))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn adult_query(request: &Value) -> &'static str {
    if adult_enabled(request) {
        "&adult=1"
    } else {
        ""
    }
}

fn normalize_key(value: &str) -> String {
    value
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(value)
        .to_string()
}

fn normalize_image(value: &str) -> String {
    let raw = value.trim_start_matches('/').trim_start_matches("static/");
    let image = if value.starts_with("http") {
        value.to_string()
    } else if value.starts_with("//") {
        format!("https:{value}")
    } else {
        format!("{BASE_URL}/static/{raw}")
    };
    image
        .replacen("http://", "https://", 1)
        .replacen("https:///", "https://", 1)
}

fn names_from_value(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text.clone()),
                Value::Object(object) => object
                    .get("name")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn authors_from_value(value: Option<&Value>, role: Option<&str>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(text) if role.is_none() => Some(text.clone()),
                Value::Object(object) => {
                    let name = object.get("name").and_then(Value::as_str)?;
                    let kind = object.get("type").and_then(Value::as_str);
                    if role.is_none() || kind == role {
                        Some(name.to_string())
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn epoch_millis_to_year(millis: i64) -> i64 {
    let days = millis / 1000 / 86400 + 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += (month <= 2) as i64;
    year
}

fn parse_date_value(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64().map(|value| {
            if value > 10_000_000_000 {
                value / 1000
            } else {
                value
            }
        }),
        Value::String(text) => parse_iso_date(text),
        _ => None,
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let cleaned = value.replace("T ", "T").replace('Z', "");
    let (date, time) = cleaned.split_once('T')?;
    let d = date
        .split('-')
        .filter_map(|part| part.parse::<i64>().ok())
        .collect::<Vec<_>>();
    let t = time
        .split(':')
        .filter_map(|part| part.split('.').next()?.parse::<i64>().ok())
        .collect::<Vec<_>>();
    if d.len() != 3 || t.len() < 2 {
        return None;
    }
    Some(timestamp_utc(
        d[0],
        d[1],
        d[2],
        t[0],
        t[1],
        *t.get(2).unwrap_or(&0),
    ))
}

fn timestamp_utc(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> i64 {
    let y = year - (month <= 2) as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86400 + hour * 3600 + minute * 60 + second
}

fn fallback_catalog(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: key.replace('-', " "),
        url: Some(format!("{BASE_URL}/manga/{key}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn chapter_scanlator_id(chapter: &ChapterDto) -> &str {
    chapter.scanlation_manga_id.as_deref().unwrap_or_default()
}

#[derive(Default, Deserialize)]
struct BrowseManga {
    #[serde(default)]
    items: Vec<MangaDto>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaObject {
    manga_page: MangaDto,
}

#[derive(Default, Deserialize)]
struct SearchResults {
    #[serde(default)]
    page: u64,
    #[serde(default)]
    found: u64,
    #[serde(default)]
    hits: Vec<SearchHit>,
    #[serde(default, rename = "request_params")]
    request_params: RequestParams,
}

#[derive(Default, Deserialize)]
struct SearchHit {
    document: MangaDto,
}

#[derive(Default, Deserialize)]
struct RequestParams {
    #[serde(default, rename = "per_page")]
    per_page: u64,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaDto {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default, alias = "poster", alias = "image")]
    image_path: Value,
    authors: Option<Value>,
    synopsis: Option<String>,
    #[serde(alias = "genres", alias = "tags")]
    genres: Option<Value>,
    released: Option<i64>,
    status: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    views: Option<Value>,
    other_names: Option<Vec<String>>,
    avg_rating: Option<f32>,
    scanlators: Option<Vec<Scanlator>>,
}

impl MangaDto {
    fn image_path(&self) -> Option<String> {
        match &self.image_path {
            Value::String(text) => Some(text.clone()),
            Value::Object(object) => object
                .get("image")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            _ => None,
        }
    }

    fn into_catalog(self) -> CatalogItem {
        let cover = self.image_path().map(|image| normalize_image(&image));
        let year = self
            .released
            .filter(|value| *value > 0)
            .map(epoch_millis_to_year);
        let mut description = Vec::new();
        if let Some(rating) = self.avg_rating.filter(|value| *value > 0.0) {
            description.push(format!("Rating: {rating:.2}/10"));
        }
        if let Some(year) = year {
            description.push(format!("Year: {year}"));
        }
        if let Some(views) = self.views.as_ref().and_then(Value::as_str) {
            description.push(format!("Views: {views}"));
        } else if let Some(views) = self.views.as_ref().and_then(Value::as_i64) {
            description.push(format!("Views: {views}"));
        }
        if let Some(synopsis) = self.synopsis.filter(|value| !value.is_empty()) {
            description.push(format!("Synopsis: {}", html::strip_tags(&synopsis)));
        }
        if let Some(names) = self.other_names.filter(|names| !names.is_empty()) {
            description.push(format!("Alternative Names:\n- {}", names.join("\n- ")));
        }

        let mut tags = Vec::new();
        tags.extend(self.kind);
        tags.extend(names_from_value(self.genres.as_ref()));
        let authors = authors_from_value(self.authors.as_ref(), Some("Author"));
        let artists = authors_from_value(self.authors.as_ref(), Some("Artist"));
        let authors = if authors.is_empty() {
            authors_from_value(self.authors.as_ref(), None)
        } else {
            authors
        };
        CatalogItem {
            key: self.id.clone(),
            title: self.title,
            cover,
            authors,
            artists,
            description: (!description.is_empty()).then(|| description.join("\n\n")),
            tags,
            url: Some(format!("{BASE_URL}/manga/{}", self.id)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            status: match self
                .status
                .as_deref()
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("ongoing") => ItemStatus::Ongoing,
                Some("completed") => ItemStatus::Completed,
                Some("hiatus") => ItemStatus::Hiatus,
                Some("canceled") | Some("cancelled") => ItemStatus::Cancelled,
                _ => ItemStatus::Unknown,
            },
            ..CatalogItem::default()
        }
    }

    fn into_catalog_initialized(self) -> CatalogItem {
        CatalogItem {
            initialized: true,
            ..self.into_catalog()
        }
    }
}

#[derive(Default, Deserialize)]
struct Scanlator {
    id: String,
    name: String,
}

#[derive(Default, Deserialize)]
struct AllChapters {
    #[serde(default)]
    chapters: Vec<ChapterDto>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterDto {
    #[serde(default)]
    id: String,
    #[serde(default)]
    number: f32,
    #[serde(default)]
    title: String,
    scanlation_manga_id: Option<String>,
    created_at: Option<Value>,
}

impl ChapterDto {
    fn into_chapter(self, manga_id: &str, scanlator: Option<String>) -> MangaChapter {
        MangaChapter {
            key: format!("{manga_id}/{}", self.id),
            title: Some(self.title),
            chapter_number: Some(self.number),
            scanlators: scanlator.into_iter().collect(),
            date_uploaded: self.created_at.as_ref().and_then(parse_date_value),
            url: Some(format!("{BASE_URL}/read/{manga_id}/{}", self.id)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageObject {
    read_chapter: PageList,
}

#[derive(Default, Deserialize)]
struct PageList {
    #[serde(default)]
    pages: Vec<PageDto>,
}

#[derive(Default, Deserialize)]
struct PageDto {
    #[serde(default)]
    image: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
{ "items": [{ "id": "sample", "title": "Sample Atsumaru", "poster": "/covers/sample.jpg", "status": "Ongoing", "type": "Manga" }] }
"#;

const SEARCH_FIXTURE: &str = r#"
{ "page": 1, "found": 1, "request_params": { "per_page": 40 }, "hits": [{ "document": { "id": "sample", "title": "Sample Atsumaru", "poster": "/covers/sample.jpg" } }] }
"#;

const DETAILS_FIXTURE: &str = r#"
{ "mangaPage": { "id": "sample", "title": "Sample Atsumaru", "poster": "/covers/sample.jpg", "authors": ["Author"], "synopsis": "A sample.", "genres": ["Action"], "status": "Ongoing", "scanlators": [{ "id": "scan", "name": "Scanlator" }] } }
"#;

const CHAPTERS_FIXTURE: &str = r#"
{ "chapters": [{ "id": "chapter-1", "number": 1, "title": "Chapter 1", "scanlationMangaId": "scan", "createdAt": "2024-01-01T00:00:00.000Z" }] }
"#;

const PAGES_FIXTURE: &str = r#"
{ "readChapter": { "pages": [{ "image": "/pages/001.jpg" }] } }
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fixture_api() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample Atsumaru");
        let chapters = SOURCE.chapters(json!({"manga": "sample"})).unwrap();
        assert_eq!(chapters[0].scanlators, vec!["Scanlator"]);
    }
}
