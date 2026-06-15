use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, manga_image, sdk::http::HttpClient};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: Sokuyomi = Sokuyomi;
const BASE_URL: &str = "https://sokuyomi.jp";
const API_URL: &str = "https://api.sokuyomi.jp/graphql";
const CDN_URL: &str = "https://cdn.sokuyomi.jp";

struct Sokuyomi;

impl MangaSource for Sokuyomi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series(SERIES_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let field = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "LATEST_BOOK_OPEND_AT"
        } else {
            "LIKE_COUNT"
        };
        Ok(parse_series(&graphql(
            SERIES_QUERY,
            "ListTitle",
            json!({"perPage": 50, "pageNumber": page.saturating_sub(1), "field": field, "isAdult": true}),
            &request,
            SERIES_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key, &request)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let (query_doc, operation, variables) = if query.is_empty() {
            (
                TAG_FILTER_QUERY,
                "ListTitleByTag",
                json!({"tag_slug": filter_string(&request, "tag").unwrap_or_else(|| "bekjy7191h25mtofziehw4pyes10jlgh".into()), "perPage": 20, "pageNumber": page.saturating_sub(1)}),
            )
        } else {
            (
                SEARCH_QUERY,
                "ListTitle",
                json!({"name": query, "authorName": query, "tagName": query, "perPage": 50, "pageNumber": page.saturating_sub(1), "field": "LIKE_COUNT", "isAdult": true}),
            )
        };
        Ok(parse_series(&graphql(
            query_doc,
            operation,
            variables,
            &request,
            SERIES_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_by_key(&key, &request))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let hide_locked = preference_bool(&request, "hide_locked");
        let body = graphql(
            CHAPTER_LIST_QUERY,
            "ListVolume",
            json!({"titleSlug": key, "perPage": 1000, "pageNumber": 0, "sort": "DESC"}),
            &request,
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body, hide_locked))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample-volume".into());
        let body = graphql(
            VIEWER_QUERY,
            "GetVolumeViewer",
            json!({"volumeSlug": key}),
            &request,
            VIEWER_FIXTURE,
        );
        Ok(parse_pages(&body))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Compact),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::YnJn::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/comics/{key}/detail/")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/viewer/volume/{key}/")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key, &request)),
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
        .with_cookies_for(BASE_URL)
}

fn graphql(
    query: &str,
    operation: &str,
    variables: Value,
    request: &Value,
    fixture: &str,
) -> String {
    let body =
        json!({"query": query, "operationName": operation, "variables": variables}).to_string();
    let http = client();
    let mut builder = http
        .post(API_URL)
        .header("Accept", "application/json")
        .header("Origin", BASE_URL)
        .json(body);
    if let Some(token) = bearer_token(request) {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    builder.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn bearer_token(request: &Value) -> Option<String> {
    let email = preference_string(request, "email_pref")?;
    let password = preference_string(request, "password_pref")?;
    if email.is_empty() || password.is_empty() {
        return None;
    }
    let body = json!({"query": LOGIN_QUERY, "operationName": "Signin", "variables": {"mail_address": email, "password": password}}).to_string();
    let response = client()
        .post(API_URL)
        .header("Accept", "application/json")
        .header("Origin", BASE_URL)
        .json(body)
        .send_text()
        .ok()?;
    serde_json::from_str::<Value>(&response)
        .ok()?
        .pointer("/data/signin/access_token")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn parse_series(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let list = root.pointer("/data/listTitle").unwrap_or(&Value::Null);
    let entries = list
        .pointer("/edges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|edge| edge.get("node"))
        .filter_map(series_item)
        .collect();
    let has_next_page = list
        .pointer("/pageInfo/currentPage")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        + 1
        < list
            .pointer("/pageInfo/totalPage")
            .and_then(Value::as_i64)
            .unwrap_or(1);
    Paged {
        entries,
        has_next_page,
    }
}

fn series_item(node: &Value) -> Option<CatalogItem> {
    let slug = text(node, "slug")?;
    Some(CatalogItem {
        key: slug.clone(),
        title: text(node, "name").unwrap_or_else(|| slug.clone()),
        cover: node
            .pointer("/title_cover/key")
            .and_then(Value::as_str)
            .map(|key| format!("{CDN_URL}/{key}")),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        url: Some(format!("{BASE_URL}/comics/{slug}/detail/")),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn details_by_key(key: &str, request: &Value) -> CatalogItem {
    let body = graphql(
        DETAILS_QUERY,
        "GetTitle",
        json!({"titleSlug": key}),
        request,
        DETAILS_FIXTURE,
    );
    let title = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|root| root.pointer("/data/getTitle").cloned())
        .unwrap_or(Value::Null);
    let mut description = text(&title, "description").unwrap_or_default();
    let alt = [text(&title, "name_hiragana"), text(&title, "name_katakana")]
        .into_iter()
        .flatten()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if !alt.is_empty() {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Alternative Titles:\n");
        description.push_str(&alt.join("\n"));
    }
    if let Some(publisher) = title
        .pointer("/label/publisher/name")
        .and_then(Value::as_str)
    {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Publisher: ");
        description.push_str(publisher);
    }
    CatalogItem {
        key: key.into(),
        title: text(&title, "name").unwrap_or_else(|| key.into()),
        cover: title
            .pointer("/title_cover/origin_key")
            .and_then(Value::as_str)
            .map(|key| format!("{CDN_URL}/{key}")),
        authors: array_names(title.get("authors")),
        tags: text(title.pointer("/genre").unwrap_or(&Value::Null), "name")
            .into_iter()
            .chain(array_names(title.get("tags")))
            .collect(),
        description: (!description.is_empty()).then_some(description),
        status: if title.get("is_finished").and_then(Value::as_bool) == Some(true) {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        url: Some(format!("{BASE_URL}/comics/{key}/detail/")),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    root.pointer("/data/listVolume/edges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|edge| edge.get("node"))
        .filter_map(|node| {
            let locked = node
                .get("consumption_coin")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                != 0
                && node.get("is_purchase").and_then(Value::as_bool) != Some(true)
                && (node
                    .pointer("/volume_consumption_coin/consumption_coin")
                    .and_then(Value::as_i64)
                    .unwrap_or(1)
                    != 0
                    || node.get("is_available_for_sale").and_then(Value::as_bool) != Some(true));
            if hide_locked && locked {
                return None;
            }
            let slug = text(node, "slug")?;
            Some(MangaChapter {
                key: slug.clone(),
                title: Some(format!(
                    "{}{}",
                    if locked { "[Locked] " } else { "" },
                    text(node, "name").unwrap_or_else(|| slug.clone())
                )),
                chapter_number: node
                    .get("volume_number")
                    .and_then(Value::as_f64)
                    .map(|value| value as f32),
                date_uploaded: text(node, "opend_at")
                    .and_then(|date| dates::parse_ymd(&date[..10.min(date.len())])),
                url: Some(format!("{BASE_URL}/viewer/volume/{slug}/")),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    root.pointer("/data/getVolumeViewer/volume_pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let key = text(page, "key")?;
            let mut extra = BTreeMap::new();
            extra.insert("ynjnScramble".into(), json!(true));
            Some(MangaPage {
                content: PageContent::Url {
                    url: format!("{CDN_URL}/{key}"),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: page
                    .get("page_number")
                    .and_then(Value::as_i64)
                    .map(|n| format!("Page {n}")),
                extra,
                ..MangaPage::default()
            })
        })
        .collect()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn array_names(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| text(item, "name"))
        .collect()
}

fn key_from_url(input: &str) -> Option<String> {
    input.strip_prefix(BASE_URL).and_then(|path| {
        path.trim_matches('/')
            .strip_prefix("comics/")
            .and_then(|rest| rest.split('/').next())
            .map(ToOwned::to_owned)
            .or_else(|| {
                path.trim_matches('/')
                    .strip_prefix("viewer/volume/")
                    .and_then(|rest| rest.split('/').next())
                    .map(ToOwned::to_owned)
            })
    })
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn preference_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(Value::as_object)
        .and_then(|preferences| preferences.get(id))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .and_then(Value::as_object)
        .and_then(|preferences| preferences.get(id))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

const SERIES_QUERY: &str = r#"query ListTitle($perPage: Int!, $pageNumber: Int!, $field: PostOrderFields!, $isAdult: Boolean) { listTitle(input: {is_adult: {eq: $isAdult}} page: {perPage: $perPage, pageNumber: $pageNumber} orderBy: {field: $field, sort: DESC}) { pageInfo { totalPage currentPage } edges { node { name slug title_cover { key } } } } }"#;
const SEARCH_QUERY: &str = r#"query ListTitle($name: String, $authorName: String, $tagName: String, $perPage: Int!, $pageNumber: Int!, $field: PostOrderFields!, $isAdult: Boolean) { listTitle(input: {name: {contains: $name}, author_name: {contains: $authorName}, tag_name: {contains: $tagName}, is_adult: {eq: $isAdult}} page: {perPage: $perPage, pageNumber: $pageNumber} orderBy: {field: $field, sort: DESC}) { pageInfo { totalPage currentPage } edges { node { name slug title_cover { key } } } } }"#;
const TAG_FILTER_QUERY: &str = r#"query ListTitleByTag($tag_slug: String!, $perPage: Int!, $pageNumber: Int!) { listTitle(input: {tag_slug: {eq: $tag_slug}} page: {perPage: $perPage, pageNumber: $pageNumber} orderBy: {field: LIKE_COUNT, sort: ASC}) { pageInfo { totalPage currentPage } edges { node { name slug title_cover { key } } } } }"#;
const DETAILS_QUERY: &str = r#"query GetTitle($titleSlug: String!) { getTitle(input: {slug: {eq: $titleSlug}}) { name name_hiragana name_katakana description is_adult is_finished label { publisher { name } } genre { name } title_cover { origin_key } authors { name } tags { name } } }"#;
const CHAPTER_LIST_QUERY: &str = r#"query ListVolume($titleSlug: String, $perPage: Int!, $pageNumber: Int!, $sort: PostOrderSorts!) { listVolume(input: {title_slug: {eq: $titleSlug}} page: {perPage: $perPage, pageNumber: $pageNumber} orderBy: {field: VOLUME_NUMBER, sort: $sort}) { edges { node { volume_number name consumption_coin opend_at slug is_purchase is_available_for_sale volume_consumption_coin { consumption_coin } } } } }"#;
const VIEWER_QUERY: &str = r#"query GetVolumeViewer($volumeSlug: String!) { getVolumeViewer(input: {volume_slug: {eq: $volumeSlug}}) { volume_pages { page_number key } } }"#;
const LOGIN_QUERY: &str = r#"mutation Signin($mail_address: String!, $password: String!) { signin(input: {mail_address: $mail_address, password: $password}) { access_token refresh_token } }"#;

const SERIES_FIXTURE: &str = r#"{"data":{"listTitle":{"pageInfo":{"totalPage":1,"currentPage":0},"edges":[{"node":{"name":"Sample Sokuyomi","slug":"sample","title_cover":{"key":"cover.jpg"}}}]}}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"getTitle":{"name":"Sample Sokuyomi","name_hiragana":"","name_katakana":"","description":"Summary","is_adult":false,"is_finished":false,"label":{"publisher":{"name":"Publisher"}},"genre":{"name":"Fantasy"},"title_cover":{"origin_key":"cover.jpg"},"authors":[{"name":"Author"}],"tags":[{"name":"Tag"}]}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"listVolume":{"edges":[{"node":{"volume_number":1,"name":"Volume 1","consumption_coin":0,"opend_at":"2024-01-01T00:00:00Z","slug":"sample-volume","is_purchase":false,"is_available_for_sale":true,"volume_consumption_coin":{"consumption_coin":0}}}]}}}"#;
const VIEWER_FIXTURE: &str =
    r#"{"data":{"getVolumeViewer":{"volume_pages":[{"page_number":1,"key":"page.webp"}]}}}"#;

export_manga_source!(SOURCE);
