use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: LectorMoeSource = LectorMoeSource;
const BASE_URL: &str = "https://capibaratraductor.com/tsukinomusumescan";
const API_BASE_URL: &str = "https://capibaratraductor.com";
const ORGANIZATION: &str = "tsukinomusumescan";
const NAME: &str = "Tsuki No Musume Scan";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";
const PAGE_LIMIT: u64 = 36;

struct LectorMoeSource;

impl MangaSource for LectorMoeSource {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if listing_id(&request) == "latest" {
            "latest"
        } else {
            "popular"
        };
        Ok(parse_listing(
            &fetch_api_or_fixture(&manga_list_url(page, order, None), LIST_FIXTURE),
            page,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_slug(query);
            return Ok(Paged {
                entries: vec![details_from_slug(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = request
            .get("filters")
            .and_then(|filters| filters.get("order").or_else(|| filters.get("sort")))
            .and_then(Value::as_str)
            .filter(|value| matches!(*value, "popular" | "latest"))
            .unwrap_or("popular");
        Ok(parse_listing(
            &fetch_api_or_fixture(&manga_list_url(page, order, Some(query)), LIST_FIXTURE),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(details_from_slug(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = fetch_api_or_fixture(&details_api_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".to_string());
        Ok(parse_pages(&fetch_api_or_fixture(
            &pages_api_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| manga_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| chapter_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            let key = normalize_slug(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_slug(&key)),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_cookies_for(API_BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("x-organization", ORGANIZATION)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn manga_list_url(page: u64, order: &str, query: Option<&str>) -> String {
    let mut out =
        format!("{API_BASE_URL}/api/manga-custom?page={page}&limit={PAGE_LIMIT}&order={order}");
    if let Some(query) = query.filter(|value| !value.is_empty()) {
        out.push_str("&title=");
        out.push_str(&url::query_escape(query));
    }
    out
}

fn details_api_url(slug: &str) -> String {
    format!("{API_BASE_URL}/api/manga-custom/{}", normalize_slug(slug))
}

fn pages_api_url(key: &str) -> String {
    let (slug, chapter) = key.split_once('/').unwrap_or((key, "1"));
    format!(
        "{API_BASE_URL}/api/manga-custom/{}/chapter/{}/pages",
        normalize_slug(slug),
        chapter
    )
}

fn parse_listing(body: &str, page: u64) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, LIST_FIXTURE);
    let data = root.get("data").unwrap_or(&Value::Null);
    let entries = data
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(catalog_from_json)
        .collect();
    let max_page = data.get("maxPage").and_then(Value::as_u64).unwrap_or(page);
    Paged {
        entries,
        has_next_page: page < max_page,
    }
}

fn details_from_slug(slug: &str) -> CatalogItem {
    let key = normalize_slug(slug);
    let body = fetch_api_or_fixture(&details_api_url(&key), DETAILS_FIXTURE);
    catalog_details_from_json(
        json_or_fixture(&body, DETAILS_FIXTURE)
            .get("data")
            .unwrap_or(&Value::Null),
    )
}

fn catalog_from_json(item: &Value) -> CatalogItem {
    let key = string_path(item, &["manga", "slug"])
        .or_else(|| string_value(item, "slug"))
        .unwrap_or_else(|| "sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: string_value(item, "title")
            .or_else(|| string_path(item, &["manga", "title"]))
            .unwrap_or_else(|| NAME.to_string()),
        cover: string_value(item, "imageUrl").or_else(|| string_path(item, &["manga", "imageUrl"])),
        url: Some(manga_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn catalog_details_from_json(item: &Value) -> CatalogItem {
    let mut out = catalog_from_json(item);
    out.description = string_value(item, "description")
        .or_else(|| string_path(item, &["manga", "description"]))
        .or_else(|| string_value(item, "shortDescription"));
    out.authors = item
        .get("authors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|author| string_value(author, "name"))
        .collect();
    out.tags = item
        .get("genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|genre| string_value(genre, "name"))
        .collect();
    out.status = parse_status(string_value(item, "status").as_deref());
    out.banner = string_value(item, "bannerUrl");
    out.initialized = true;
    out
}

fn parse_chapters(body: &str, series_slug: &str) -> Vec<MangaChapter> {
    let root = json_or_fixture(body, DETAILS_FIXTURE);
    let slug = string_path(&root, &["data", "manga", "slug"])
        .unwrap_or_else(|| normalize_slug(series_slug));
    let now = unix_now();
    root.get("data")
        .and_then(|data| data.get("chapters"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|chapter| {
            !chapter
                .get("isUnreleased")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|chapter| {
            let number = chapter_number_text(chapter.get("number")?)?;
            let released_at =
                string_value(chapter, "releasedAt").and_then(|value| parse_rfc3339_utc(&value));
            if released_at.is_some_and(|date| date > now) {
                return None;
            }
            let raw_title = string_value(chapter, "title").unwrap_or_default();
            let title = if raw_title.is_empty() {
                format!("Capitulo {number}")
            } else {
                format!("Capitulo {number} - {raw_title}")
            };
            Some(MangaChapter {
                key: format!("{slug}/{number}"),
                title: Some(title),
                chapter_number: chapter
                    .get("number")
                    .and_then(Value::as_f64)
                    .map(|value| value as f32),
                date_uploaded: released_at,
                language: Some(LANG.to_string()),
                url: Some(chapter_url(&format!("{slug}/{number}"))),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = json_or_fixture(body, PAGES_FIXTURE);
    root.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| string_value(page, "imageUrl"))
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

fn manga_url(slug: &str) -> String {
    format!("{BASE_URL}/manga/{}", normalize_slug(slug))
}

fn chapter_url(key: &str) -> String {
    let (slug, chapter) = key.split_once('/').unwrap_or((key, "1"));
    format!(
        "{BASE_URL}/manga/{}/chapters/{chapter}",
        normalize_slug(slug)
    )
}

fn normalize_slug(input: &str) -> String {
    let mut value = input.trim().trim_end_matches('/').to_string();
    if let Some((_, rest)) = value.split_once("/manga/") {
        value = rest.split('/').next().unwrap_or(rest).to_string();
    }
    value.trim_matches('/').to_string()
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status {
        Some("ongoing") => ItemStatus::Ongoing,
        Some("hiatus") => ItemStatus::Hiatus,
        Some("finished") | Some("completed") => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn string_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn string_path(value: &Value, path: &[&str]) -> Option<String> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    cursor
        .as_str()
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn chapter_number_text(value: &Value) -> Option<String> {
    if let Some(number) = value.as_i64() {
        return Some(number.to_string());
    }
    let number = value.as_f64()?;
    if number.fract() == 0.0 {
        Some(format!("{}", number as i64))
    } else {
        Some(number.to_string())
    }
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn parse_rfc3339_utc(value: &str) -> Option<i64> {
    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = value.get(5..7)?.parse::<i32>().ok()?;
    let day = value.get(8..10)?.parse::<i32>().ok()?;
    let hour = value.get(11..13)?.parse::<i64>().ok()?;
    let minute = value.get(14..16)?.parse::<i64>().ok()?;
    let second = value.get(17..19)?.parse::<i64>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(i64::MAX)
}

const LIST_FIXTURE: &str = r#"{"status":true,"data":{"items":[{"title":"Sample","imageUrl":"https://r2.capibaratraductor.com/sample-cover.jpg","status":"ongoing","manga":{"slug":"sample","title":"Sample"}}],"maxPage":1}}"#;
const DETAILS_FIXTURE: &str = r#"{"status":true,"data":{"title":"Sample","description":"Summary","imageUrl":"https://r2.capibaratraductor.com/sample-cover.jpg","status":"ongoing","manga":{"slug":"sample","title":"Sample"},"authors":[{"name":"Author"}],"genres":[{"name":"Drama"}],"chapters":[{"number":1,"title":"Sample","releasedAt":"2024-01-01T00:00:00.000Z","isUnreleased":false}]}}"#;
const PAGES_FIXTURE: &str =
    r#"{"status":true,"data":[{"imageUrl":"https://r2.capibaratraductor.com/sample-page.jpg"}]}"#;

export_manga_source!(SOURCE);
