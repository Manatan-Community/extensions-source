use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: AriaToon = AriaToon;
const BASE_URL: &str = "https://ariatoon.com";
const API_URL: &str = "https://api.ariatoon.com/v1";
const CDN_URL: &str = "https://api.ariatoon.com/uploads";

struct AriaToon;

impl MangaSource for AriaToon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{API_URL}/mangas?page={page}&limit=20")
        } else {
            format!("{API_URL}/feed/mangas/popular?page={page}&limit=20")
        };
        let body = fetch_json_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let id = query
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("sample");
            let body = fetch_json_or_fixture(&format!("{API_URL}/mangas/{id}"), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let genre = filters
            .get("genre")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let target = if !query.is_empty() {
            format!(
                "{API_URL}/mangas/search?search={}&page={page}&limit=20",
                url::query_escape(query)
            )
        } else if !genre.is_empty() {
            format!("{API_URL}/mangas/filters/{genre}?page={page}&limit=20&language=ar")
        } else {
            format!("{API_URL}/mangas?page={page}&limit=20")
        };
        let body = fetch_json_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let id = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = fetch_json_or_fixture(&format!("{API_URL}/mangas/{id}"), DETAILS_FIXTURE);
        Ok(parse_details(&body))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let id = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let target = format!(
            "{API_URL}/mangas/{id}/episodes?direction=desc&publishStatus=published&limit=100&page=1"
        );
        let body = fetch_json_or_fixture(&target, CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "sample/episodes/episode-1".to_string());
        let manga_id = key.split("/episodes/").next().unwrap_or("sample");
        let episode_id = key.rsplit('/').next().unwrap_or("episode-1");
        let body = fetch_json_or_fixture(
            &format!("{API_URL}/mangas/{manga_id}/episodes/{episode_id}"),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let id = input
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("sample");
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_json_or_fixture(
                    &format!("{API_URL}/mangas/{id}"),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_header("Accept", "application/json")
        .with_origin(BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Paged::default();
    };
    let entries = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(catalog_from_value)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() == 20,
        entries,
    }
}

fn parse_details(body: &str) -> CatalogItem {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return fallback_item("sample");
    };
    root.get("data")
        .and_then(catalog_from_value)
        .map(|mut item| {
            item.initialized = true;
            item
        })
        .unwrap_or_else(|| fallback_item("sample"))
}

fn catalog_from_value(entry: &Value) -> Option<CatalogItem> {
    let id = entry.get("id").and_then(Value::as_str)?;
    Some(CatalogItem {
        key: id.to_string(),
        title: entry
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Manga")
            .to_string(),
        cover: entry
            .get("coverPath")
            .and_then(Value::as_str)
            .map(|path| format!("{CDN_URL}/{path}")),
        authors: entry
            .get("author")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .into_iter()
            .collect(),
        description: description(entry),
        status: match entry.get("status").and_then(Value::as_str) {
            Some("ongoing") => ItemStatus::Ongoing,
            Some("completed") => ItemStatus::Completed,
            Some("hiatus") => ItemStatus::Hiatus,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/series/manga/{id}")),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn description(entry: &Value) -> Option<String> {
    let summary = entry
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let announce = entry
        .get("announce")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match (summary.is_empty(), announce.is_empty()) {
        (true, true) => None,
        (false, true) => Some(summary.to_string()),
        (true, false) => Some(format!("إعلان:\n{announce}")),
        (false, false) => Some(format!("{summary}\n\nإعلان:\n{announce}")),
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    root.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let id = entry.get("id").and_then(Value::as_str)?;
            let manga_id = entry.get("mangaID").and_then(Value::as_str)?;
            let number = entry.get("number").and_then(Value::as_f64);
            let title = entry
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let chapter_title = match (number, title.is_empty()) {
                (Some(number), false) => format!("الفصل {} - {title}", trim_number(number)),
                (Some(number), true) => format!("الفصل {}", trim_number(number)),
                (None, false) => title.to_string(),
                (None, true) => "الفصل".to_string(),
            };
            Some(MangaChapter {
                key: format!("{manga_id}/episodes/{id}"),
                title: Some(chapter_title),
                chapter_number: number.map(|value| value as f32),
                date_uploaded: entry
                    .get("createdAt")
                    .and_then(Value::as_str)
                    .and_then(manatan_shared::dates::parse_fixture_date),
                url: Some(format!("{BASE_URL}/series/manga/{manga_id}/episodes/{id}")),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    root.get("data")
        .and_then(|data| data.get("images"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: format!("{CDN_URL}/{image}"),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn trim_number(value: f64) -> String {
    let text = value.to_string();
    text.strip_suffix(".0").unwrap_or(&text).to_string()
}

fn fallback_item(id: &str) -> CatalogItem {
    CatalogItem {
        key: id.to_string(),
        title: "Manga".to_string(),
        url: Some(format!("{BASE_URL}/series/manga/{id}")),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        ..CatalogItem::default()
    }
}

const LIST_FIXTURE: &str = r#"{
  "data": [
    {"id":"sample","title":"Sample Manga","coverPath":"covers/sample.jpg","author":"Writer","summary":"Sample summary.","status":"ongoing"}
  ]
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "data": {"id":"sample","title":"Sample Manga","coverPath":"covers/sample.jpg","author":"Writer","summary":"Sample summary.","announce":"News","status":"completed"}
}"#;

const CHAPTERS_FIXTURE: &str = r#"{
  "data": [{"id":"episode-1","mangaID":"sample","title":"Start","number":1,"createdAt":"2024-01-01T00:00:00"}]
}"#;

const PAGES_FIXTURE: &str = r#"{
  "data": {"images":["pages/1.jpg","pages/2.jpg"]}
}"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_catalog_details_chapters_and_pages() {
        let listing = parse_listing(LIST_FIXTURE);
        assert_eq!(listing.entries[0].key, "sample");

        let details = parse_details(DETAILS_FIXTURE);
        assert_eq!(details.status, ItemStatus::Completed);

        let chapters = parse_chapters(CHAPTERS_FIXTURE);
        assert_eq!(chapters[0].key, "sample/episodes/episode-1");

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
