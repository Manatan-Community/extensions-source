use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: HentaiHall = HentaiHall;
const BASE_URL: &str = "https://hentaihall.com";
const API_URL: &str = "https://hentaihallbackend-production.up.railway.app";
const NAME: &str = "HentaiHall";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct HentaiHall;

impl MangaSource for HentaiHall {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if listing_id(&request) == "latest" {
            "creacion"
        } else {
            "seguir"
        };
        Ok(parse_listing(&fetch_json_or_fixture(
            &library_url(page, "", "nombre", order, "desc", ""),
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
                entries: vec![parse_details(&fetch_json_or_fixture(
                    &details_api_url(&key),
                    DETAILS_FIXTURE,
                ))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let search_by = filter_str(filters, "searchBy").unwrap_or_else(|| "nombre".to_string());
        let sort_by = filter_str(filters, "sort").unwrap_or_else(|| "seguir".to_string());
        let direction = filter_str(filters, "direction").unwrap_or_else(|| "desc".to_string());
        let genres = filter_array(filters, "genres").join("_");
        Ok(parse_listing(&fetch_json_or_fixture(
            &library_url(page, query, &search_by, &sort_by, &direction, &genres),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_details(&fetch_json_or_fixture(
            &details_api_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = fetch_json_or_fixture(&details_api_url(&key), DETAILS_FIXTURE);
        Ok(vec![chapter_from_details(
            json_or_fixture(&body, DETAILS_FIXTURE)
                .get("data")
                .unwrap_or(&Value::Null),
        )])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample".to_string());
        Ok(parse_pages(&fetch_json_or_fixture(
            &format!("{API_URL}/manhwa/chapter/{}", normalize_key(&key)),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/content/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}/reader/{key}")))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(&fetch_json_or_fixture(
            &library_url(1, "", "nombre", "seguir", "desc", ""),
            LIST_FIXTURE,
        ));
        let latest = parse_listing(&fetch_json_or_fixture(
            &library_url(1, "", "nombre", "creacion", "desc", ""),
            LIST_FIXTURE,
        ));
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_json_or_fixture(
                    &details_api_url(&key),
                    DETAILS_FIXTURE,
                ))),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_cookies_for(API_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Origin", BASE_URL)
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn library_url(
    page: u64,
    query: &str,
    search_by: &str,
    order: &str,
    direction: &str,
    genres: &str,
) -> String {
    format!(
        "{API_URL}/manhwa/library?buscar={}&quebusca={}&order_item={}&order_dir={}&page={}&generes={}",
        url::query_escape(query),
        url::query_escape(search_by),
        url::query_escape(order),
        url::query_escape(direction),
        page.saturating_sub(1),
        url::query_escape(genres)
    )
}

fn details_api_url(key: &str) -> String {
    format!("{API_URL}/manhwa/see/{}", normalize_key(key))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, LIST_FIXTURE);
    let entries = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(catalog_from_json)
        .collect();
    Paged {
        entries,
        has_next_page: root.get("next").and_then(Value::as_bool).unwrap_or(false),
    }
}

fn catalog_from_json(item: &Value) -> CatalogItem {
    let key = string_value(item, "_id").unwrap_or_else(|| "sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: string_value(item, "nombre").unwrap_or_else(|| NAME.to_string()),
        cover: string_value(item, "imagen"),
        url: Some(format!("{BASE_URL}/content/{key}")),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str) -> CatalogItem {
    let root = json_or_fixture(body, DETAILS_FIXTURE);
    let item = root.get("data").unwrap_or(&root);
    let mut out = catalog_from_json(item);
    out.authors = string_array(item, "autores");
    out.artists = out.authors.clone();
    out.tags = string_array(item, "tags");
    out.status = ItemStatus::Completed;
    out.description = details_description(item);
    out.initialized = true;
    out
}

fn chapter_from_details(item: &Value) -> MangaChapter {
    let key = string_value(item, "_id").unwrap_or_else(|| "sample".to_string());
    MangaChapter {
        key: key.clone(),
        title: Some("Chapter".to_string()),
        chapter_number: Some(1.0),
        date_uploaded: string_value(item, "creacion").and_then(|value| parse_rfc3339(&value)),
        url: Some(format!("{BASE_URL}/reader/{key}")),
        language: Some(LANG.to_string()),
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    json_or_fixture(body, PAGES_FIXTURE)
        .get("chapter")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|image| !image.trim().is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.to_string(),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn details_description(item: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(kind) = string_value(item, "tipo") {
        parts.push(format!("Tipo: {kind}"));
    }
    if let Some(lang) = string_value(item, "lenguaje") {
        parts.push(format!(
            "Lenguaje: {}",
            if lang == "esp" { "Espanol" } else { &lang }
        ));
    }
    if let Some(group) = string_value(item, "name_grupo") {
        parts.push(format!("Grupo: {group}"));
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(input)
        .to_string()
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn filter_str(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn filter_array(filters: &Value, key: &str) -> Vec<String> {
    filters
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn string_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn parse_rfc3339(value: &str) -> Option<i64> {
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

const LIST_FIXTURE: &str = r#"{"data":[{"_id":"sample","nombre":"Sample","imagen":"https://hentaihall.com/sample.jpg"}],"next":false}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"_id":"sample","nombre":"Sample","imagen":"https://hentaihall.com/sample.jpg","tags":["Tag"],"autores":["Author"],"tipo":"doujin","creacion":"2024-01-01T00:00:00.000Z","name_grupo":"Group","lenguaje":"esp"}}"#;
const PAGES_FIXTURE: &str = r#"{"chapter":["https://hentaihall.com/page1.jpg"]}"#;

export_manga_source!(SOURCE);
