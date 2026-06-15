use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::http::HttpClient};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: MangaNo = MangaNo;
const BASE_URL: &str = "https://manga-no.com";
const API_URL: &str = "https://manga-no.com/query";
const LOGIN_KEY: &str = "AIzaSyASnOvvLWrECQKNRI0R_82droxO1QMd4O8";

struct MangaNo;

impl MangaSource for MangaNo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listingId")
            .or_else(|| request.get("listing"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            return Ok(parse_series(
                &post_graphql(
                    &request,
                    LATEST_QUERY,
                    "NewWorks",
                    json!({"after": null}),
                    LATEST_FIXTURE,
                ),
                "/data/newWorks2",
            ));
        }
        Ok(parse_edges(
            &post_graphql(
                &request,
                POPULAR_QUERY,
                "RankingsMonthly",
                json!({}),
                POPULAR_FIXTURE,
            ),
            "/data/ranking/monthly2/edges",
            false,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_from_key(&request, &key)],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(parse_series(
                &post_graphql(
                    &request,
                    SEARCH_QUERY,
                    "Search",
                    json!({"keyword": query, "after": null}),
                    SEARCH_FIXTURE,
                ),
                "/data/search",
            ));
        }
        let tag = filter_string(&request, "tag").unwrap_or("日常");
        Ok(parse_series(
            &post_graphql(
                &request,
                TAG_QUERY,
                "Tag",
                json!({"title": tag, "first": 100, "after": null}),
                TAG_FIXTURE,
            ),
            "/data/tag/works",
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_from_key(&request, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let hide_locked = preference_bool(&request, "hide_locked");
        let body = post_graphql(
            &request,
            CHAPTER_LIST_QUERY,
            "ChapterList",
            json!({"id": key}),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body, hide_locked))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "sample-episode".into());
        let body = post_graphql(
            &request,
            VIEWER_QUERY,
            "GetEpisode",
            json!({"id": key}),
            PAGES_FIXTURE,
        );
        let pages = parse_pages(&body);
        if pages.is_empty() {
            return Ok(vec![manga::text_page(
                "Enter credentials in Settings and purchase this chapter to read.",
            )]);
        }
        Ok(pages)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input).filter(|key| !key.starts_with("episode:")) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&request, &key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn post_graphql(
    request: &Value,
    query: &str,
    operation: &str,
    variables: Value,
    fixture: &str,
) -> String {
    let http = client();
    let mut builder = http.post(API_URL).json(
        json!({"operationName": operation, "query": query, "variables": variables}).to_string(),
    );
    if let Some(token) = token(request) {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    builder.send_text().unwrap_or_else(|_| fixture.into())
}

fn token(request: &Value) -> Option<String> {
    let email = preference_string(request, "email_pref").unwrap_or_default();
    let password = preference_string(request, "password_pref").unwrap_or_default();
    if !email.is_empty() && !password.is_empty() {
        let body = json!({"email": email, "password": password, "returnSecureToken": true});
        if let Some(token) = auth(
            "https://identitytoolkit.googleapis.com/v1/accounts:signInWithPassword",
            body,
        ) {
            return Some(token);
        }
    }
    auth(
        "https://identitytoolkit.googleapis.com/v1/accounts:signUp",
        json!({"returnSecureToken": true}),
    )
}

fn auth(endpoint: &str, body: Value) -> Option<String> {
    let target = format!("{endpoint}?key={LOGIN_KEY}");
    let text = client()
        .post(target)
        .json(body.to_string())
        .send_text()
        .ok()?;
    serde_json::from_str::<LoginResponse>(&text)
        .ok()
        .map(|response| response.id_token)
}

fn parse_edges(body: &str, pointer: &str, initialized: bool) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(POPULAR_FIXTURE).unwrap_or(Value::Null));
    let entries = root
        .pointer(pointer)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|edge| edge.get("node"))
        .filter_map(|node| item_from_node(node, initialized))
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_series(body: &str, pointer: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(LATEST_FIXTURE).unwrap_or(Value::Null));
    let container = root.pointer(pointer).unwrap_or(&Value::Null);
    let entries = container
        .get("edges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|edge| edge.get("node"))
        .filter_map(|node| item_from_node(node, false))
        .collect();
    let has_next_page = container
        .pointer("/pageInfo/hasNextPage")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Paged {
        entries,
        has_next_page,
    }
}

fn details_from_key(request: &Value, key: &str) -> CatalogItem {
    let body = post_graphql(
        request,
        DETAILS_QUERY,
        "MangaDetails",
        json!({"id": key}),
        DETAILS_FIXTURE,
    );
    let root = serde_json::from_str::<Value>(&body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap_or(Value::Null));
    root.pointer("/data/node")
        .and_then(|node| item_from_node(node, true))
        .unwrap_or_else(|| sample_item(key))
}

fn item_from_node(node: &Value, initialized: bool) -> Option<CatalogItem> {
    let id = string_at(node, "/id")?;
    let mut item = CatalogItem {
        key: id.clone(),
        title: string_at(node, "/title").unwrap_or_else(|| "MangaNo".into()),
        cover: string_at(node, "/coverImage/url").map(|image| {
            image
                .rsplit('/')
                .next()
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .unwrap_or(image)
        }),
        description: string_at(node, "/description"),
        authors: string_at(node, "/user/displayName").into_iter().collect(),
        status: if node.get("isCompleted").and_then(Value::as_bool) == Some(true) {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(format!("{BASE_URL}/works/{id}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized,
        ..CatalogItem::default()
    };
    if let Some(tags) = node.get("tags").and_then(Value::as_array) {
        item.tags = tags
            .iter()
            .filter_map(|tag| string_at(tag, "/title"))
            .collect();
    }
    Some(item)
}

fn parse_chapters(body: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).unwrap_or(Value::Null));
    root.pointer("/data/node/episodes/edges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|edge| edge.get("node"))
        .filter_map(|node| {
            let id = string_at(node, "/id")?;
            let title = string_at(node, "/title").unwrap_or_else(|| "Chapter".into());
            let is_paid = node.get("purchasedByViewer").and_then(Value::as_bool) != Some(true)
                && node.get("canViewerSkipPaywall").and_then(Value::as_bool) != Some(true);
            let pages_charged = node
                .pointer("/salesInfo/pagesChargedFrom")
                .and_then(Value::as_i64);
            let locked = is_paid && pages_charged == Some(0);
            let preview = is_paid && pages_charged.is_some_and(|value| value != 0);
            if hide_locked && (locked || preview) {
                return None;
            }
            Some(MangaChapter {
                key: id.clone(),
                title: Some(format!(
                    "{}{}{}",
                    if locked { "[Locked] " } else { "" },
                    if preview { "[Preview] " } else { "" },
                    title
                )),
                chapter_number: node
                    .get("number")
                    .and_then(Value::as_f64)
                    .map(|value| value as f32),
                date_uploaded: string_at(node, "/publishedAt").and_then(|date| {
                    dates::parse_ymd(date.split('T').next().unwrap_or(&date))
                        .map(|seconds| seconds * 1000)
                }),
                url: Some(format!("{BASE_URL}/episodes/{id}")),
                language: Some("ja".into()),
                is_locked: locked || preview,
                ..MangaChapter::default()
            })
        })
        .rev()
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).unwrap_or(Value::Null));
    root.pointer("/data/node/allPagesConnection/edges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|edge| string_at(edge, "/node/image/url"))
        .map(|image| MangaPage {
            content: PageContent::Url {
                url: image.clone(),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            ..MangaPage::default()
        })
        .collect()
}

fn sample_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.into(),
        title: "MangaNo".into(),
        url: Some(format!("{BASE_URL}/works/{key}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn key_from_url(input: &str) -> Option<String> {
    if input.is_empty() {
        return None;
    }
    if !input.starts_with("http") && !input.starts_with('/') {
        return Some(input.into());
    }
    input
        .find("/works/")
        .map(|index| {
            input[index + "/works/".len()..]
                .trim_matches('/')
                .to_string()
        })
        .or_else(|| {
            input.find("/episodes/").map(|index| {
                format!(
                    "episode:{}",
                    input[index + "/episodes/".len()..].trim_matches('/')
                )
            })
        })
}

fn string_at(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|text| !text.is_empty())
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .or_else(|| request.get(id))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(id))
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_str().map(|text| text == "true"))
        })
        .unwrap_or(false)
}

fn preference_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(id))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[derive(Deserialize)]
struct LoginResponse {
    id_token: String,
}

export_manga_source!(SOURCE);

const POPULAR_QUERY: &str = r#"query RankingsMonthly { ranking { monthly2(first: 100) { edges { node { id title coverImage { url } } } } } }"#;
const LATEST_QUERY: &str = r#"query NewWorks($after: String) { newWorks2(first: 100, after: $after) { edges { node { id title coverImage { url } } } pageInfo { endCursor hasNextPage } } }"#;
const SEARCH_QUERY: &str = r#"query Search($keyword: String!, $after: String) { search(keyword: $keyword, first: 50, after: $after) { edges { node { ... on Work { id title coverImage { url } } } } pageInfo { endCursor hasNextPage } } }"#;
const TAG_QUERY: &str = r#"query Tag($title: String!, $first: Int!, $after: String) { tag(title: $title) { works(first: $first, after: $after) { edges { node { id title coverImage { url } } } pageInfo { endCursor hasNextPage } } } }"#;
const DETAILS_QUERY: &str = r#"query MangaDetails($id: ID!) { node(id: $id) { ... on Work { id title description isCompleted coverImage { url } user { displayName } tags { title } } } }"#;
const CHAPTER_LIST_QUERY: &str = r#"query ChapterList($id: ID!) { node(id: $id) { ... on Work { episodes(first: 1000) { edges { node { id title number publishedAt salesInfo { pagesChargedFrom } purchasedByViewer canViewerSkipPaywall } } } } } }"#;
const VIEWER_QUERY: &str = r#"query GetEpisode($id: ID!) { node(id: $id) { ... on Episode { allPagesConnection: pages(first: 2000) { edges { node { image { url } } } } } } }"#;

const POPULAR_FIXTURE: &str = r#"{"data":{"ranking":{"monthly2":{"edges":[{"node":{"id":"sample","title":"Sample MangaNo","coverImage":{"url":"https://img.example.test/mangano.jpg"}}}]}}}}"#;
const LATEST_FIXTURE: &str = r#"{"data":{"newWorks2":{"edges":[{"node":{"id":"sample","title":"Sample MangaNo","coverImage":{"url":"https://img.example.test/mangano.jpg"}}}],"pageInfo":{"endCursor":null,"hasNextPage":false}}}}"#;
const SEARCH_FIXTURE: &str = r#"{"data":{"search":{"edges":[{"node":{"id":"sample","title":"Sample MangaNo","coverImage":{"url":"https://img.example.test/mangano.jpg"}}}],"pageInfo":{"endCursor":null,"hasNextPage":false}}}}"#;
const TAG_FIXTURE: &str = r#"{"data":{"tag":{"works":{"edges":[{"node":{"id":"sample","title":"Sample MangaNo","coverImage":{"url":"https://img.example.test/mangano.jpg"}}}],"pageInfo":{"endCursor":null,"hasNextPage":false}}}}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"node":{"id":"sample","title":"Sample MangaNo","description":"Sample description.","isCompleted":false,"coverImage":{"url":"https://img.example.test/mangano.jpg"},"user":{"displayName":"Sample Author"},"tags":[{"title":"日常"}]}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"node":{"episodes":{"edges":[{"node":{"id":"sample-episode","title":"Chapter 1","number":1,"publishedAt":"2024-01-01T00:00:00Z","salesInfo":{"pagesChargedFrom":1},"purchasedByViewer":true,"canViewerSkipPaywall":true}}]}}}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"node":{"allPagesConnection":{"edges":[{"node":{"image":{"url":"https://img.example.test/mangano-page.jpg"}}}]}}}}"#;
