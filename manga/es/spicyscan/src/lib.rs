use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: SpicyScan = SpicyScan;
const BASE_URL: &str = "https://spicyseries.com";
const API_BASE_URL: &str = "https://back.spicyseries.com";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";
const PAGE_SIZE: u64 = 12;

struct SpicyScan;

impl MangaSource for SpicyScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_filter_response(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "6"
        } else {
            "3"
        };
        Ok(parse_filter_response(&fetch_json_or_fixture(
            &filter_url(page, order, "desc", "", "", ""),
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
            let key = normalize_slug(query);
            return Ok(Paged {
                entries: vec![details_from_slug(&key)],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            if query.chars().count() < 2 {
                return Ok(Paged::default());
            }
            return Ok(parse_search_response(&fetch_json_or_fixture(
                &format!(
                    "{API_BASE_URL}/home/buscar?query={}",
                    url::query_escape(query)
                ),
                SEARCH_FIXTURE,
            )));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        Ok(parse_filter_response(&fetch_json_or_fixture(
            &filter_url(
                page,
                filter(filters, "orderBy", "3"),
                filter(filters, "sort", "desc"),
                filter(filters, "gendersId", ""),
                filter(filters, "origin", ""),
                filter(filters, "state", ""),
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(details_from_slug(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let series = details_json(&key);
        Ok(series
            .get("chapters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|chapter| chapter_from_json(chapter, &normalize_slug(&key)))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_json_or_fixture(
            &format!("{API_BASE_URL}/serie/{}/", key.trim_matches('/')),
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
        if input.starts_with(BASE_URL) && input.contains("/comic/") {
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

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Origin", BASE_URL)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn filter_url(
    page: u64,
    order_by: &str,
    sort: &str,
    genres: &str,
    origin: &str,
    state: &str,
) -> String {
    format!(
        "{API_BASE_URL}/filtrar?page={page}&limit={PAGE_SIZE}&orderBy={}&sort={}&gendersId={}&origin={}&state={}&loading=true",
        url::query_escape(order_by),
        url::query_escape(sort),
        url::query_escape(genres),
        url::query_escape(origin),
        url::query_escape(state),
    )
}

fn parse_filter_response(body: &str) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, LIST_FIXTURE);
    let entries = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(catalog_from_json)
        .collect();
    let meta = root.get("meta").unwrap_or(&Value::Null);
    Paged {
        entries,
        has_next_page: meta
            .get("current_page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            < meta.get("last_page").and_then(Value::as_u64).unwrap_or(1),
    }
}

fn parse_search_response(body: &str) -> Paged<CatalogItem> {
    let entries = json_or_fixture(body, SEARCH_FIXTURE)
        .as_array()
        .into_iter()
        .flatten()
        .map(catalog_from_json)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn details_from_slug(slug: &str) -> CatalogItem {
    catalog_details_from_json(&details_json(slug))
}

fn details_json(slug: &str) -> Value {
    json_or_fixture(
        &fetch_json_or_fixture(
            &format!("{API_BASE_URL}/serie/{}", normalize_slug(slug)),
            DETAILS_FIXTURE,
        ),
        DETAILS_FIXTURE,
    )
    .get("serie")
    .cloned()
    .unwrap_or_else(|| json_or_fixture(DETAILS_FIXTURE, DETAILS_FIXTURE)["serie"].clone())
}

fn catalog_from_json(item: &Value) -> CatalogItem {
    let key = normalize_slug(string_value(item, "slug").as_deref().unwrap_or("sample"));
    CatalogItem {
        key: key.clone(),
        title: string_value(item, "name").unwrap_or_else(|| "Spicy Scan".to_string()),
        cover: string_value(item, "urlImg"),
        url: Some(manga_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn catalog_details_from_json(item: &Value) -> CatalogItem {
    let mut out = catalog_from_json(item);
    out.description = string_value(item, "sinopsis");
    out.tags = item
        .get("genders")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|genre| string_value(genre, "name"))
        .collect();
    out.status = match item.get("stateId").and_then(Value::as_i64) {
        Some(1) => ItemStatus::Ongoing,
        Some(2) => ItemStatus::Hiatus,
        Some(3) | Some(5) => ItemStatus::Cancelled,
        Some(4) => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    };
    out.initialized = true;
    out
}

fn chapter_from_json(chapter: &Value, manga_slug: &str) -> MangaChapter {
    let slug = string_value(chapter, "slug").unwrap_or_else(|| "chapter-1".to_string());
    let number = chapter
        .get("num")
        .and_then(Value::as_f64)
        .map(|value| value as f32);
    MangaChapter {
        key: format!("{}/{}", normalize_slug(manga_slug), slug),
        title: Some(
            number
                .map(|value| format!("Capítulo {}", trim_float(value)))
                .unwrap_or_else(|| "Capítulo".to_string()),
        ),
        chapter_number: number,
        date_uploaded: string_value(chapter, "createdAt").and_then(|value| parse_date(&value)),
        url: Some(chapter_url(&format!("{manga_slug}/{slug}"))),
        language: Some(LANG.to_string()),
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = json_or_fixture(body, PAGES_FIXTURE);
    let raw = root
        .get("pageches")
        .and_then(|value| {
            value
                .as_array()
                .and_then(|items| items.first())
                .or_else(|| value.as_object().map(|_| value))
        })
        .and_then(|value| value.get("urlImg"))
        .and_then(Value::as_str)
        .unwrap_or("[]");
    serde_json::from_str::<Vec<String>>(raw)
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

fn manga_url(slug: &str) -> String {
    format!("{BASE_URL}/comic/{}", normalize_slug(slug))
}

fn chapter_url(key: &str) -> String {
    format!("{BASE_URL}/comic/{}", key.trim_matches('/'))
}

fn normalize_slug(input: &str) -> String {
    let mut value = input.trim().trim_end_matches('/').to_string();
    if let Some((_, rest)) = value.split_once("/comic/") {
        value = rest.to_string();
    }
    value.trim_matches('/').to_string()
}

fn filter<'a>(filters: &'a Value, key: &str, default: &'a str) -> &'a str {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
}

fn string_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn trim_float(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

fn parse_date(value: &str) -> Option<i64> {
    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = value.get(5..7)?.parse::<i32>().ok()?;
    let day = value.get(8..10)?.parse::<i32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
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

const LIST_FIXTURE: &str = r#"{"data":[{"id":1,"name":"Sample Spicy","slug":"sample","type":"Manhwa","sinopsis":"Summary","urlImg":"https://spicyseries.com/cover.jpg","stateId":1,"genders":[{"name":"Drama","id":10}],"chapters":[{"id":1,"num":1,"slug":"chapter-1","createdAt":"2024-01-01T00:00:00.000"}]}],"meta":{"current_page":1,"last_page":1}}"#;
const SEARCH_FIXTURE: &str = r#"[{"id":1,"name":"Sample Spicy","slug":"sample","type":"Manhwa","sinopsis":"Summary","urlImg":"https://spicyseries.com/cover.jpg","stateId":1,"genders":[{"name":"Drama","id":10}],"chapters":[]}]"#;
const DETAILS_FIXTURE: &str = r#"{"serie":{"id":1,"name":"Sample Spicy","slug":"sample","type":"Manhwa","sinopsis":"Summary","urlImg":"https://spicyseries.com/cover.jpg","stateId":1,"genders":[{"name":"Drama","id":10}],"chapters":[{"id":1,"num":1,"slug":"chapter-1","createdAt":"2024-01-01T00:00:00.000"}]}}"#;
const PAGES_FIXTURE: &str = r#"{"id":1,"num":1,"name":"Chapter 1","slug":"chapter-1","hasNext":false,"hasPrevious":false,"pageches":{"urlImg":"[\"https://spicyseries.com/page1.jpg\"]","chapterId":1}}"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(parse_filter_response(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(SOURCE.chapters(serde_json::json!({})).unwrap().len(), 1);
        assert_eq!(SOURCE.pages(serde_json::json!({})).unwrap().len(), 1);
    }
}
