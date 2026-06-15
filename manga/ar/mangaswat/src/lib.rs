use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaSwat = MangaSwat;
const API_BASE_URL: &str = "https://meshmanga.com/v2/api/v2";

struct MangaSwat;

impl MangaSource for MangaSwat {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list(LIST_FIXTURE, base_url(&request)));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{API_BASE_URL}/series/releases?page_size=20&page={page}")
        } else {
            format!("{API_BASE_URL}/series/?order_by=-followers_count&page={page}")
        };
        let body = fetch_json_or_fixture(&request, &target, LIST_FIXTURE);
        Ok(parse_list(&body, base_url(&request)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let base = base_url(&request);
        if query.starts_with(&base) {
            let key = normalize_key(query);
            let body = fetch_json_or_fixture(
                &request,
                &format!("{API_BASE_URL}/series/{}", key.trim_start_matches('/')),
                DETAILS_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![parse_details(&body, key, base)],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            format!("{API_BASE_URL}/series/?order_by=-followers_count&page={page}")
        } else {
            format!(
                "{API_BASE_URL}/series/?search={}&page={page}",
                url::query_escape(query)
            )
        };
        let body = fetch_json_or_fixture(&request, &target, LIST_FIXTURE);
        Ok(parse_list(&body, base))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        let body = fetch_json_or_fixture(
            &request,
            &format!("{API_BASE_URL}/series/{}", key.trim_start_matches('/')),
            DETAILS_FIXTURE,
        );
        Ok(parse_details(&body, key, base_url(&request)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        let target = format!(
            "{API_BASE_URL}/chapters/?serie={}&order_by=-order&page_size=200",
            url::query_escape(key.trim_start_matches('/'))
        );
        let body = fetch_json_or_fixture(&request, &target, CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body, base_url(&request)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/chapters/10/sample-chapter/".to_string());
        let id = key
            .trim_start_matches("/chapters/")
            .split('/')
            .next()
            .unwrap_or(key.trim_matches('/'));
        let body = fetch_json_or_fixture(
            &request,
            &format!("{API_BASE_URL}/chapters/{id}"),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body, base_url(&request)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let base = base_url(&request);
        if input.starts_with(&format!("{base}/series/")) {
            let key = normalize_key(input);
            let body = fetch_json_or_fixture(
                &request,
                &format!("{API_BASE_URL}/series/{}", key.trim_start_matches('/')),
                DETAILS_FIXTURE,
            );
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, key, base)),
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

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("overrideBaseUrl"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("https://meshmanga.com")
        .trim_end_matches('/')
        .to_string()
}

fn fetch_json_or_fixture(request: &Value, target: &str, fixture: &str) -> String {
    let base = base_url(request);
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{base}/"))
        .with_cookies_for(&base)
        .with_webview_challenge_fallback()
        .get(target)
        .header("Accept", "application/json, text/plain, */*")
        .header("Origin", &base)
        .header("User-Agent", "ktor-client")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_list(body: &str, base: String) -> Paged<CatalogItem> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Paged::default();
    };
    let entries = root
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let id = entry
                .get("id")
                .or_else(|| entry.get("serie_id"))
                .and_then(Value::as_i64)?;
            let key = id.to_string();
            Some(CatalogItem {
                key: key.clone(),
                title: entry
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Manga")
                    .to_string(),
                cover: poster_url(entry),
                url: Some(format!("{base}/series/{key}")),
                language: Some("ar".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: root.get("next").is_some_and(|value| !value.is_null()),
    }
}

fn parse_details(body: &str, key: String, base: String) -> CatalogItem {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    CatalogItem {
        key: key.clone(),
        title: root
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Manga")
            .to_string(),
        cover: poster_url(&root),
        description: root
            .get("story")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        authors: person_name(root.get("author")).into_iter().collect(),
        artists: person_name(root.get("artist")).into_iter().collect(),
        tags: root
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| genre.get("name").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect(),
        status: root
            .get("status")
            .and_then(|status| status.get("name"))
            .and_then(Value::as_str)
            .map(parse_status)
            .unwrap_or(ItemStatus::Unknown),
        url: Some(format!("{base}/series/{}", key.trim_start_matches('/'))),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base: String) -> Vec<MangaChapter> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    root.get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let id = chapter.get("id").and_then(Value::as_i64)?;
            let slug = chapter.get("slug").and_then(Value::as_str).unwrap_or("");
            let key = format!("/chapters/{id}/{slug}/");
            Some(MangaChapter {
                key: key.clone(),
                title: chapter
                    .get("chapter")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                date_uploaded: chapter
                    .get("created_at")
                    .and_then(Value::as_str)
                    .and_then(manatan_shared::dates::parse_fixture_date),
                url: Some(format!("{base}/chapter/{id}")),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, base: String) -> Vec<MangaPage> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let mut pages = root
        .get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let image = page.get("image").and_then(Value::as_str)?;
            let order = page.get("order").and_then(Value::as_u64).unwrap_or(0);
            Some((order, image.to_string()))
        })
        .collect::<Vec<_>>();
    pages.sort_by_key(|(order, _)| *order);
    pages
        .into_iter()
        .enumerate()
        .map(|(index, (_, image))| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(&base)),
            },
            headers: manga::image_headers(&base),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn poster_url(value: &Value) -> Option<String> {
    value
        .get("poster")
        .and_then(|poster| poster.get("medium"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn person_name(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(name) => Some(name.clone()),
        Value::Object(object) => object
            .get("name")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        _ => None,
    }
}

fn parse_status(value: &str) -> ItemStatus {
    match value.to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn normalize_key(input: &str) -> String {
    input
        .split("/series/")
        .nth(1)
        .unwrap_or(input)
        .trim_matches('/')
        .to_string()
}

const LIST_FIXTURE: &str = r#"{"results":[{"id":1,"title":"Sample Manga","poster":{"medium":"https://img.example/cover.jpg"}}],"next":null}"#;
const DETAILS_FIXTURE: &str = r#"{"title":"Sample Manga","story":"Sample description.","author":{"name":"Writer"},"artist":"Artist","genres":[{"name":"Drama"}],"status":{"name":"completed"},"poster":{"medium":"https://img.example/cover.jpg"}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"results":[{"id":10,"slug":"sample-chapter","chapter":"Chapter 1","created_at":"2024-01-01T00:00:00Z"}],"next":null}"#;
const PAGES_FIXTURE: &str = r#"{"images":[{"image":"https://img.example/2.jpg","order":2},{"image":"https://img.example/1.jpg","order":1}]}"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mangaswat_api() {
        let listing = parse_list(LIST_FIXTURE, "https://meshmanga.com".to_string());
        assert_eq!(listing.entries[0].key, "1");

        let details = parse_details(
            DETAILS_FIXTURE,
            "1".to_string(),
            "https://meshmanga.com".to_string(),
        );
        assert_eq!(details.status, ItemStatus::Completed);
        assert_eq!(details.authors, vec!["Writer"]);

        let chapters = parse_chapters(CHAPTERS_FIXTURE, "https://meshmanga.com".to_string());
        assert_eq!(chapters[0].key, "/chapters/10/sample-chapter/");

        let pages = parse_pages(PAGES_FIXTURE, "https://meshmanga.com".to_string());
        assert_eq!(pages.len(), 2);
    }
}
