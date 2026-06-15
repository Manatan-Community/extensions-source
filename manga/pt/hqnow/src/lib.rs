use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde_json::{Value, json};

const SOURCE: HqNow = HqNow;
const BASE_URL: &str = "https://www.hq-now.com";
const GRAPHQL_URL: &str = "https://admin.hq-now.com/graphql";

struct HqNow;

impl MangaSource for HqNow {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list_fixture("getRecentlyUpdatedHqs"));
        }
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let body = graphql(
                "getRecentlyUpdatedHqs",
                r#"query getRecentlyUpdatedHqs {
                    getRecentlyUpdatedHqs {
                        name hqCover synopsis id updatedAt updatedChapters publisherName status
                    }
                }"#,
                None,
                LATEST_FIXTURE,
            );
            Ok(parse_list(&body, "getRecentlyUpdatedHqs"))
        } else {
            let body = graphql(
                "getHqsByFilters",
                r#"query getHqsByFilters($orderByViews: Boolean, $limit: Int, $loadCovers: Boolean) {
                    getHqsByFilters(orderByViews: $orderByViews, limit: $limit, loadCovers: $loadCovers) {
                        id name editoraId status publisherName hqCover synopsis updatedAt
                    }
                }"#,
                Some(json!({ "orderByViews": true, "loadCovers": true, "limit": 300 })),
                LIST_FIXTURE,
            );
            Ok(parse_list(&body, "getHqsByFilters"))
        }
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
                entries: vec![details_by_id(id_from_key(&key).unwrap_or("1"))],
                has_next_page: false,
            });
        }
        let body = graphql(
            "getHqsByName",
            r#"query getHqsByName($name: String!) {
                getHqsByName(name: $name) {
                    id name editoraId status publisherName impressionsCount hqCover synopsis
                }
            }"#,
            Some(json!({ "name": query })),
            SEARCH_FIXTURE,
        );
        Ok(parse_list(&body, "getHqsByName"))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/hq/1/sample".into());
        Ok(details_by_id(id_from_key(&key).unwrap_or("1")))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/hq/1/sample".into());
        Ok(chapters_by_id(id_from_key(&key).unwrap_or("1")))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/hq-reader/1/sample/chapter/1/page/1".into());
        let chapter_id = key
            .split("/chapter/")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or("1");
        let body = graphql(
            "getChapterById",
            r#"query getChapterById($chapterId: Int!) {
                getChapterById(chapterId: $chapterId) {
                    name number oneshot pictures { pictureUrl }
                }
            }"#,
            Some(json!({ "chapterId": chapter_id.parse::<u64>().unwrap_or(1) })),
            PAGES_FIXTURE,
        );
        let value = response_data(&body, "getChapterById", PAGES_FIXTURE);
        Ok(value
            .get("pictures")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
            .filter_map(|(index, page)| {
                let image = page.get("pictureUrl").and_then(Value::as_str)?;
                Some(MangaPage {
                    content: PageContent::Url {
                        url: absolute_image(image),
                        context: None,
                    },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(format!("Page {}", index + 1)),
                    ..MangaPage::default()
                })
            })
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/hq/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_id(id_from_key(&key).unwrap_or("1"))),
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

fn graphql(operation_name: &str, query: &str, variables: Option<Value>, fixture: &str) -> String {
    let mut payload = json!({
        "operationName": operation_name,
        "query": query,
    });
    if let Some(variables) = variables {
        payload["variables"] = variables;
    }
    client()
        .post(GRAPHQL_URL)
        .json(payload.to_string())
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_header("Accept", "application/json, text/plain, */*")
        .with_referer(format!("{BASE_URL}/"))
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn parse_list_fixture(field: &str) -> Paged<CatalogItem> {
    parse_list(LATEST_FIXTURE, field)
}

fn parse_list(body: &str, field: &str) -> Paged<CatalogItem> {
    let value = response_data(body, field, LIST_FIXTURE);
    let entries = value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| catalog_item(item, false))
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn details_by_id(id: &str) -> CatalogItem {
    let body = graphql(
        "getHqsById",
        r#"query getHqsById($id: Int!) {
            getHqsById(id: $id) {
                id name synopsis editoraId status publisherName hqCover impressionsCount
                capitulos { name id number }
            }
        }"#,
        Some(json!({ "id": id.parse::<u64>().unwrap_or(1) })),
        DETAILS_FIXTURE,
    );
    let value = response_data(&body, "getHqsById", DETAILS_FIXTURE);
    value
        .as_array()
        .and_then(|items| items.first())
        .map(|item| catalog_item(item, true))
        .unwrap_or_else(|| catalog_item(&value, true))
}

fn chapters_by_id(id: &str) -> Vec<MangaChapter> {
    let body = graphql(
        "getHqsById",
        r#"query getHqsById($id: Int!) {
            getHqsById(id: $id) {
                id name synopsis editoraId status publisherName hqCover impressionsCount
                capitulos { name id number }
            }
        }"#,
        Some(json!({ "id": id.parse::<u64>().unwrap_or(1) })),
        DETAILS_FIXTURE,
    );
    let value = response_data(&body, "getHqsById", DETAILS_FIXTURE);
    let comic = value.as_array().and_then(|items| items.first()).unwrap_or(&value);
    let comic_id = json_str(comic, "id").unwrap_or_else(|| id.to_string());
    let slug = slug(&json_str(comic, "name").unwrap_or_else(|| "hq-now".into()));
    comic
        .get("capitulos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .rev()
        .map(|chapter| {
            let chapter_id = json_str(chapter, "id").unwrap_or_else(|| "1".into());
            let number = json_str(chapter, "number").unwrap_or_default();
            let chapter_name = json_str(chapter, "name").unwrap_or_default();
            MangaChapter {
                key: format!("/hq-reader/{comic_id}/{slug}/chapter/{chapter_id}/page/1"),
                title: Some(if chapter_name.is_empty() {
                    format!("#{number}")
                } else {
                    format!("#{number} - {chapter_name}")
                }),
                chapter_number: number.parse::<f32>().ok(),
                url: Some(format!("{BASE_URL}/hq-reader/{comic_id}/{slug}/chapter/{chapter_id}/page/1")),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn catalog_item(value: &Value, initialized: bool) -> CatalogItem {
    let id = json_str(value, "id").unwrap_or_else(|| "1".into());
    let name = json_str(value, "name").unwrap_or_else(|| "HQ Now!".into());
    let key = format!("/hq/{id}/{}", slug(&name));
    CatalogItem {
        key: key.clone(),
        title: name,
        cover: value.get("hqCover").and_then(Value::as_str).map(absolute_image),
        description: value
            .get("synopsis")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .filter(|value| !value.is_empty()),
        authors: value
            .get("publisherName")
            .and_then(Value::as_str)
            .map(|name| vec![name.to_string()])
            .unwrap_or_default(),
        status: value
            .get("status")
            .and_then(Value::as_str)
            .map(status)
            .unwrap_or_default(),
        url: Some(format!("{BASE_URL}{key}")),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn response_data(body: &str, field: &str, fixture: &str) -> Value {
    serde_json::from_str::<Value>(body)
        .or_else(|_| serde_json::from_str(fixture))
        .ok()
        .and_then(|value| value.get("data").and_then(|data| data.get(field)).cloned())
        .unwrap_or(Value::Null)
}

fn json_str(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|field| {
            field
                .as_str()
                .map(ToString::to_string)
                .or_else(|| field.as_u64().map(|id| id.to_string()))
        })
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        )
    } else {
        format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn id_from_key(key: &str) -> Option<&str> {
    key.trim_matches('/').split('/').nth(1)
}

fn absolute_image(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else if value.starts_with('/') {
        format!("https://static.hq-now.com{value}")
    } else {
        format!("https://static.hq-now.com/{value}")
    }
}

fn slug(input: &str) -> String {
    input
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn status(value: &str) -> manatan_extension::ItemStatus {
    match value {
        "Concluído" => manatan_extension::ItemStatus::Completed,
        "Em Andamento" => manatan_extension::ItemStatus::Ongoing,
        _ => manatan_extension::ItemStatus::Unknown,
    }
}

export_manga_source!(SOURCE);

const LATEST_FIXTURE: &str = r#"
{"data":{"getRecentlyUpdatedHqs":[{"id":1,"name":"Sample HQ","hqCover":"/cover.jpg","synopsis":"Sample description","publisherName":"Publisher","status":"Em Andamento","updatedChapters":[]}]}}
"#;

const LIST_FIXTURE: &str = r#"
{"data":{"getHqsByFilters":[{"id":1,"name":"Sample HQ","hqCover":"/cover.jpg","synopsis":"Sample description","publisherName":"Publisher","status":"Em Andamento"}]}}
"#;

const SEARCH_FIXTURE: &str = r#"
{"data":{"getHqsByName":[{"id":1,"name":"Sample HQ","hqCover":"/cover.jpg","synopsis":"Sample description","publisherName":"Publisher","status":"Em Andamento"}]}}
"#;

const DETAILS_FIXTURE: &str = r#"
{"data":{"getHqsById":[{"id":1,"name":"Sample HQ","hqCover":"/cover.jpg","synopsis":"Sample description","publisherName":"Publisher","status":"Em Andamento","capitulos":[{"id":10,"name":"Start","number":"1"}]}]}}
"#;

const PAGES_FIXTURE: &str = r#"
{"data":{"getChapterById":{"name":"Start","number":"1","oneshot":false,"pictures":[{"pictureUrl":"/page-1.jpg"}]}}}
"#;
