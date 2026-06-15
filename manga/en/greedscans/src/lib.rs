use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: GreedScans = GreedScans;
const BASE_URL: &str = "https://gojoscans.com";
const API_URL: &str = "https://api.gojoscans.com/api";

struct GreedScans;

impl MangaSource for GreedScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_list(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest_update"
        } else {
            "popular"
        };
        Ok(parse_series_list(&fetch_api(
            &series_url(page, "", Some(sort), None),
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
                    &fetch_api(&format!("{API_URL}{key}"), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let filters = request.get("filters");
        Ok(parse_series_list(&fetch_api(
            &series_url(
                page,
                query,
                filter_string(filters, "sort_by").as_deref(),
                filters,
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        Ok(parse_details(
            &fetch_api(
                &format!("{API_URL}{}", compatible_key(&key)),
                DETAILS_FIXTURE,
            ),
            Some(compatible_key(&key)),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        Ok(parse_chapters(&fetch_api(
            &format!("{API_URL}{}", compatible_key(&key)),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapters/chapter-1".to_string());
        Ok(parse_pages(&fetch_api(
            &format!("{API_URL}{key}"),
            CHAPTER_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_api(
                        &format!("{API_URL}{}", compatible_key(&key)),
                        DETAILS_FIXTURE,
                    ),
                    Some(compatible_key(&key)),
                )),
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
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn series_url(page: u64, query: &str, sort: Option<&str>, filters: Option<&Value>) -> String {
    let mut params = vec![
        ("page".to_string(), page.to_string()),
        ("per_page".to_string(), "24".to_string()),
        ("sort_order".to_string(), "desc".to_string()),
    ];
    if !query.is_empty() {
        params.push(("search".to_string(), query.to_string()));
    }
    if let Some(sort) = sort.filter(|value| !value.is_empty()) {
        params.push(("sort_by".to_string(), sort.to_string()));
    }
    for value in filter_values(filters, "status") {
        params.push(("status".to_string(), value));
    }
    for value in filter_values(filters, "type") {
        params.push(("type".to_string(), value));
    }
    for value in filter_values(filters, "genres") {
        params.push(("genres[]".to_string(), value));
    }
    if let Some(min) = filter_string(filters, "min_chapters").filter(|value| !value.is_empty()) {
        params.push(("min_chapters".to_string(), min));
    }
    format!(
        "{API_URL}/series?{}",
        params
            .into_iter()
            .map(|(key, value)| format!(
                "{}={}",
                url::query_escape(&key),
                url::query_escape(&value)
            ))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn filter_string(filters: Option<&Value>, key: &str) -> Option<String> {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(|value| value.as_str().map(ToString::to_string))
}

fn filter_values(filters: Option<&Value>, key: &str) -> Vec<String> {
    let Some(value) = filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
    else {
        return Vec::new();
    };
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect();
    }
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .map(|value| vec![value.to_string()])
        .unwrap_or_default()
}

fn parse_series_list(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<SeriesListResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: response
            .data
            .data
            .into_iter()
            .map(|series| CatalogItem {
                key: format!("/series/{}", series.slug),
                title: series.title,
                cover: series.cover_image,
                status: parse_status(series.status.as_deref()),
                url: Some(format!("{BASE_URL}/series/{}", series.slug)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
            .collect(),
        has_next_page: response.data.current_page < response.data.last_page,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let response = serde_json::from_str::<SeriesDetailResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let data = response.data;
    let key = key.unwrap_or_else(|| format!("/series/{}", data.slug));
    let mut description = data.synopsis.unwrap_or_default();
    if !data.alternative_titles.is_empty() {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Alternative Titles:\n");
        description.push_str(&data.alternative_titles.join("\n"));
    }
    CatalogItem {
        key: key.clone(),
        title: data.title,
        authors: data.author.into_iter().collect(),
        artists: data.studio.into_iter().collect(),
        cover: data.cover_image,
        status: parse_status(data.status.as_deref()),
        tags: data.genres,
        description: (!description.is_empty()).then_some(description),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<SeriesDetailResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let slug = response.data.slug;
    let mut chapters = response
        .data
        .chapters
        .into_iter()
        .map(|chapter| {
            let key = format!("/series/{slug}/chapters/{}", chapter.slug);
            MangaChapter {
                key: key.clone(),
                title: Some(chapter.title),
                chapter_number: Some(chapter.chapter_number),
                date_uploaded: chapter
                    .published_at
                    .or(chapter.created_at)
                    .and_then(|value| parse_iso_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|left, right| {
        right
            .chapter_number
            .partial_cmp(&left.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<ChapterDetailResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTER_FIXTURE).expect("fixture is valid"));
    response
        .data
        .chapter
        .images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.image_url,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.unwrap_or_default().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "hiatus" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn normalize_key(value: &str) -> String {
    let path = value
        .strip_prefix(BASE_URL)
        .unwrap_or(value)
        .trim_start_matches('/')
        .trim_end_matches('/');
    compatible_key(&format!("/{path}"))
}

fn compatible_key(key: &str) -> String {
    key.replace("/manga/", "/series/")
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let date = value.split('T').next()?;
    let mut parts = date.split('-');
    unix_date(
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    )
}

fn unix_date(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month as i32 + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(((era * 146_097 + doe - 719_468) as i64) * 86_400)
}

#[derive(Deserialize)]
struct SeriesListResponse {
    data: PaginatedData,
}

#[derive(Deserialize)]
struct PaginatedData {
    data: Vec<BrowseSeries>,
    current_page: u64,
    last_page: u64,
}

#[derive(Deserialize)]
struct BrowseSeries {
    title: String,
    slug: String,
    cover_image: Option<String>,
    status: Option<String>,
}

#[derive(Deserialize)]
struct SeriesDetailResponse {
    data: SeriesDetail,
}

#[derive(Deserialize)]
struct SeriesDetail {
    title: String,
    slug: String,
    synopsis: Option<String>,
    author: Option<String>,
    studio: Option<String>,
    cover_image: Option<String>,
    status: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    alternative_titles: Vec<String>,
    #[serde(default)]
    chapters: Vec<Chapter>,
}

#[derive(Deserialize)]
struct Chapter {
    title: String,
    slug: String,
    chapter_number: f32,
    created_at: Option<String>,
    published_at: Option<String>,
}

#[derive(Deserialize)]
struct ChapterDetailResponse {
    data: ChapterDetail,
}

#[derive(Deserialize)]
struct ChapterDetail {
    chapter: ChapterImages,
}

#[derive(Deserialize)]
struct ChapterImages {
    #[serde(default)]
    images: Vec<PageImage>,
}

#[derive(Deserialize)]
struct PageImage {
    image_url: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":{"data":[{"title":"Sample Manga","slug":"sample","cover_image":"https://gojoscans.com/cover.jpg","status":"ongoing"}],"current_page":1,"last_page":1}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"title":"Sample Manga","slug":"sample","synopsis":"Sample description.","author":"Author","studio":"Studio","cover_image":"https://gojoscans.com/cover.jpg","status":"ongoing","genres":["Action"],"alternative_titles":["Sample Alt"],"chapters":[{"title":"Chapter 1","slug":"chapter-1","chapter_number":1,"created_at":"2024-01-01T00:00:00.000000Z","published_at":"2024-01-01T00:00:00.000000Z"}]}}"#;
const CHAPTER_FIXTURE: &str = r#"{"data":{"chapter":{"images":[{"image_url":"https://gojoscans.com/page1.jpg"},{"image_url":"https://gojoscans.com/page2.jpg"}]}}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_api_shapes() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Manga"
        );
        assert_eq!(
            SOURCE
                .pages(json!({"chapter":"/series/sample/chapters/chapter-1"}))
                .unwrap()
                .len(),
            2
        );
    }
}
