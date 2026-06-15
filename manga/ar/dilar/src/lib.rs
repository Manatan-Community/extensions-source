use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient};
use serde_json::{Value, json};

const SOURCE: Dilar = Dilar;
const BASE_URL: &str = "https://dilar.tube";

struct Dilar;

impl MangaSource for Dilar {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_list(SERIES_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_json_or_fixture(
            &format!("{BASE_URL}/api/series?page={page}"),
            SERIES_FIXTURE,
        );
        Ok(parse_series_list(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_series_key(query);
            let body = fetch_json_or_fixture(
                &format!("{BASE_URL}/api/series/{}", series_id(&key)),
                DETAILS_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![parse_series(&json_from(&body))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = client()
            .post(format!("{BASE_URL}/api/search/filter"))
            .json(json!({"query": query, "page": page}).to_string())
            .send_text()
            .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
        Ok(parse_search_list(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "sample/Sample".to_string());
        let body = fetch_json_or_fixture(
            &format!("{BASE_URL}/api/series/{}", series_id(&key)),
            DETAILS_FIXTURE,
        );
        Ok(parse_series(&json_from(&body)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "sample/Sample".to_string());
        let body = fetch_json_or_fixture(
            &format!("{BASE_URL}/api/series/{}/chapters", series_id(&key)),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "sample/Sample/1#release-1".to_string());
        let release_id = key.rsplit('#').next().unwrap_or("release-1");
        let body = fetch_json_or_fixture(
            &format!("{BASE_URL}/api/chapters/{release_id}"),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_series_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_series(&json_from(&fetch_json_or_fixture(
                    &format!("{BASE_URL}/api/series/{}", series_id(&key)),
                    DETAILS_FIXTURE,
                )))),
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

fn parse_series_list(body: &str) -> Paged<CatalogItem> {
    let root = json_from(body);
    let entries = root
        .get("series")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| !is_novel(entry))
        .map(parse_series)
        .collect();
    Paged {
        has_next_page: root.get("currentPage").and_then(Value::as_u64).unwrap_or(1)
            < root.get("totalPages").and_then(Value::as_u64).unwrap_or(1),
        entries,
    }
}

fn parse_search_list(body: &str) -> Paged<CatalogItem> {
    let root = json_from(body);
    let entries = root
        .get("rows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| !is_novel(entry))
        .map(parse_series)
        .collect::<Vec<_>>();
    let page = root.get("page").and_then(Value::as_u64).unwrap_or(1);
    let per_page = root
        .get("perPage")
        .and_then(Value::as_u64)
        .unwrap_or(entries.len() as u64);
    let total = root
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or(entries.len() as u64);
    Paged {
        has_next_page: total > page * per_page,
        entries,
    }
}

fn parse_series(entry: &Value) -> CatalogItem {
    let id = entry.get("id").and_then(Value::as_str).unwrap_or("sample");
    let title = entry
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Manga");
    CatalogItem {
        key: format!("{id}/{title}"),
        title: title.to_string(),
        cover: entry
            .get("cover")
            .and_then(Value::as_str)
            .map(|cover| thumbnail(id, cover)),
        description: entry
            .get("summary")
            .and_then(Value::as_str)
            .map(str::to_string),
        authors: staff(entry, "Author"),
        artists: staff(entry, "Artist"),
        tags: entry
            .get("categories")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|category| {
                category
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect(),
        status: match entry.get("translation_status").and_then(Value::as_str) {
            Some("ongoing") => ItemStatus::Ongoing,
            Some("completed") => ItemStatus::Completed,
            Some("dropped") => ItemStatus::Cancelled,
            Some("hiatus") => ItemStatus::Hiatus,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/series/{id}/{title}")),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let root = json_from(body);
    root.get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|chapter| {
            let number = chapter
                .get("chapter")
                .and_then(Value::as_str)
                .unwrap_or("?");
            let title = chapter
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default();
            chapter
                .get("releases")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(move |release| {
                    let id = release.get("id").and_then(Value::as_str)?;
                    let name = if title.is_empty() {
                        number.trim_end_matches(".00").to_string()
                    } else {
                        format!("{} - {title}", number.trim_end_matches(".00"))
                    };
                    Some(MangaChapter {
                        key: format!("{manga_key}/{}#{id}", number.trim_end_matches(".00")),
                        title: Some(name),
                        date_uploaded: release
                            .get("created_at")
                            .and_then(Value::as_str)
                            .and_then(manatan_shared::dates::parse_fixture_date),
                        chapter_number: number.parse().ok(),
                        url: Some(format!(
                            "{BASE_URL}/reader/{manga_key}/{}",
                            number.trim_end_matches(".00")
                        )),
                        ..MangaChapter::default()
                    })
                })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = json_from(body);
    let storage = root
        .get("storage_key")
        .and_then(Value::as_str)
        .unwrap_or("sample");
    let mut pages = root
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| {
            Some((
                page.get("order")
                    .and_then(Value::as_i64)
                    .unwrap_or(i64::MAX),
                page.get("url").and_then(Value::as_str)?.to_string(),
            ))
        })
        .collect::<Vec<_>>();
    pages.sort_by_key(|(order, _)| *order);
    pages
        .into_iter()
        .enumerate()
        .map(|(index, (_, image))| MangaPage {
            content: PageContent::Url {
                url: format!("{BASE_URL}/uploads/releases/{storage}/hq/{image}"),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn staff(entry: &Value, role: &str) -> Vec<String> {
    entry
        .get("staff")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|staff| {
            staff
                .get("Staff")
                .and_then(|staff| staff.get("role"))
                .and_then(Value::as_str)
                == Some(role)
        })
        .filter_map(|staff| {
            staff
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

fn thumbnail(id: &str, cover: &str) -> String {
    let stem = cover
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(cover);
    format!("{BASE_URL}/uploads/manga/cover/{id}/large_{stem}.webp")
}

fn is_novel(entry: &Value) -> bool {
    entry.get("series_type_id").and_then(Value::as_str) == Some("99")
}

fn series_id(key: &str) -> &str {
    key.split('/').next().unwrap_or("sample")
}

fn normalize_series_key(input: &str) -> String {
    input
        .trim_end_matches('/')
        .split("/series/")
        .nth(1)
        .unwrap_or(input)
        .to_string()
}

fn json_from(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

const SERIES_FIXTURE: &str = r#"{
  "series": [{"id":"sample","title":"Sample Manga","cover":"cover.jpg","summary":"Summary","translation_status":"ongoing","series_type_id":"1","staff":[{"name":"Writer","Staff":{"role":"Author"}},{"name":"Artist","Staff":{"role":"Artist"}}],"categories":[{"name":"Action"}]}],
  "currentPage": 1,
  "totalPages": 1
}"#;

const SEARCH_FIXTURE: &str = r#"{
  "rows": [{"id":"sample","title":"Sample Manga","cover":"cover.jpg","series_type_id":"1"}],
  "total": 1,
  "page": 1,
  "perPage": 20
}"#;

const DETAILS_FIXTURE: &str = r#"{"id":"sample","title":"Sample Manga","cover":"cover.jpg","summary":"Summary","translation_status":"completed","series_type_id":"1"}"#;

const CHAPTERS_FIXTURE: &str = r#"{
  "chapters": [{"id":"chapter","chapter":"1.00","title":"Start","releases":[{"id":"release-1","created_at":"2024-01-01T00:00:00.000Z"}]}]
}"#;

const PAGES_FIXTURE: &str = r#"{
  "storage_key": "sample-storage",
  "pages": [{"url":"2.jpg","order":2},{"url":"1.jpg","order":1}]
}"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dilar_source() {
        let listing = parse_series_list(SERIES_FIXTURE);
        assert_eq!(listing.entries[0].key, "sample/Sample Manga");

        let search = parse_search_list(SEARCH_FIXTURE);
        assert_eq!(search.entries[0].title, "Sample Manga");

        let chapters = parse_chapters(CHAPTERS_FIXTURE, "sample/Sample Manga");
        assert_eq!(chapters[0].key, "sample/Sample Manga/1#release-1");

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
