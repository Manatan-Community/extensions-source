use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult,
    abi::{ExtensionResult, storage_get, storage_set},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Mangamo = Mangamo;
const BASE_URL: &str = "https://www.mangamo.com";
const FIREBASE_KEY: &str = "AIzaSyCU00GBJ4BPSK5owyaXvHZIXwMJ5Rq5F8c";
const FUNCTIONS_URL: &str = "https://us-central1-mangamoapp1.cloudfunctions.net/api";
const FIRESTORE_URL: &str =
    "https://firestore.googleapis.com/v1/projects/mangamoapp1/databases/(default)/documents";
const PAGE_SIZE: u64 = 50;

struct Mangamo;

impl MangaSource for Mangamo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_page(SERIES_FIXTURE, false));
        }
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let token = id_token(&request);
        let body = firestore_collection(
            "Series",
            series_fields(latest, preference_has(&request, "hide_coin_manga", "browse"), preference_has(&request, "exclusives_only", "browse")),
            Some(enabled_filter()),
            latest.then_some(vec![order_desc("updatedAt")]),
            page(&request),
            token.as_deref(),
            SERIES_FIXTURE,
        );
        Ok(parse_series_page(
            &body,
            preference_has(&request, "hide_coin_manga", "browse"),
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(series_id) = series_id_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_id(series_id, &request)],
                has_next_page: false,
            });
        }
        let lower = query.to_ascii_lowercase();
        let token = id_token(&request);
        let filter = if lower.is_empty() {
            enabled_filter()
        } else {
            and_filter(vec![
                enabled_filter(),
                field_filter("name_lowercase", "GREATER_THAN_OR_EQUAL", json!({"stringValue": lower})),
                field_filter("name_lowercase", "LESS_THAN_OR_EQUAL", json!({"stringValue": format!("{lower}\u{f8ff}")})),
            ])
        };
        let body = firestore_collection(
            "Series",
            series_fields(false, preference_has(&request, "hide_coin_manga", "search"), preference_has(&request, "exclusives_only", "search")),
            Some(filter),
            None,
            page(&request),
            token.as_deref(),
            SERIES_FIXTURE,
        );
        Ok(parse_series_page(
            &body,
            preference_has(&request, "hide_coin_manga", "search"),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(details_by_id(series_id_from_key(&key), &request))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let series_id = series_id_from_key(&key);
        let token = id_token(&request);
        let series = firestore_document(
            &format!("Series/{series_id}"),
            &["maxFreeChapterNumber", "maxMeteredReadingChapterNumber", "onlyTransactional"],
            token.as_deref(),
            SERIES_DETAIL_FIXTURE,
        );
        let chapters = firestore_collection(
            &format!("Series/{series_id}/chapters"),
            &["enabled", "id", "seriesId", "chapterNumber", "name", "createdAt", "onlyTransactional"],
            None,
            Some(vec![order_desc("chapterNumber")]),
            1,
            token.as_deref(),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(
            &series,
            &chapters,
            preference_has(&request, "hide_coin_manga", "chapters"),
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1:1".into());
        let (series, chapter) = chapter_parts(&key);
        let token = id_token(&request);
        let body = client()
            .post(format!("{FUNCTIONS_URL}/page/{series}/{chapter}"))
            .json(json!({ "idToken": token.unwrap_or_default() }).to_string())
            .send_text()
            .unwrap_or_else(|_| PAGES_FIXTURE.to_string());
        Ok(parse_pages(&body))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?;
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

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| catalog_url(&series_id_from_key(&key), "")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (series, chapter) = chapter_parts(&key);
            format!("{BASE_URL}/catalog?series={series}&chapter={chapter}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(series_id) = series_id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_id(series_id, &request)),
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
}

fn id_token(request: &Value) -> Option<String> {
    let user_token = request
        .get("preferences")
        .and_then(|prefs| prefs.get("user_token"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .or_else(|| storage_get("mangamo", "user_token").ok().flatten()?.as_str().map(ToOwned::to_owned))
        .or_else(|| {
            let body = client()
                .post(format!("https://identitytoolkit.googleapis.com/v1/accounts:signUp?key={FIREBASE_KEY}"))
                .json(r#"{"returnSecureToken":true}"#)
                .send_text()
                .ok()?;
            let local_id = json_value(&body).get("localId")?.as_str()?.to_string();
            let _ = storage_set("mangamo", "user_token", Value::String(local_id.clone()));
            Some(local_id)
        })?;
    let custom = client()
        .post(format!("{FUNCTIONS_URL}/v3/login"))
        .json(json!({ "purchaserInfo": { "originalAppUserId": user_token } }).to_string())
        .send_text()
        .ok()?;
    let custom_token = json_value(&custom).get("accessToken")?.as_str()?.to_string();
    let auth = client()
        .post(format!("https://identitytoolkit.googleapis.com/v1/accounts:signInWithCustomToken?key={FIREBASE_KEY}"))
        .json(json!({ "token": custom_token, "returnSecureToken": true }).to_string())
        .send_text()
        .ok()?;
    json_value(&auth).get("idToken")?.as_str().map(ToOwned::to_owned)
}

fn firestore_document(path: &str, fields: &[&str], token: Option<&str>, fixture: &str) -> String {
    let mut target = format!("{FIRESTORE_URL}/{path}");
    if !fields.is_empty() {
        target.push('?');
        target.push_str(
            &fields
                .iter()
                .map(|field| format!("mask.fieldPaths={field}"))
                .collect::<Vec<_>>()
                .join("&"),
        );
    }
    let http = client();
    let mut request = http.get(target);
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn firestore_collection(
    path: &str,
    fields: &[&str],
    filter: Option<Value>,
    order_by: Option<Vec<Value>>,
    page: u64,
    token: Option<&str>,
    fixture: &str,
) -> String {
    let (parent, collection) = path.rsplit_once('/').unwrap_or(("", path));
    let mut query = serde_json::Map::new();
    query.insert("from".into(), json!([{ "collectionId": collection }]));
    if !fields.is_empty() {
        query.insert(
            "select".into(),
            json!({ "fields": fields.iter().map(|field| json!({"fieldPath": field})).collect::<Vec<_>>() }),
        );
    }
    if let Some(filter) = filter {
        query.insert("where".into(), filter);
    }
    if let Some(order_by) = order_by {
        query.insert("orderBy".into(), Value::Array(order_by));
    }
    query.insert("offset".into(), json!((page.saturating_sub(1)) * PAGE_SIZE));
    query.insert("limit".into(), json!(PAGE_SIZE));
    let http = client();
    let mut request = http
        .post(format!("{FIRESTORE_URL}/{parent}:runQuery"))
        .json(json!({ "structuredQuery": Value::Object(query) }).to_string());
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn series_fields(include_latest_fields: bool, include_coin: bool, include_exclusive: bool) -> &'static [&'static str] {
    match (include_latest_fields, include_coin, include_exclusive) {
        (true, true, true) => &["id", "name", "name_lowercase", "description", "authors", "genres", "ongoing", "releaseStatusTag", "titleArt", "enabled", "onlyTransactional", "onlyOnMangamo"],
        (true, true, false) => &["id", "name", "name_lowercase", "description", "authors", "genres", "ongoing", "releaseStatusTag", "titleArt", "enabled", "onlyTransactional"],
        (true, false, true) => &["id", "name", "name_lowercase", "description", "authors", "genres", "ongoing", "releaseStatusTag", "titleArt", "enabled", "onlyOnMangamo"],
        (_, true, true) => &["id", "name", "name_lowercase", "description", "authors", "genres", "ongoing", "releaseStatusTag", "titleArt", "onlyTransactional", "onlyOnMangamo"],
        (_, true, false) => &["id", "name", "name_lowercase", "description", "authors", "genres", "ongoing", "releaseStatusTag", "titleArt", "onlyTransactional"],
        (_, false, true) => &["id", "name", "name_lowercase", "description", "authors", "genres", "ongoing", "releaseStatusTag", "titleArt", "onlyOnMangamo"],
        _ => &["id", "name", "name_lowercase", "description", "authors", "genres", "ongoing", "releaseStatusTag", "titleArt"],
    }
}

fn enabled_filter() -> Value {
    field_filter("enabled", "EQUAL", json!({"booleanValue": true}))
}

fn field_filter(field: &str, op: &str, value: Value) -> Value {
    json!({ "fieldFilter": { "op": op, "field": { "fieldPath": field }, "value": value } })
}

fn and_filter(filters: Vec<Value>) -> Value {
    json!({ "compositeFilter": { "op": "AND", "filters": filters } })
}

fn order_desc(field: &str) -> Value {
    json!({ "direction": "DESCENDING", "field": { "fieldPath": field } })
}

fn parse_series_page(body: &str, hide_coin: bool) -> Paged<CatalogItem> {
    let values = json_value(body);
    let entries = values
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|wrapper| wrapper.get("document").or_else(|| wrapper.get("found")))
        .filter_map(|doc| doc.get("fields"))
        .map(decode_firestore)
        .filter(|value| !(hide_coin && value.get("onlyTransactional").and_then(Value::as_bool) == Some(true)))
        .map(|value| series_item(&value))
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() >= PAGE_SIZE as usize,
        entries,
    }
}

fn details_by_id(series_id: i64, request: &Value) -> CatalogItem {
    let token = id_token(request);
    let body = firestore_document(
        &format!("Series/{series_id}"),
        series_fields(false, false, false),
        token.as_deref(),
        SERIES_DETAIL_FIXTURE,
    );
    let fields = json_value(&body)
        .get("fields")
        .map(decode_firestore)
        .unwrap_or_else(|| json_value(SERIES_DETAIL_DECODED_FIXTURE));
    series_item(&fields)
}

fn series_item(value: &Value) -> CatalogItem {
    let id = value.get("id").and_then(Value::as_i64).unwrap_or(0);
    let title = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Mangamo")
        .to_string();
    CatalogItem {
        key: id.to_string(),
        title: title.clone(),
        cover: value.get("titleArt").and_then(Value::as_str).map(ToOwned::to_owned),
        url: Some(catalog_url(&id, &title)),
        authors: names(value.get("authors")),
        description: value.get("description").and_then(Value::as_str).map(ToOwned::to_owned),
        tags: names(value.get("genres")),
        language: Some("en".into()),
        content_rating: Some("adult".into()),
        status: match value.get("releaseStatusTag").and_then(Value::as_str) {
            Some("Ongoing") => ItemStatus::Ongoing,
            Some("series-complete") | Some("Completed") => ItemStatus::Completed,
            Some("Paused") => ItemStatus::Hiatus,
            _ if value.get("ongoing").and_then(Value::as_bool) == Some(true) => ItemStatus::Ongoing,
            _ => ItemStatus::Unknown,
        },
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(series_body: &str, chapters_body: &str, hide_coin: bool) -> Vec<MangaChapter> {
    let series = json_value(series_body)
        .get("fields")
        .map(decode_firestore)
        .unwrap_or_default();
    let max_free = series.get("maxFreeChapterNumber").and_then(Value::as_i64).unwrap_or(0) as f32;
    let max_metered = series
        .get("maxMeteredReadingChapterNumber")
        .and_then(Value::as_i64)
        .unwrap_or(0) as f32;
    let series_coin = series.get("onlyTransactional").and_then(Value::as_bool) == Some(true);
    json_value(chapters_body)
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|wrapper| wrapper.get("document"))
        .filter_map(|doc| doc.get("fields"))
        .map(decode_firestore)
        .filter(|value| value.get("enabled").and_then(Value::as_bool) == Some(true))
        .filter_map(|value| {
            let series_id = value.get("seriesId").and_then(Value::as_i64)?;
            let id = value.get("id").and_then(Value::as_i64)?;
            let number = value.get("chapterNumber").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let coin = value.get("onlyTransactional").and_then(Value::as_bool) == Some(true)
                || (series_coin && number > max_free);
            if hide_coin && coin {
                return None;
            }
            let mut title = value.get("name").and_then(Value::as_str).unwrap_or("Chapter").to_string();
            if coin {
                title.push_str(" [Coin]");
            } else if number > max_free && number <= max_metered {
                title.push_str(" [Metered]");
            } else if number > max_free {
                title.push_str(" [Locked]");
            }
            Some(MangaChapter {
                key: format!("{series_id}:{id}"),
                title: Some(title),
                chapter_number: Some(number),
                date_uploaded: value.get("createdAt").and_then(Value::as_i64),
                language: Some("en".into()),
                url: Some(format!("{BASE_URL}/catalog?series={series_id}&chapter={id}")),
                is_locked: number > max_free,
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    json_value(body)
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let url = page.get("uri").and_then(Value::as_str)?.to_string();
            let index = page.get("pageNumber").and_then(Value::as_u64).unwrap_or(1).saturating_sub(1) as usize;
            Some(MangaPage {
                content: PageContent::Url {
                    url: url.clone(),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn decode_firestore(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return Value::Null;
    };
    Value::Object(
        object
            .iter()
            .map(|(key, field)| (key.clone(), decode_field(field)))
            .collect(),
    )
}

fn decode_field(field: &Value) -> Value {
    let Some((kind, value)) = field.as_object().and_then(|object| object.iter().next()) else {
        return Value::Null;
    };
    match kind.as_str() {
        "stringValue" => Value::String(value.as_str().unwrap_or_default().to_string()),
        "integerValue" => value
            .as_str()
            .and_then(|text| text.parse::<i64>().ok())
            .map(Value::from)
            .unwrap_or(Value::Null),
        "doubleValue" => value.as_f64().map(Value::from).unwrap_or(Value::Null),
        "booleanValue" => Value::Bool(value.as_bool().unwrap_or(false)),
        "arrayValue" => Value::Array(
            value
                .get("values")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(decode_field)
                .collect(),
        ),
        "mapValue" => value.get("fields").map(decode_firestore).unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

fn names(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("name").and_then(Value::as_str).or_else(|| entry.as_str()))
        .map(ToOwned::to_owned)
        .collect()
}

fn preference_has(request: &Value, id: &str, value: &str) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(id))
        .and_then(Value::as_array)
        .is_some_and(|values| values.iter().any(|entry| entry.as_str() == Some(value)))
}

fn catalog_url(id: &i64, title: &str) -> String {
    let slug = title.to_ascii_lowercase().replace(' ', "-");
    format!("{BASE_URL}/catalog/{}?series={id}", url::query_escape(&slug))
}

fn series_id_from_url(input: &str) -> Option<i64> {
    input
        .split('?')
        .nth(1)?
        .split('&')
        .find_map(|part| part.strip_prefix("series="))
        .and_then(|value| value.parse().ok())
}

fn series_id_from_key(key: &str) -> i64 {
    key.split(':').next().and_then(|value| value.parse().ok()).unwrap_or(1)
}

fn chapter_parts(key: &str) -> (i64, i64) {
    let mut parts = key.split(':');
    (
        parts.next().and_then(|value| value.parse().ok()).unwrap_or(1),
        parts.next().and_then(|value| value.parse().ok()).unwrap_or(1),
    )
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn json_value(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

export_manga_source!(SOURCE);

const SERIES_FIXTURE: &str = r#"[{"document":{"fields":{"id":{"integerValue":"1"},"name":{"stringValue":"Sample Mangamo"},"name_lowercase":{"stringValue":"sample mangamo"},"description":{"stringValue":"Sample"},"enabled":{"booleanValue":true},"ongoing":{"booleanValue":true},"titleArt":{"stringValue":"https://www.mangamo.com/sample.jpg"},"authors":{"arrayValue":{"values":[{"mapValue":{"fields":{"name":{"stringValue":"Mangamo"}}}}]}},"genres":{"arrayValue":{"values":[{"mapValue":{"fields":{"name":{"stringValue":"Action"}}}}]}}}}}]"#;
const SERIES_DETAIL_FIXTURE: &str = r#"{"fields":{"id":{"integerValue":"1"},"name":{"stringValue":"Sample Mangamo"},"name_lowercase":{"stringValue":"sample mangamo"},"description":{"stringValue":"Sample"},"enabled":{"booleanValue":true},"ongoing":{"booleanValue":true},"titleArt":{"stringValue":"https://www.mangamo.com/sample.jpg"},"maxFreeChapterNumber":{"integerValue":"1"},"maxMeteredReadingChapterNumber":{"integerValue":"1"},"authors":{"arrayValue":{"values":[{"mapValue":{"fields":{"name":{"stringValue":"Mangamo"}}}}]}},"genres":{"arrayValue":{"values":[{"mapValue":{"fields":{"name":{"stringValue":"Action"}}}}]}}}}"#;
const SERIES_DETAIL_DECODED_FIXTURE: &str = r#"{"id":1,"name":"Sample Mangamo","description":"Sample","ongoing":true,"titleArt":"https://www.mangamo.com/sample.jpg"}"#;
const CHAPTERS_FIXTURE: &str = r#"[{"document":{"fields":{"enabled":{"booleanValue":true},"id":{"integerValue":"1"},"seriesId":{"integerValue":"1"},"chapterNumber":{"doubleValue":1.0},"name":{"stringValue":"Chapter 1"},"createdAt":{"integerValue":"1704067200"}}}}]"#;
const PAGES_FIXTURE: &str = r#"[{"id":1,"pageNumber":1,"uri":"https://www.mangamo.com/sample.jpg"}]"#;
