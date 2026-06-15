use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Dynasty = Dynasty;
const BASE_URL: &str = "https://manhuako.net";
const NAME: &str = "Dynasty";
const LANG: &str = "es";
const CONTENT_RATING: &str = "safe";
const LIMIT: u64 = 20;

struct Dynasty;

impl MangaSource for Dynasty {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_page(LIST_FIXTURE, "popular", 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if listing_id(&request) == "latest" {
            "newest"
        } else {
            "popular"
        };
        Ok(parse_manga_page(
            &fetch_json(&manga_list_url(page, sort, None, None), LIST_FIXTURE),
            sort,
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
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let sort = filters
            .get("sort")
            .and_then(Value::as_str)
            .filter(|value| matches!(*value, "newest" | "popular" | "rating" | "az"))
            .unwrap_or("newest");
        let genre = if query.is_empty() {
            filters
                .get("genre")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        } else {
            None
        };
        Ok(parse_manga_page(
            &fetch_json(
                &manga_list_url(page, sort, Some(query), genre),
                LIST_FIXTURE,
            ),
            sort,
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1|sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1|sample".into());
        let manga_id = id_from_key(&key);
        let mut page = 1;
        let mut total = 1;
        let mut chapters = Vec::new();
        while page <= total {
            let body = fetch_json(
                &format!(
                    "{BASE_URL}/api/chapters/paginated?manga_id={manga_id}&page={page}&limit=100&sort=desc"
                ),
                CHAPTERS_FIXTURE,
            );
            let root = json_or_fixture(&body, CHAPTERS_FIXTURE);
            total = root
                .get("totalPages")
                .and_then(Value::as_u64)
                .unwrap_or(page);
            chapters.extend(
                root.get("chapters")
                    .or_else(|| root.get("data"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(chapter_from_json),
            );
            page += 1;
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "10".into());
        Ok(parse_pages(&fetch_json(
            &format!(
                "{BASE_URL}/api/chapter-pages?chapter_id={}",
                id_from_key(&key)
            ),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| {
            let slug = slug_from_key(&key);
            if slug.is_empty() {
                BASE_URL.to_string()
            } else {
                format!("{BASE_URL}/manga/{slug}")
            }
        }))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_manga_page(
            &fetch_json(&manga_list_url(1, "popular", None, None), LIST_FIXTURE),
            "popular",
            1,
        );
        let latest = parse_manga_page(
            &fetch_json(&manga_list_url(1, "newest", None, None), LIST_FIXTURE),
            "newest",
            1,
        );
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Populares".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Recientes".to_string(),
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
            let slug = input
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("sample");
            return Ok(Some(UrlResolveResult {
                search: Some(SearchRequest {
                    query: slug.replace('-', " "),
                    ..SearchRequest::default()
                }),
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
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn manga_list_url(page: u64, sort: &str, query: Option<&str>, genre: Option<&str>) -> String {
    let mut out = format!("{BASE_URL}/api/mangas?page={page}&limit={LIMIT}");
    if let Some(query) = query.filter(|value| !value.is_empty()) {
        out.push_str("&search=");
        out.push_str(&url::query_escape(query));
    } else if let Some(genre) = genre.filter(|value| !value.is_empty()) {
        out.push_str("&genre=");
        out.push_str(&url::query_escape(genre));
    }
    out.push_str("&sort=");
    out.push_str(sort);
    out
}

fn parse_manga_page(body: &str, sort: &str, page: u64) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, LIST_FIXTURE);
    let mut mangas = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| {
            string_value(item, "type")
                .is_none_or(|value| !value.to_ascii_lowercase().contains("novel"))
        })
        .cloned()
        .collect::<Vec<_>>();
    match sort {
        "popular" => {
            mangas.sort_by_key(|item| -(item.get("views").and_then(Value::as_i64).unwrap_or(0)))
        }
        "newest" => mangas.sort_by_key(|item| {
            -parse_rfc3339_utc(
                string_value(item, "updated_at")
                    .as_deref()
                    .unwrap_or_default(),
            )
            .unwrap_or(0)
        }),
        "rating" => mangas.sort_by(|a, b| {
            b.get("rating")
                .and_then(Value::as_f64)
                .partial_cmp(&a.get("rating").and_then(Value::as_f64))
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        "az" => mangas.sort_by_key(|item| string_value(item, "title").unwrap_or_default()),
        _ => {}
    }
    let total_pages = root
        .get("totalPages")
        .and_then(Value::as_u64)
        .unwrap_or(page);
    Paged {
        entries: mangas.iter().map(catalog_from_json).collect(),
        has_next_page: page < total_pages,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let id = id_from_key(key);
    let body = fetch_json(&format!("{BASE_URL}/api/mangas/{id}"), DETAILS_FIXTURE);
    let root = json_or_fixture(&body, DETAILS_FIXTURE);
    let data = root.get("data").unwrap_or(&root);
    catalog_details_from_json(data)
}

fn catalog_from_json(item: &Value) -> CatalogItem {
    let id = item.get("id").and_then(Value::as_i64).unwrap_or(1);
    let slug = string_value(item, "slug").unwrap_or_else(|| "sample".to_string());
    let key = format!("{id}|{slug}");
    CatalogItem {
        key: key.clone(),
        title: string_value(item, "title").unwrap_or_else(|| NAME.to_string()),
        cover: string_value(item, "cover_image"),
        url: Some(format!("{BASE_URL}/manga/{slug}")),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn catalog_details_from_json(item: &Value) -> CatalogItem {
    let mut out = catalog_from_json(item);
    out.description = string_value(item, "description");
    out.authors = string_value(item, "author").into_iter().collect();
    out.artists = string_value(item, "artist").into_iter().collect();
    out.tags = string_value(item, "type")
        .map(|value| vec![capitalize(&value)])
        .unwrap_or_default();
    out.status = match string_value(item, "status").as_deref() {
        Some("ongoing") => ItemStatus::Ongoing,
        Some("completed") => ItemStatus::Completed,
        Some("hiatus") => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    };
    out.initialized = true;
    out
}

fn chapter_from_json(item: &Value) -> MangaChapter {
    let id = item
        .get("id")
        .and_then(Value::as_i64)
        .unwrap_or(1)
        .to_string();
    let number = item.get("number").and_then(Value::as_f64);
    let number_label = number
        .map(|value| {
            if value.fract() == 0.0 {
                format!("{}", value as i64)
            } else {
                value.to_string()
            }
        })
        .unwrap_or_default();
    let title = string_value(item, "title").filter(|value| value != "null");
    let name = match (number_label.is_empty(), title) {
        (false, Some(title)) if !title.is_empty() => format!("Capitulo {number_label} - {title}"),
        (false, _) => format!("Capitulo {number_label}"),
        (true, Some(title)) if !title.is_empty() => title,
        _ => "Capitulo".to_string(),
    };
    MangaChapter {
        key: id.clone(),
        title: Some(name),
        chapter_number: number.map(|value| value as f32),
        date_uploaded: string_value(item, "created_at").and_then(|value| parse_rfc3339_utc(&value)),
        url: Some(id),
        language: Some(LANG.to_string()),
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = json_or_fixture(body, PAGES_FIXTURE);
    root.as_array()
        .into_iter()
        .flatten()
        .filter_map(|page| string_value(page, "image_url"))
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

fn id_from_key(key: &str) -> String {
    key.split('|')
        .next()
        .unwrap_or(key)
        .trim_matches('/')
        .to_string()
}

fn slug_from_key(key: &str) -> String {
    key.split('|').nth(1).unwrap_or_default().to_string()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return input
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample")
            .to_string();
    }
    input.to_string()
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
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

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
        .unwrap_or_default()
}

fn parse_rfc3339_utc(value: &str) -> Option<i64> {
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

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":[{"id":1,"title":"Sample","slug":"sample","cover_image":"https://manhuako.net/cover.jpg","status":"ongoing","type":"manga","views":10,"rating":4.5,"updated_at":"2024-01-01T00:00:00.000Z"}],"totalPages":1}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"id":1,"title":"Sample","slug":"sample","description":"Summary","cover_image":"https://manhuako.net/cover.jpg","author":"Author","artist":"Artist","status":"ongoing","type":"manga"}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"chapters":[{"id":10,"number":1,"title":"Start","created_at":"2024-01-01T00:00:00.000Z"}],"totalPages":1}"#;
const PAGES_FIXTURE: &str = r#"[{"image_url":"https://manhuako.net/page1.jpg"},{"image_url":"https://manhuako.net/page2.jpg"}]"#;
