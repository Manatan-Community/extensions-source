use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Alandal = Alandal;
const BASE_URL: &str = "https://alandal.com";
const API_URL: &str = "https://qq.alandal.com/api";

struct Alandal;

impl MangaSource for Alandal {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_search(SEARCH_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "new"
        } else {
            "popular"
        };
        Ok(parse_search(&fetch_api_or_fixture(
            &format!("/series?type=comic&sort={sort}&genres=-1&status=-1&page={page}"),
            SEARCH_FIXTURE,
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
            let body = fetch_api_or_fixture(&format!("{key}?type=comic"), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, key)],
                has_next_page: false,
            });
        }
        let mut path = format!("/series?type=comic&genres=-1&status=-1&page={page}");
        if !query.is_empty() {
            path.push_str("&name=");
            path.push_str(&url::query_escape(query));
        }
        append_filters(&mut path, request.get("filters"));
        Ok(parse_search(&fetch_api_or_fixture(&path, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let body = fetch_api_or_fixture(&format!("{key}?type=comic"), DETAILS_FIXTURE);
        Ok(parse_details(&body, key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let body = fetch_api_or_fixture(
            &format!("{key}/chapters?type=comic&from=0&to=999"),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapters/1".to_string());
        if request
            .get("title")
            .and_then(Value::as_str)
            .is_some_and(|title| title.starts_with("[LOCKED]"))
        {
            return Ok(Vec::new());
        }
        let body = fetch_api_or_fixture(&format!("{key}?type=comic&traveler=0"), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_api_or_fixture(&format!("{key}?type=comic"), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, key)),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_origin(BASE_URL)
        .with_header("Accept", "application/json")
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

fn append_filters(path: &mut String, filters: Option<&Value>) {
    let Some(filters) = filters.and_then(Value::as_object) else {
        return;
    };
    if let Some(sort) = filters.get("sort").and_then(Value::as_str) {
        path.push_str("&sort=");
        path.push_str(&url::query_escape(sort));
    }
    if let Some(status) = filters.get("status").and_then(Value::as_str) {
        path.push_str("&status=");
        path.push_str(&url::query_escape(status));
    }
    if let Some(genres) = filters.get("genres").and_then(Value::as_array) {
        for genre in genres.iter().filter_map(Value::as_i64) {
            path.push_str("&genres=");
            path.push_str(&genre.to_string());
        }
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let response: ResponseDto<SearchSeriesDto> = serde_json::from_str(body).unwrap_or_default();
    let series = response.data.series;
    Paged {
        has_next_page: series.current_page < series.last_page,
        entries: series
            .data
            .into_iter()
            .map(SearchEntryDto::into_catalog)
            .collect(),
    }
}

fn parse_details(body: &str, key: String) -> CatalogItem {
    let response: ResponseDto<MangaDetailsDto> = serde_json::from_str(body).unwrap_or_default();
    let details = response.data.series;
    CatalogItem {
        key: key.clone(),
        title: details.name,
        cover: Some(details.cover),
        description: Some(html::strip_tags(&details.summary)),
        tags: details.genres.into_iter().map(|item| item.name).collect(),
        authors: details
            .creators
            .iter()
            .filter(|creator| creator.kind.as_deref() == Some("author"))
            .map(|creator| creator.name.clone())
            .collect(),
        status: match details.status.name.to_ascii_lowercase().as_str() {
            "ongoing" => ItemStatus::Ongoing,
            "completed" => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!(
            "{BASE_URL}{}",
            key.replace("series/", "series/comic-")
        )),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, series_key: &str) -> Vec<MangaChapter> {
    let response: ChapterResponseDto = serde_json::from_str(body).unwrap_or_default();
    response
        .data
        .into_iter()
        .rev()
        .map(|chapter| {
            let prefix = if chapter.access { "" } else { "[LOCKED] " };
            let key = format!("{series_key}/chapters/{}", chapter.name);
            MangaChapter {
                key: key.clone(),
                title: Some(format!("{prefix}Chapter {}", chapter.name)),
                date_uploaded: manatan_shared::dates::parse_fixture_date(&chapter.published),
                url: Some(format!(
                    "{BASE_URL}{}",
                    key.replace("series/", "chapter/comic-")
                        .replace("chapters/", "")
                )),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    root.pointer("/data/chapter/chapter/pages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(ToString::to_string))
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

fn normalize_key(input: &str) -> String {
    let path = input.trim_start_matches(BASE_URL).trim_matches('/');
    let normalized = path.replace("series/comic-", "series/");
    format!("/{normalized}")
}

#[derive(Default, Deserialize)]
struct ResponseDto<T: Default> {
    #[serde(default)]
    data: ResultDto<T>,
}

#[derive(Default, Deserialize)]
struct ResultDto<T: Default> {
    #[serde(default)]
    series: T,
}

#[derive(Default, Deserialize)]
struct SearchSeriesDto {
    current_page: i32,
    last_page: i32,
    #[serde(default)]
    data: Vec<SearchEntryDto>,
}

#[derive(Default, Deserialize)]
struct SearchEntryDto {
    name: String,
    slug: String,
    cover: String,
}

impl SearchEntryDto {
    fn into_catalog(self) -> CatalogItem {
        let key = format!("/series/{}", self.slug);
        CatalogItem {
            key: key.clone(),
            title: self.name,
            cover: Some(self.cover),
            url: Some(format!(
                "{BASE_URL}{}",
                key.replace("series/", "series/comic-")
            )),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct MangaDetailsDto {
    name: String,
    summary: String,
    status: NamedObject,
    #[serde(default)]
    genres: Vec<NamedObject>,
    #[serde(default)]
    creators: Vec<NamedObject>,
    cover: String,
}

#[derive(Default, Deserialize)]
struct NamedObject {
    name: String,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Default, Deserialize)]
struct ChapterResponseDto {
    #[serde(default)]
    data: Vec<ChapterDto>,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    name: String,
    #[serde(rename = "published_at")]
    published: String,
    access: bool,
}

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"
{"data":{"series":{"current_page":1,"last_page":2,"data":[{"name":"Alandal Sample","slug":"sample","cover":"https://img/cover.jpg"}]}}}
"#;

const DETAILS_FIXTURE: &str = r#"
{"data":{"series":{"name":"Alandal Sample","summary":"<p>Summary</p>","status":{"name":"ongoing"},"genres":[{"name":"Action"}],"creators":[{"name":"Author","type":"author"}],"cover":"https://img/cover.jpg"}}}
"#;

const CHAPTERS_FIXTURE: &str = r#"
{"data":[{"name":"1","published_at":"2024-01-01T00:00:00.000000Z","access":true},{"name":"2","published_at":"2024-01-02T00:00:00.000000Z","access":false}]}
"#;

const PAGES_FIXTURE: &str = r#"
{"data":{"chapter":{"chapter":{"pages":["https://img/page1.jpg","https://img/page2.jpg"]}}}}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_source() {
        assert!(parse_search(SEARCH_FIXTURE).has_next_page);
        assert_eq!(
            parse_details(DETAILS_FIXTURE, "/series/sample".into()).authors[0],
            "Author"
        );
        assert!(
            parse_chapters(CHAPTERS_FIXTURE, "/series/sample")[0]
                .title
                .as_deref()
                .unwrap()
                .starts_with("[LOCKED]")
        );
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
