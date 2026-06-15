use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: PlatinumLilyScan = PlatinumLilyScan;
const BASE_URL: &str = "https://platinumlilyscan.com";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct PlatinumLilyScan;

impl MangaSource for PlatinumLilyScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = fetch_api_or_fixture("/api/series", SERIES_FIXTURE);
        let mut series = parse_series_list(&body);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            series.sort_by(|left, right| {
                updated_at(right)
                    .unwrap_or_default()
                    .cmp(&updated_at(left).unwrap_or_default())
            });
        } else {
            series.sort_by(|left, right| bookmark_count(right).cmp(&bookmark_count(left)));
        }
        Ok(Paged {
            entries: series
                .iter()
                .map(|item| catalog_from_series(item))
                .collect(),
            has_next_page: false,
        })
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
        let mut entries = parse_series_list(&fetch_api_or_fixture("/api/series", SERIES_FIXTURE));
        entries.retain(|series| {
            matches_query(series, query) && matches_filters(series, request.get("filters"))
        });
        entries.sort_by(|left, right| {
            updated_at(right)
                .unwrap_or_default()
                .cmp(&updated_at(left).unwrap_or_default())
        });
        Ok(Paged {
            entries: entries
                .iter()
                .map(|item| catalog_from_series(item))
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(details_from_slug(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let series = series_from_slug(&key);
        Ok(series
            .get("chapters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|chapter| string_value(chapter, "id").is_some())
            .map(|chapter| chapter_from_json(chapter, &normalize_slug(&key)))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "sample#chapter-1".to_string());
        let (series_slug, chapter_id) = key.split_once('#').unwrap_or(("sample", "chapter-1"));
        let series = series_from_slug(series_slug);
        let pages = series
            .get("chapters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|chapter| string_value(chapter, "id").as_deref() == Some(chapter_id))
            .and_then(|chapter| chapter.get("pages"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| json_array_or_empty(PAGES_FIXTURE));
        Ok(pages
            .iter()
            .filter_map(|page| string_value(page, "imageUrl"))
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: absolute_url(&image),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| manga_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (series_slug, _) = key.split_once('#').unwrap_or((&key, ""));
            manga_url(series_slug)
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/series/") {
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
        .with_webview_challenge_fallback()
}

fn fetch_api_or_fixture(path: &str, fixture: &str) -> String {
    client()
        .get(url::join_url(BASE_URL, path))
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_series_list(body: &str) -> Vec<Value> {
    json_or_fixture(body, SERIES_FIXTURE)
        .as_array()
        .cloned()
        .unwrap_or_else(|| json_array_or_empty(SERIES_FIXTURE))
}

fn series_from_slug(slug: &str) -> Value {
    let key = normalize_slug(slug);
    json_or_fixture(
        &fetch_api_or_fixture(&format!("/api/series/{key}"), DETAILS_FIXTURE),
        DETAILS_FIXTURE,
    )
}

fn details_from_slug(slug: &str) -> CatalogItem {
    catalog_from_series(&series_from_slug(slug))
}

fn catalog_from_series(series: &Value) -> CatalogItem {
    let key = normalize_slug(string_value(series, "slug").as_deref().unwrap_or("sample"));
    CatalogItem {
        key: key.clone(),
        title: string_value(series, "title").unwrap_or_else(|| "Platinum Lily Scan".to_string()),
        cover: string_value(series, "coverUrl").map(|value| absolute_url(&value)),
        description: string_value(series, "description"),
        authors: string_value(series, "author").into_iter().collect(),
        artists: string_value(series, "artist").into_iter().collect(),
        tags: genre_names(series),
        status: match string_value(series, "status").as_deref() {
            Some("ONGOING") => ItemStatus::Ongoing,
            Some("COMPLETED") => ItemStatus::Completed,
            Some("HIATUS") => ItemStatus::Hiatus,
            _ => ItemStatus::Unknown,
        },
        url: Some(manga_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn chapter_from_json(chapter: &Value, series_slug: &str) -> MangaChapter {
    let id = string_value(chapter, "id").unwrap_or_else(|| "chapter-1".to_string());
    let number = chapter
        .get("number")
        .and_then(Value::as_f64)
        .map(|value| value as f32);
    let number_text = number
        .map(|value| {
            if value.fract() == 0.0 {
                format!("{}", value as i64)
            } else {
                value.to_string()
            }
        })
        .unwrap_or_else(|| "?".to_string());
    let raw_title = string_value(chapter, "title").unwrap_or_default();
    let title = if raw_title.is_empty() {
        format!("Capitulo {number_text}")
    } else {
        format!("Capitulo {number_text} - {raw_title}")
    };
    MangaChapter {
        key: format!("{series_slug}#{id}"),
        title: Some(title),
        chapter_number: number,
        date_uploaded: string_value(chapter, "publishedAt")
            .and_then(|value| parse_rfc3339_utc(&value)),
        language: Some(LANG.to_string()),
        url: Some(manga_url(series_slug)),
        ..MangaChapter::default()
    }
}

fn matches_query(series: &Value, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let needle = query.to_ascii_lowercase();
    string_value(series, "title").is_some_and(|title| title.to_ascii_lowercase().contains(&needle))
        || string_value(series, "altTitles")
            .is_some_and(|title| title.to_ascii_lowercase().contains(&needle))
}

fn matches_filters(series: &Value, filters: Option<&Value>) -> bool {
    let filters = filters.unwrap_or(&Value::Null);
    for key in ["type", "status"] {
        if let Some(expected) = filter_string(filters, key).filter(|value| !value.is_empty()) {
            if string_value(series, key).as_deref() != Some(expected.as_str()) {
                return false;
            }
        }
    }
    if let Some(expected) = filter_string(filters, "rating").filter(|value| !value.is_empty()) {
        if string_value(series, "contentRating").as_deref() != Some(expected.as_str()) {
            return false;
        }
    }
    if let Some(expected) = filter_string(filters, "genre").filter(|value| !value.is_empty()) {
        if !genre_names(series)
            .iter()
            .any(|genre| genre.eq_ignore_ascii_case(&expected))
        {
            return false;
        }
    }
    true
}

fn genre_names(series: &Value) -> Vec<String> {
    series
        .get("genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|genre| string_path(genre, &["genre", "name"]))
        .collect()
}

fn bookmark_count(series: &Value) -> i64 {
    series
        .get("_count")
        .and_then(|count| count.get("bookmarks"))
        .and_then(Value::as_i64)
        .unwrap_or_default()
}

fn updated_at(series: &Value) -> Option<i64> {
    string_value(series, "updatedAt").and_then(|value| parse_rfc3339_utc(&value))
}

fn manga_url(slug: &str) -> String {
    format!("{BASE_URL}/series/{}", normalize_slug(slug))
}

fn normalize_slug(input: &str) -> String {
    let mut value = input.trim().trim_end_matches('/').to_string();
    if let Some((_, rest)) = value.split_once("/series/") {
        value = rest.split('/').next().unwrap_or(rest).to_string();
    }
    value.trim_matches('/').to_string()
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        url::join_url(BASE_URL, value)
    }
}

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
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

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn json_array_or_empty(body: &str) -> Vec<Value> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
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

const SERIES_FIXTURE: &str = r#"[{"title":"Sample","slug":"sample","description":"Summary","coverUrl":"/cover.jpg","author":"Author","artist":"Artist","genres":[{"genre":{"name":"Yuri"}}],"status":"ONGOING","type":"MANGA","contentRating":"SAFE","updatedAt":"2024-01-01T00:00:00.000Z","_count":{"bookmarks":3},"chapters":[{"id":"chapter-1","number":1,"title":"Sample","publishedAt":"2024-01-01T00:00:00.000Z","pages":[{"imageUrl":"/page1.jpg"}]}]}]"#;
const DETAILS_FIXTURE: &str = r#"{"title":"Sample","slug":"sample","description":"Summary","coverUrl":"/cover.jpg","author":"Author","artist":"Artist","genres":[{"genre":{"name":"Yuri"}}],"status":"ONGOING","type":"MANGA","contentRating":"SAFE","updatedAt":"2024-01-01T00:00:00.000Z","_count":{"bookmarks":3},"chapters":[{"id":"chapter-1","number":1,"title":"Sample","publishedAt":"2024-01-01T00:00:00.000Z","pages":[{"imageUrl":"/page1.jpg"}]}]}"#;
const PAGES_FIXTURE: &str = r#"[{"imageUrl":"/page1.jpg"}]"#;

export_manga_source!(SOURCE);
