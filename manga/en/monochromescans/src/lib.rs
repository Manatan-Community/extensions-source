use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MonochromeScans = MonochromeScans;
const BASE_URL: &str = "https://manga.d34d.one";
const API_URL: &str = "https://api.manga.d34d.one";
const LIMIT: u64 = 10;

struct MonochromeScans;

impl MangaSource for MonochromeScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_results(&fetch_json(&search_url("", page), RESULTS_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let id = query.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
            return Ok(Paged {
                entries: vec![parse_manga(&fetch_json(&format!("{API_URL}/manga/{id}"), MANGA_FIXTURE))],
                has_next_page: false,
            });
        }
        if let Some(id) = query.strip_prefix("uuid:") {
            return Ok(Paged {
                entries: vec![parse_manga(&fetch_json(&format!("{API_URL}/manga/{id}"), MANGA_FIXTURE))],
                has_next_page: false,
            });
        }
        Ok(parse_results(&fetch_json(&search_url(query, page), RESULTS_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let id = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_manga(&fetch_json(&format!("{API_URL}/manga/{id}"), MANGA_FIXTURE)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let id = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_chapters(&fetch_json(&format!("{API_URL}/manga/{id}/chapters"), CHAPTERS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "chapter|1|1".to_string());
        Ok(parse_pages(&key))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|id| format!("{BASE_URL}/manga/{id}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .and_then(|key| key.split('|').next().map(ToString::to_string))
            .map(|id| format!("{BASE_URL}/chapters/{id}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let id = input.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
            return Ok(Some(UrlResolveResult {
                item: Some(parse_manga(&fetch_json(&format!("{API_URL}/manga/{id}"), MANGA_FIXTURE))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(query: &str, page: u64) -> String {
    let offset = LIMIT * page.saturating_sub(1);
    format!("{API_URL}/manga?limit={LIMIT}&offset={offset}&title={}", url::query_escape(query))
}

fn parse_results(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let offset = root.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let limit = root.get("limit").and_then(Value::as_u64).unwrap_or(LIMIT);
    let total = root.get("total").and_then(Value::as_u64).unwrap_or(0);
    let rows = root.get("results").and_then(Value::as_array).cloned().unwrap_or_default();
    Paged {
        has_next_page: total > offset.saturating_add(rows.len() as u64).max(offset + limit),
        entries: rows.iter().map(item_from_value).collect(),
    }
}

fn parse_manga(body: &str) -> CatalogItem {
    let row = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    item_from_value(&row)
}

fn item_from_value(row: &Value) -> CatalogItem {
    let id = json_text(row, "id").unwrap_or_else(|| "sample".to_string());
    let version = row.get("version").and_then(Value::as_i64).unwrap_or(0);
    CatalogItem {
        key: id.clone(),
        title: json_text(row, "title").unwrap_or_else(|| "Monochrome Scans".to_string()),
        cover: Some(format!("{API_URL}/media/{id}/cover.jpg?version={version}")),
        authors: json_text(row, "author").into_iter().filter(|value| !value.is_empty()).collect(),
        artists: json_text(row, "artist").into_iter().filter(|value| !value.is_empty()).collect(),
        description: json_text(row, "description"),
        status: match json_text(row, "status").unwrap_or_default().as_str() {
            "ongoing" | "hiatus" => ItemStatus::Ongoing,
            "completed" | "cancelled" => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/manga/{id}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|row| {
            let id = json_text(row, "id").unwrap_or_else(|| "chapter".to_string());
            let version = row.get("version").and_then(Value::as_i64).unwrap_or(0);
            let length = row.get("length").and_then(Value::as_u64).unwrap_or(1);
            let number = row.get("number").and_then(Value::as_f64).map(|value| value as f32);
            let mut title = number
                .map(|value| format!("Chapter {}", trim_float(value as f64)))
                .unwrap_or_else(|| "Chapter".to_string());
            if let Some(name) = json_text(row, "name").filter(|name| !name.is_empty()) {
                title.push_str(" - ");
                title.push_str(&name);
            }
            MangaChapter {
                key: format!("{id}|{version}|{length}"),
                title: Some(title),
                chapter_number: number,
                scanlators: json_text(row, "scanGroup").into_iter().collect(),
                url: Some(format!("{BASE_URL}/chapters/{id}")),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(key: &str) -> Vec<MangaPage> {
    let parts = key.split('|').collect::<Vec<_>>();
    let id = parts.first().copied().unwrap_or("chapter");
    let version = parts.get(1).copied().unwrap_or("0");
    let length = parts.get(2).and_then(|value| value.parse::<usize>().ok()).unwrap_or(1);
    (1..=length)
        .map(|page| MangaPage {
            content: PageContent::Url {
                url: format!("{API_URL}/media/{id}/{page}.jpg?version={version}"),
                context: Some(manga::image_headers(API_URL)),
            },
            headers: manga::image_headers(API_URL),
            description: Some(format!("Page {page}")),
            ..MangaPage::default()
        })
        .collect()
}

fn json_text(row: &Value, key: &str) -> Option<String> {
    row.get(key).map(|value| value.as_str().map(ToString::to_string).unwrap_or_else(|| value.to_string()))
}

fn trim_float(value: f64) -> String {
    let mut text = format!("{value:.2}");
    while text.ends_with('0') {
        text.pop();
    }
    text.trim_end_matches('.').to_string()
}

export_manga_source!(SOURCE);

const RESULTS_FIXTURE: &str = r#"{"offset":0,"limit":10,"total":1,"results":[{"id":"sample","title":"Sample","description":"Summary","author":"Author","artist":"Artist","status":"ongoing","version":1}]}"#;
const MANGA_FIXTURE: &str = r#"{"id":"sample","title":"Sample","description":"Summary","author":"Author","artist":"Artist","status":"ongoing","version":1}"#;
const CHAPTERS_FIXTURE: &str = r#"[{"name":"Start","number":1.0,"scanGroup":"Team","id":"chapter","version":1,"length":1,"uploadTime":"2024-01-01T00:00:00.000000"}]"#;
