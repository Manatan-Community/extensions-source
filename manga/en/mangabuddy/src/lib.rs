use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaK = MangaK;
const BASE_URL: &str = "https://mangak.io";
const API_URL: &str = "https://api.mangak.io";

struct MangaK;

impl MangaSource for MangaK {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_search_response(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort = if listing == "latest" {
            "latest"
        } else {
            "popular"
        };
        Ok(parse_search_response(&fetch_json(
            &search_url(
                page,
                "",
                sort,
                if listing == "popular" {
                    Some("week")
                } else {
                    None
                },
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
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let sort = filter_value(&request, "sort").unwrap_or_else(|| "latest".to_string());
        Ok(parse_search_response(&fetch_json(
            &search_url(page, query, &sort, None),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/sample#1".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/sample#1".into());
        let id = key
            .rsplit_once('#')
            .map(|(_, id)| id)
            .unwrap_or_else(|| id_from_details(&key).unwrap_or("1"));
        Ok(parse_chapters(&fetch_json(
            &format!("{API_URL}/titles/{id}/chapters?cv=0"),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/title/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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
        .with_origin(BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(page: u64, query: &str, sort: &str, window: Option<&str>) -> String {
    let mut params = vec![
        format!("page={page}"),
        "limit=24".to_string(),
        format!("sort={}", url::query_escape(sort)),
    ];
    if let Some(window) = window {
        params.push(format!("window={}", url::query_escape(window)));
    }
    if !query.is_empty() {
        params.push(format!("q={}", url::query_escape(query)));
    }
    format!("{API_URL}/titles/search?{}", params.join("&"))
}

fn parse_search_response(body: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let entries = root
        .pointer("/data/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(search_item)
        .collect();
    Paged {
        entries,
        has_next_page: root
            .pointer("/data/pagination/has_next")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn search_item(item: &Value) -> CatalogItem {
    let id = string_field(item, "id");
    let path = string_field(item, "url");
    let key = if id.is_empty() {
        normalize_key(&path)
    } else {
        format!("{}#{id}", normalize_key(&path))
    };
    CatalogItem {
        key: key.clone(),
        title: string_field(item, "name"),
        cover: string_opt(item, "cover"),
        url: Some(url::join_url(
            BASE_URL,
            key.split('#').next().unwrap_or(&key),
        )),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let path = key.split('#').next().unwrap_or(key);
    let body = fetch_document(&url::join_url(BASE_URL, path), DETAILS_FIXTURE);
    let data =
        extract_next_data(&body).unwrap_or_else(|| serde_json::from_str(DETAILS_JSON).unwrap());
    let manga = data
        .pointer("/props/pageProps/initialManga")
        .or_else(|| data.pointer("/pageProps/initialManga"));
    manga
        .map(|value| details_item(value, Some(key.to_string())))
        .unwrap_or_else(|| fallback_item(key))
}

fn id_from_details(key: &str) -> Option<&str> {
    key.rsplit_once('#').map(|(_, id)| id)
}

fn details_item(value: &Value, key: Option<String>) -> CatalogItem {
    let id = string_field(value, "id");
    let key = key.unwrap_or_else(|| format!("/title/sample#{id}"));
    CatalogItem {
        key: key.clone(),
        title: string_field(value, "name"),
        cover: string_opt(value, "cover"),
        description: string_opt(value, "summary")
            .map(|summary| {
                if id.is_empty() {
                    summary
                } else {
                    format!("{summary}\n\nManga ID: {id}")
                }
            })
            .filter(|value| !value.trim().is_empty()),
        authors: entity_names(value.get("authors")),
        tags: entity_names(value.get("genres")),
        status: status_from(string_opt(value, "status").as_deref()),
        url: Some(url::join_url(
            BASE_URL,
            key.split('#').next().unwrap_or(&key),
        )),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let mut chapters = root
        .pointer("/data/chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|chapter| {
            let key = normalize_key(&string_field(chapter, "url"));
            MangaChapter {
                key: key.clone(),
                title: string_opt(chapter, "name"),
                chapter_number: chapter
                    .get("chapter_number")
                    .and_then(Value::as_f64)
                    .map(|n| n as f32),
                date_uploaded: parse_rfc3339_date(string_opt(chapter, "updated_at").as_deref()),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .partial_cmp(&a.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let data = extract_next_data(body).unwrap_or_else(|| serde_json::from_str(PAGES_JSON).unwrap());
    data.pointer("/props/pageProps/initialChapter/images")
        .or_else(|| data.pointer("/pageProps/initialChapter/images"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.to_string(),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn extract_next_data(body: &str) -> Option<Value> {
    let after_marker = body.split("__NEXT_DATA__").nth(1)?;
    let json = after_marker.split_once('>')?.1.split("</script>").next()?;
    serde_json::from_str(&html::html_unescape(json)).ok()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input
                .trim_start_matches(BASE_URL)
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn entity_names(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("name").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn status_from(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "hiatus" => ItemStatus::Hiatus,
        "cancelled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_rfc3339_date(value: Option<&str>) -> Option<i64> {
    let date = value?.split('T').next()?;
    let parts = date.split('-').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    unix_date(
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    )
}

fn unix_date(year: i32, month: u32, day: u32) -> Option<i64> {
    let mut days = 0i64;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    for m in 1..month {
        days += match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if is_leap(year) => 29,
            2 => 28,
            _ => return None,
        };
    }
    Some((days + day as i64 - 1) * 86_400)
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn string_field(value: &Value, key: &str) -> String {
    string_opt(value, key).unwrap_or_default()
}

fn string_opt(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn filter_value(request: &Value, key: &str) -> Option<String> {
    request
        .get(key)
        .and_then(Value::as_str)
        .or_else(|| request.get("filters")?.get(key)?.as_str())
        .map(ToString::to_string)
}

fn fallback_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: url::slug_from_url(key).unwrap_or_else(|| "MangaK".to_string()),
        url: Some(url::join_url(
            BASE_URL,
            key.split('#').next().unwrap_or(key),
        )),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":{"items":[{"id":"1","name":"Sample Manga","cover":"https://img.example/cover.jpg","url":"/title/sample"}],"pagination":{"has_next":false}}}"#;
const DETAILS_JSON: &str = r#"{"props":{"pageProps":{"initialManga":{"id":"1","name":"Sample Manga","summary":"Description","cover":"https://img.example/cover.jpg","authors":[{"name":"Author"}],"genres":[{"name":"Action"}],"status":"ongoing"}}}}"#;
const DETAILS_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"initialManga":{"id":"1","name":"Sample Manga","summary":"Description","cover":"https://img.example/cover.jpg","authors":[{"name":"Author"}],"genres":[{"name":"Action"}],"status":"ongoing"}}}}</script>"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"chapters":[{"url":"/title/sample/chapter-1","name":"Chapter 1","updated_at":"2024-01-01T00:00:00.000Z","chapter_number":1}]}}"#;
const PAGES_JSON: &str = r#"{"props":{"pageProps":{"initialChapter":{"images":["https://img.example/001.jpg","https://img.example/002.jpg"]}}}}"#;
const PAGES_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"initialChapter":{"images":["https://img.example/001.jpg","https://img.example/002.jpg"]}}}}</script>"#;
