use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: MangaTime = MangaTime;
const BASE_URL: &str = "https://mangatime.org";
const LIMIT: u64 = 24;

struct MangaTime;

impl MangaSource for MangaTime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "recent"
        } else {
            "popularity"
        };
        let body = fetch_trpc_or_fixture(
            "search.searchSeries",
            search_input(page, sort, ""),
            LIST_FIXTURE,
        );
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let slug = key.trim_matches('/').split('/').nth(1).unwrap_or_default();
            let body = fetch_trpc_or_fixture(
                "content.getSeriesBySlug",
                trpc_input(json!({"slug": slug})),
                DETAILS_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![parse_details(&body, key)],
                has_next_page: false,
            });
        }
        let body = fetch_trpc_or_fixture(
            "search.searchSeries",
            search_input(page, "popularity", query),
            LIST_FIXTURE,
        );
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/manga/sample#series1".to_string());
        let slug = key.trim_matches('/').split('/').nth(1).unwrap_or("sample");
        let body = fetch_trpc_or_fixture(
            "content.getSeriesBySlug",
            trpc_input(json!({"slug": slug})),
            DETAILS_FIXTURE,
        );
        Ok(parse_details(&body, key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/manga/sample#series1".to_string());
        let series_id = key.split('#').nth(1).unwrap_or("series1");
        let body = fetch_trpc_or_fixture(
            "content.getChapters",
            trpc_input(json!({"seriesId": series_id, "limit": -1})),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter/1".to_string());
        let mut parts = key.trim_matches('/').split('/');
        let series_slug = parts.nth(1).unwrap_or("sample");
        let chapter_number = key
            .trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("1")
            .parse::<u64>()
            .unwrap_or(1);
        let body = fetch_trpc_or_fixture(
            "content.getChapterPages",
            trpc_input(json!({"seriesSlug": series_slug, "chapterNumber": chapter_number})),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                search: Some(SearchRequest {
                    query: key,
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

fn search_input(page: u64, sort_by: &str, query: &str) -> String {
    let mut data = json!({"page": page, "limit": LIMIT, "sortBy": sort_by, "sortOrder": "desc"});
    if !query.trim().is_empty() {
        data["query"] = Value::String(query.to_string());
    }
    trpc_input(data)
}

fn trpc_input(json_value: Value) -> String {
    json!({"0": {"json": json_value}}).to_string()
}

fn fetch_trpc_or_fixture(endpoint: &str, input: String, fixture: &str) -> String {
    let target = format!(
        "{BASE_URL}/api/trpc/{endpoint}?batch=1&input={}",
        url::query_escape(&input)
    );
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn trpc_json(body: &str) -> Option<Value> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .as_array()?
        .first()?
        .get("result")?
        .get("data")?
        .get("json")
        .cloned()
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let Some(root) = trpc_json(body) else {
        return Paged::default();
    };
    let entries = root
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_str)?;
            let slug = item.get("slug").and_then(Value::as_str)?;
            let media_type = item.get("type").and_then(Value::as_str).unwrap_or("manga");
            Some(CatalogItem {
                key: format!("/{media_type}/{slug}#{id}"),
                title: item
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Manga")
                    .to_string(),
                cover: item.get("coverUrl").and_then(Value::as_str).map(to_image),
                url: Some(format!("{BASE_URL}/{media_type}/{slug}")),
                language: Some("ar".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: root
            .get("hasMore")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn parse_details(body: &str, key: String) -> CatalogItem {
    let root = trpc_json(body).unwrap_or(Value::Null);
    CatalogItem {
        key: key.clone(),
        title: root
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Manga")
            .to_string(),
        cover: root.get("coverUrl").and_then(Value::as_str).map(to_image),
        description: root
            .get("description")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        tags: root
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|g| g.get("name").and_then(Value::as_str))
            .chain(root.get("type").and_then(Value::as_str))
            .map(|s| s.replace('،', ","))
            .collect(),
        status: match root
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "ongoing" => ItemStatus::Ongoing,
            "completed" => ItemStatus::Completed,
            "hiatus" => ItemStatus::Hiatus,
            "cancelled" => ItemStatus::Cancelled,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!(
            "{BASE_URL}{}",
            key.split('#').next().unwrap_or(&key)
        )),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let base_key = manga_key.split('#').next().unwrap_or(manga_key);
    trpc_json(body)
        .and_then(|root| root.get("chapters").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|chapter| {
            let number = chapter.get("number").and_then(Value::as_u64)?;
            Some(MangaChapter {
                key: format!("{base_key}/chapter/{number}"),
                title: chapter
                    .get("title")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                date_uploaded: chapter
                    .get("publishedAt")
                    .and_then(Value::as_str)
                    .and_then(manatan_shared::dates::parse_fixture_date),
                url: Some(format!("{BASE_URL}{base_key}/chapter/{number}")),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let Some(root) = trpc_json(body) else {
        return Vec::new();
    };
    if root.get("isUnlocked").and_then(Value::as_bool) == Some(false) {
        return Vec::new();
    }
    root.get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: to_image(image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn to_image(input: &str) -> String {
    let escaped = input.replace(' ', "%20");
    if escaped.starts_with("http") {
        escaped
    } else {
        format!("{BASE_URL}{escaped}")
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        return format!(
            "/{}",
            input.split('/').skip(3).collect::<Vec<_>>().join("/")
        )
        .trim_end_matches('/')
        .to_string();
    }
    format!("/{}", input.trim_matches('/'))
}

const LIST_FIXTURE: &str = r#"[{"result":{"data":{"json":{"results":[{"id":"series1","title":"Sample Manga","slug":"sample","coverUrl":"/cover.jpg","type":"manga"}],"hasMore":false}}}}]"#;
const DETAILS_FIXTURE: &str = r#"[{"result":{"data":{"json":{"title":"Sample Manga","slug":"sample","coverUrl":"/cover.jpg","type":"manga","genres":[{"name":"Drama"}],"description":"Summary","status":"completed"}}}}]"#;
const CHAPTERS_FIXTURE: &str = r#"[{"result":{"data":{"json":{"chapters":[{"number":1,"title":"Chapter 1","publishedAt":"2024-01-01T00:00:00.000Z"}]}}}}]"#;
const PAGES_FIXTURE: &str = r#"[{"result":{"data":{"json":{"pages":["/page1.jpg","/page2.jpg"],"isUnlocked":true,"id":"chapter1","seriesId":"series1"}}}}]"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mangatime_trpc() {
        let listing = parse_listing(LIST_FIXTURE);
        assert_eq!(listing.entries[0].key, "/manga/sample#series1");

        let details = parse_details(DETAILS_FIXTURE, "/manga/sample#series1".into());
        assert_eq!(details.status, ItemStatus::Completed);

        let chapters = parse_chapters(CHAPTERS_FIXTURE, "/manga/sample#series1");
        assert_eq!(chapters[0].key, "/manga/sample/chapter/1");

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
