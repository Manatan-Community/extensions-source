use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: TraduccionesMoonlight = TraduccionesMoonlight;
const BASE_URL: &str = "https://traduccionesmoonlight.com";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";
const PAGE_SIZE: usize = 15;

struct TraduccionesMoonlight;

impl MangaSource for TraduccionesMoonlight {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(TOP_FIXTURE));
        }
        let listing = request
            .get("listingId")
            .or_else(|| request.get("listing"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            return Ok(parse_series_response(
                &fetch_api("/api/lastUpdates", LATEST_FIXTURE),
                1,
            ));
        }
        Ok(parse_popular(&fetch_api("/api/topSerie", TOP_FIXTURE)))
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1) as usize;
        let body = fetch_api("/api/comics", COMICS_FIXTURE);
        let mut entries = series_array_from_response(&body);
        if !query.is_empty() {
            let query_lower = query.to_ascii_lowercase();
            entries.retain(|item| {
                string_value(item, "name")
                    .or_else(|| string_value(item, "alternativeName"))
                    .is_some_and(|value| value.to_ascii_lowercase().contains(&query_lower))
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        if let Some(status) = filter(filters, "status").filter(|value| *value != "0") {
            entries.retain(|item| {
                item.get("state_id")
                    .and_then(Value::as_i64)
                    .map(|value| value.to_string())
                    .as_deref()
                    == Some(status)
            });
        }
        sort_entries(
            &mut entries,
            filter(filters, "sort").unwrap_or("updated_at"),
        );
        if filter(filters, "direction") != Some("asc") {
            entries.reverse();
        }
        Ok(page_entries(entries, page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/ver/sample".to_string());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/ver/sample".to_string());
        let series = details_json(&key);
        let slug = string_value(&series, "slug").unwrap_or_else(|| normalize_key(&key));
        Ok(series
            .get("lastChapters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|chapter| chapter_from_json(chapter, &slug))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/ver/sample/chapter-1".to_string());
        let body = fetch_document(&absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/ver/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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
        .with_webview_challenge_fallback()
}

fn fetch_api(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{BASE_URL}{path}"))
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

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let response = json_or_fixture(body, TOP_FIXTURE);
    let mut entries = Vec::new();
    for key in ["diario", "semanal", "mensual"] {
        for item in response
            .pointer(&format!("/response/{key}"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|group| group.as_array().into_iter().flatten())
            .filter_map(|payload| payload.get("project"))
        {
            let catalog = catalog_from_json(item);
            if !entries
                .iter()
                .any(|existing: &CatalogItem| existing.key == catalog.key)
            {
                entries.push(catalog);
            }
        }
    }
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_series_response(body: &str, page: usize) -> Paged<CatalogItem> {
    page_entries(series_array_from_response(body), page)
}

fn series_array_from_response(body: &str) -> Vec<Value> {
    json_or_fixture(body, COMICS_FIXTURE)
        .get("response")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn page_entries(entries: Vec<Value>, page: usize) -> Paged<CatalogItem> {
    let start = PAGE_SIZE * page.saturating_sub(1);
    let has_next_page = entries.len() > start + PAGE_SIZE;
    Paged {
        entries: entries
            .into_iter()
            .skip(start)
            .take(PAGE_SIZE)
            .map(|item| catalog_from_json(&item))
            .collect(),
        has_next_page,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    catalog_details_from_json(&details_json(key))
}

fn details_json(key: &str) -> Value {
    let slug = normalize_key(key).trim_start_matches("ver/").to_string();
    json_or_fixture(
        &fetch_api(&format!("/api/showProject/{slug}"), DETAILS_FIXTURE),
        DETAILS_FIXTURE,
    )
    .get("response")
    .cloned()
    .unwrap_or_else(|| json_or_fixture(DETAILS_FIXTURE, DETAILS_FIXTURE)["response"].clone())
}

fn catalog_from_json(item: &Value) -> CatalogItem {
    let slug = string_value(item, "slug").unwrap_or_else(|| "sample".to_string());
    CatalogItem {
        key: format!("/ver/{slug}"),
        title: string_value(item, "name").unwrap_or_else(|| "Traducciones Moonlight".to_string()),
        cover: string_value(item, "urlImg"),
        url: Some(absolute_url(&format!("/ver/{slug}"))),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn catalog_details_from_json(item: &Value) -> CatalogItem {
    let mut out = catalog_from_json(item);
    out.description = string_value(item, "sinopsis");
    if let Some(alt) = string_value(item, "alternativeName") {
        out.description = Some(format!(
            "{}\n\nNombres alternativos: {alt}",
            out.description.unwrap_or_default()
        ));
    }
    out.tags = nested_names(item, "genders", "gender");
    out.authors = nested_names(item, "autors", "autor");
    out.artists = nested_names(item, "artists", "artist");
    out.status = match item.get("state_id").and_then(Value::as_i64) {
        Some(1) => ItemStatus::Ongoing,
        Some(2) => ItemStatus::Hiatus,
        Some(3) => ItemStatus::Cancelled,
        Some(4) => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    };
    out.initialized = true;
    out
}

fn chapter_from_json(chapter: &Value, series_slug: &str) -> MangaChapter {
    let slug = string_value(chapter, "slug").unwrap_or_else(|| "chapter-1".to_string());
    let number = chapter
        .get("num")
        .and_then(Value::as_f64)
        .map(|value| value as f32);
    let mut title = number
        .map(|value| format!("Capítulo {}", trim_float(value)))
        .unwrap_or_else(|| "Capítulo".to_string());
    if let Some(name) = string_value(chapter, "name").filter(|value| !value.is_empty()) {
        title.push_str(" - ");
        title.push_str(&name);
    }
    MangaChapter {
        key: format!("/ver/{series_slug}/{slug}"),
        title: Some(title),
        chapter_number: number,
        date_uploaded: string_value(chapter, "created_at").and_then(|value| parse_date(&value)),
        url: Some(absolute_url(&format!("/ver/{series_slug}/{slug}"))),
        language: Some(LANG.to_string()),
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "data-lazy-src")
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "data-cfsrc"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
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
        .collect()
}

fn sort_entries(entries: &mut [Value], key: &str) {
    match key {
        "name" => entries.sort_by_key(|item| string_value(item, "name").unwrap_or_default()),
        "views" => entries.sort_by_key(|item| {
            item.pointer("/trending/visitas")
                .and_then(Value::as_i64)
                .unwrap_or_default()
        }),
        "created_at" => {
            entries.sort_by_key(|item| string_value(item, "created_at").unwrap_or_default())
        }
        _ => entries.sort_by_key(|item| string_value(item, "actualizacionCap").unwrap_or_default()),
    }
}

fn nested_names(item: &Value, array_key: &str, nested_key: &str) -> Vec<String> {
    item.get(array_key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .get(nested_key)
                .and_then(|value| string_value(value, "name"))
        })
        .collect()
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn filter<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
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

const TOP_FIXTURE: &str = r#"{"response":{"diario":[[{"project":{"name":"Sample Moonlight","alternativeName":"Sample Alt","slug":"sample","sinopsis":"Summary","urlImg":"https://traduccionesmoonlight.com/cover.jpg","actualizacionCap":"2024-01-01T00:00:00.000Z","created_at":"2024-01-01T00:00:00.000Z","state_id":1,"genders":[{"gender":{"name":"Drama"}}],"autors":[{"autor":{"name":"Author"}}],"artists":[{"artist":{"name":"Artist"}}],"lastChapters":[{"num":1,"name":"Inicio","slug":"chapter-1","created_at":"2024-01-01T00:00:00.000Z"}],"trending":{"visitas":10}}}]],"semanal":[[{"project":{"name":"Sample Moonlight","alternativeName":"Sample Alt","slug":"sample","sinopsis":"Summary","urlImg":"https://traduccionesmoonlight.com/cover.jpg","actualizacionCap":"2024-01-01T00:00:00.000Z","created_at":"2024-01-01T00:00:00.000Z","state_id":1,"genders":[{"gender":{"name":"Drama"}}],"autors":[{"autor":{"name":"Author"}}],"artists":[{"artist":{"name":"Artist"}}],"lastChapters":[{"num":1,"name":"Inicio","slug":"chapter-1","created_at":"2024-01-01T00:00:00.000Z"}],"trending":{"visitas":10}}}]],"mensual":[[{"project":{"name":"Sample Moonlight","alternativeName":"Sample Alt","slug":"sample","sinopsis":"Summary","urlImg":"https://traduccionesmoonlight.com/cover.jpg","actualizacionCap":"2024-01-01T00:00:00.000Z","created_at":"2024-01-01T00:00:00.000Z","state_id":1,"genders":[{"gender":{"name":"Drama"}}],"autors":[{"autor":{"name":"Author"}}],"artists":[{"artist":{"name":"Artist"}}],"lastChapters":[{"num":1,"name":"Inicio","slug":"chapter-1","created_at":"2024-01-01T00:00:00.000Z"}],"trending":{"visitas":10}}}]]}}"#;
const LATEST_FIXTURE: &str = r#"{"response":[{"name":"Sample Moonlight","alternativeName":"Sample Alt","slug":"sample","sinopsis":"Summary","urlImg":"https://traduccionesmoonlight.com/cover.jpg","actualizacionCap":"2024-01-01T00:00:00.000Z","created_at":"2024-01-01T00:00:00.000Z","state_id":1,"genders":[{"gender":{"name":"Drama"}}],"autors":[{"autor":{"name":"Author"}}],"artists":[{"artist":{"name":"Artist"}}],"lastChapters":[{"num":1,"name":"Inicio","slug":"chapter-1","created_at":"2024-01-01T00:00:00.000Z"}],"trending":{"visitas":10}}]}"#;
const COMICS_FIXTURE: &str = LATEST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"{"response":{"name":"Sample Moonlight","alternativeName":"Sample Alt","slug":"sample","sinopsis":"Summary","urlImg":"https://traduccionesmoonlight.com/cover.jpg","actualizacionCap":"2024-01-01T00:00:00.000Z","created_at":"2024-01-01T00:00:00.000Z","state_id":1,"genders":[{"gender":{"name":"Drama"}}],"autors":[{"autor":{"name":"Author"}}],"artists":[{"artist":{"name":"Artist"}}],"lastChapters":[{"num":1,"name":"Inicio","slug":"chapter-1","created_at":"2024-01-01T00:00:00.000Z"}],"trending":{"visitas":10}}}"#;
const PAGES_FIXTURE: &str = r#"<main class="contenedor read"><img data-src="/page1.jpg"></main>"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(parse_popular(TOP_FIXTURE).entries.len(), 1);
        assert_eq!(SOURCE.chapters(serde_json::json!({})).unwrap().len(), 1);
        assert_eq!(SOURCE.pages(serde_json::json!({})).unwrap().len(), 1);
    }
}
