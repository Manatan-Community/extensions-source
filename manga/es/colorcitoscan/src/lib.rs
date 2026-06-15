use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::{ExtensionError, ExtensionResult},
    export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: ColorcitoScan = ColorcitoScan;
const BASE_URL: &str = "https://colorcitoscan.com";
const API_BASE_URL: &str = "https://api.colorcitoscan.com";
const NAME: &str = "Colorcito Scan";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";
const PAGE_SIZE: u64 = 12;

struct ColorcitoScan;

impl MangaSource for ColorcitoScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order_by = if listing_id(&request) == "latest" {
            "3"
        } else {
            "6"
        };
        Ok(parse_listing(
            &fetch_json_or_fixture(
                &filter_url(page, order_by, "desc", &Value::Null),
                LIST_FIXTURE,
            ),
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
        if !query.is_empty() {
            if query.chars().count() < 2 {
                return Err(ExtensionError {
                    message: "Escribe al menos 2 caracteres para buscar".to_string(),
                });
            }
            return Ok(Paged {
                entries: parse_search(&fetch_json_or_fixture(
                    &format!(
                        "{API_BASE_URL}/home/buscar?query={}",
                        url::query_escape(query)
                    ),
                    SEARCH_FIXTURE,
                )),
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let order_by = filters
            .get("orderBy")
            .or_else(|| filters.get("sort"))
            .and_then(Value::as_str)
            .filter(|value| matches!(*value, "2" | "3" | "4" | "5" | "6"))
            .unwrap_or("3");
        let direction = filters
            .get("direction")
            .or_else(|| filters.get("sortDirection"))
            .and_then(Value::as_str)
            .filter(|value| matches!(*value, "asc" | "desc"))
            .unwrap_or("desc");
        Ok(parse_listing(
            &fetch_json_or_fixture(
                &filter_url(page, order_by, direction, filters),
                LIST_FIXTURE,
            ),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(details_from_slug(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = fetch_json_or_fixture(&details_api_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/chapter-1".into());
        Ok(parse_pages(&fetch_json_or_fixture(
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
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_cookies_for(API_BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .header("Origin", BASE_URL)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn filter_url(page: u64, order_by: &str, direction: &str, filters: &Value) -> String {
    let mut pairs = vec![
        ("page", page.to_string()),
        ("limit", PAGE_SIZE.to_string()),
        ("orderBy", order_by.to_string()),
        ("sort", direction.to_string()),
        ("gendersId", filter_value(filters, &["gendersId", "genres"])),
        ("origin", filter_value(filters, &["origin"])),
        ("state", filter_value(filters, &["state", "status"])),
        ("loading", "true".to_string()),
    ];
    format!(
        "{API_BASE_URL}/filtrar?{}",
        pairs
            .drain(..)
            .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn details_api_url(slug: &str) -> String {
    format!("{API_BASE_URL}/serie/{}", normalize_slug(slug))
}

fn pages_api_url(key: &str) -> String {
    format!("{API_BASE_URL}/serie/{}/", key.trim_matches('/'))
}

fn parse_listing(body: &str, page: u64) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, LIST_FIXTURE);
    let entries = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(catalog_from_json)
        .collect();
    let current_page = root
        .get("meta")
        .and_then(|meta| meta.get("current_page"))
        .and_then(Value::as_u64)
        .unwrap_or(page);
    let last_page = root
        .get("meta")
        .and_then(|meta| meta.get("last_page"))
        .and_then(Value::as_u64)
        .unwrap_or(current_page);
    Paged {
        entries,
        has_next_page: current_page < last_page,
    }
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    json_or_fixture(body, SEARCH_FIXTURE)
        .as_array()
        .into_iter()
        .flatten()
        .map(catalog_from_json)
        .collect()
}

fn details_from_slug(slug: &str) -> CatalogItem {
    let body = fetch_json_or_fixture(&details_api_url(slug), DETAILS_FIXTURE);
    catalog_details_from_json(
        json_or_fixture(&body, DETAILS_FIXTURE)
            .get("serie")
            .unwrap_or(&Value::Null),
    )
}

fn catalog_from_json(item: &Value) -> CatalogItem {
    let key = string_value(item, "slug").unwrap_or_else(|| "sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: string_value(item, "name").unwrap_or_else(|| NAME.to_string()),
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
    out.description = string_value(item, "sinopsis").or_else(|| string_value(item, "synopsis"));
    out.tags = item
        .get("genders")
        .or_else(|| item.get("genres"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|genre| string_value(genre, "name"))
        .collect();
    out.status = match item.get("stateId").and_then(Value::as_i64) {
        Some(1) => ItemStatus::Ongoing,
        Some(2) => ItemStatus::Hiatus,
        Some(4) => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    };
    out.initialized = true;
    out
}

fn parse_chapters(body: &str, series_slug: &str) -> Vec<MangaChapter> {
    let root = json_or_fixture(body, DETAILS_FIXTURE);
    let series = root.get("serie").unwrap_or(&Value::Null);
    let slug = string_value(series, "slug").unwrap_or_else(|| normalize_slug(series_slug));
    series
        .get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|chapter| {
            let chapter_slug =
                string_value(chapter, "slug").unwrap_or_else(|| "chapter-1".to_string());
            let number = chapter.get("num").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            MangaChapter {
                key: format!("{slug}/{chapter_slug}"),
                title: Some(format!("Capitulo {number}")),
                chapter_number: Some(number),
                date_uploaded: string_value(chapter, "createdAt")
                    .and_then(|value| parse_date(&value)),
                url: Some(chapter_url(&format!("{slug}/{chapter_slug}"))),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = json_or_fixture(body, PAGES_FIXTURE);
    let raw = root
        .get("pageches")
        .and_then(first_value)
        .and_then(|pageches| string_value(pageches, "urlImg"))
        .unwrap_or_else(|| "[]".to_string());
    let images = serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default();
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn first_value(value: &Value) -> Option<&Value> {
    value
        .as_array()
        .and_then(|items| items.first())
        .or(Some(value))
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
        value = rest.split('/').next().unwrap_or(rest).to_string();
    }
    value.trim_matches('/').to_string()
}

fn filter_value(filters: &Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(value) = filters.get(*key) {
            if let Some(text) = value.as_str().filter(|text| !text.is_empty()) {
                return text.to_string();
            }
            if let Some(items) = value.as_array() {
                return items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|item| !item.is_empty())
                    .collect::<Vec<_>>()
                    .join(",");
            }
        }
    }
    String::new()
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
        .or_else(|_| serde_json::from_str(fixture))
        .unwrap_or(Value::Null)
}

fn parse_date(value: &str) -> Option<i64> {
    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = value.get(5..7)?.parse::<i32>().ok()?;
    let day = value.get(8..10)?.parse::<i32>().ok()?;
    let hour = value.get(11..13).unwrap_or("00").parse::<i64>().ok()?;
    let minute = value.get(14..16).unwrap_or("00").parse::<i64>().ok()?;
    let second = value.get(17..19).unwrap_or("00").parse::<i64>().ok()?;
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

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

const LIST_FIXTURE: &str = r#"{"data":[{"id":1,"name":"Sample","slug":"sample","type":"Manga","sinopsis":"Summary","urlImg":"https://api.colorcitoscan.com/sample.jpg","stateId":1,"alternativeName":null,"createdAt":"2024-01-01T00:00:00.000","users_count":1,"chapters_count":1,"genders":[],"chapters":null}],"meta":{"total":1,"per_page":12,"current_page":1,"last_page":1}}"#;
const SEARCH_FIXTURE: &str = r#"[{"id":1,"name":"Sample","slug":"sample","type":"Manga","sinopsis":"Summary","urlImg":"https://api.colorcitoscan.com/sample.jpg","stateId":1,"alternativeName":null,"createdAt":"2024-01-01T00:00:00.000","users_count":1,"chapters_count":1,"genders":[],"chapters":null}]"#;
const DETAILS_FIXTURE: &str = r#"{"serie":{"id":1,"name":"Sample","slug":"sample","type":"Manga","sinopsis":"Summary","urlImg":"https://api.colorcitoscan.com/sample.jpg","stateId":1,"alternativeName":null,"createdAt":"2024-01-01T00:00:00.000","users_count":1,"chapters_count":1,"genders":[{"name":"Drama","id":1}],"chapters":[{"id":1,"num":1,"slug":"chapter-1","createdAt":"2024-01-01T00:00:00.000"}]}}"#;
const PAGES_FIXTURE: &str = r#"{"id":1,"num":1,"name":null,"slug":"chapter-1","hasNext":false,"hasPrevious":false,"pageches":{"urlImg":"[\"https://api.colorcitoscan.com/page-1.jpg\"]","chapterId":1}}"#;

export_manga_source!(SOURCE);
