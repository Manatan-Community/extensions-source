use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, Viewer, abi::ExtensionResult, export_manga_source, http::HttpClient,
    source::MangaSource,
};
use manatan_shared::{dates, manga, url};
use serde_json::{Value, json};

const SOURCE: HoneyManga = HoneyManga;
const BASE_URL: &str = "https://honey-manga.com.ua";
const API_URL: &str = "https://data.api.honey-manga.com.ua";
const SEARCH_API_URL: &str = "https://search.api.honey-manga.com.ua";
const IMAGE_STORAGE_URL: &str = "https://hmvolumestorage.b-cdn.net/public-resources";

struct HoneyManga;

impl MangaSource for HoneyManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "lastUpdated"
        } else {
            "likes"
        };
        Ok(parse_catalog(
            &post_json(
                &format!("{API_URL}/v2/manga/cursor-list"),
                catalog_body(
                    page(&request),
                    sort,
                    "DESC",
                    &Value::Null,
                    request.get("preferences").unwrap_or(&Value::Null),
                ),
                LIST_FIXTURE,
            ),
            request.get("preferences").unwrap_or(&Value::Null),
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let preferences = request.get("preferences").unwrap_or(&Value::Null);
        if query.starts_with(BASE_URL) && query.contains("/book/") {
            let key = query
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("sample");
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_json(&details_url(key), DETAILS_FIXTURE),
                    key,
                )],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            let target = format!(
                "{SEARCH_API_URL}/v2/manga/pattern?query={}",
                url::query_escape(query)
            );
            return Ok(Paged {
                entries: parse_search_pattern(&fetch_json(&target, SEARCH_FIXTURE), preferences),
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let sort = filter_string(filters, "sortBy").unwrap_or_else(|| "likes".to_string());
        let order = filter_string(filters, "sortOrder").unwrap_or_else(|| "DESC".to_string());
        Ok(parse_catalog(
            &post_json(
                &format!("{API_URL}/v2/manga/cursor-list"),
                catalog_body(page(&request), &sort, &order, filters, preferences),
                LIST_FIXTURE,
            ),
            preferences,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = normalize_id(
            &manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string()),
        );
        Ok(parse_details(
            &fetch_json(&details_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = normalize_id(
            &manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string()),
        );
        Ok(parse_chapters(&post_json(
            &format!("{API_URL}/v2/chapter/cursor-list"),
            json!({ "mangaId": key, "page": 1, "pageSize": 10000, "sortOrder": "DESC" }),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "chapter-id/sample".to_string());
        let chapter_id = key.split('/').next().unwrap_or("chapter-id");
        Ok(parse_pages(
            &fetch_json(
                &format!("{API_URL}/chapter/frames/{chapter_id}"),
                PAGES_FIXTURE,
            ),
            &key,
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/book/{}", normalize_id(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}/read/{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/book/") {
            let key = input
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("sample");
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_json(&details_url(key), DETAILS_FIXTURE),
                    key,
                )),
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

export_manga_source!(SOURCE);

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_origin(format!("{BASE_URL}/"))
        .with_header("Accept", "application/json, text/plain, */*")
        .with_header("Content-Type", "application/json")
        .with_header("Accept-Language", "uk-UA,uk;q=0.9,en-US;q=0.8,en;q=0.7")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_json(target: &str, body: Value, fixture: &str) -> String {
    client()
        .post(target)
        .xhr()
        .json(body.to_string())
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn catalog_body(
    page: u64,
    sort_by: &str,
    sort_order: &str,
    filters: &Value,
    preferences: &Value,
) -> Value {
    let mut filter_items = Vec::new();
    add_filter(
        &mut filter_items,
        filters,
        "translationStatus",
        "translationStatus",
        "EQUAL",
    );
    add_filter(
        &mut filter_items,
        filters,
        "titleStatus",
        "titleStatus",
        "EQUAL",
    );
    add_filter(&mut filter_items, filters, "type", "type", "EQUAL");
    add_filter(&mut filter_items, filters, "hideType", "type", "NOT_IN");
    add_filter(&mut filter_items, filters, "genres", "genres", "ALL");
    add_filter(&mut filter_items, filters, "tags", "tags", "ALL");
    if filters == &Value::Null {
        if let Some(types) =
            preference_values(preferences, "blockedTypes").filter(|values| !values.is_empty())
        {
            filter_items.push(
                json!({ "filterBy": "type", "filterOperator": "NOT_IN", "filterValue": types }),
            );
        } else {
            filter_items.push(json!({ "filterBy": "type", "filterOperator": "NOT_IN", "filterValue": ["Новела"] }));
        }
        if let Some(genres) =
            preference_values(preferences, "blockedGenres").filter(|values| !values.is_empty())
        {
            filter_items.push(
                json!({ "filterBy": "genres", "filterOperator": "NOT_IN", "filterValue": genres }),
            );
        }
    }
    json!({
        "page": page,
        "pageSize": 30,
        "sort": { "sortBy": sort_by, "sortOrder": sort_order },
        "filters": if filter_items.is_empty() { Value::Null } else { Value::Array(filter_items) }
    })
}

fn add_filter(out: &mut Vec<Value>, filters: &Value, source_key: &str, filter_by: &str, op: &str) {
    if let Some(values) = filter_values(filters, source_key).filter(|values| !values.is_empty()) {
        out.push(json!({ "filterBy": filter_by, "filterOperator": op, "filterValue": values }));
    }
}

fn details_url(key: &str) -> String {
    format!("{API_URL}/manga/{}", url::query_escape(&normalize_id(key)))
}

fn parse_catalog(body: &str, preferences: &Value) -> Paged<CatalogItem> {
    let root = parse_json(body);
    let entries = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| catalog_item(item, preferences))
        .collect();
    Paged {
        entries,
        has_next_page: root
            .get("cursorNext")
            .and_then(Value::as_object)
            .is_some_and(|value| !value.is_empty()),
    }
}

fn parse_search_pattern(body: &str, preferences: &Value) -> Vec<CatalogItem> {
    parse_json(body)
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| catalog_item(item, preferences))
        .collect()
}

fn catalog_item(item: &Value, preferences: &Value) -> Option<CatalogItem> {
    let kind = string(item, "type");
    let genres = value_strings(item.get("genres"));
    if preference_values(preferences, "blockedTypes")
        .unwrap_or_else(|| vec!["Новела".to_string()])
        .contains(&kind)
    {
        return None;
    }
    if preference_values(preferences, "blockedGenres")
        .unwrap_or_default()
        .iter()
        .any(|genre| genres.contains(genre))
    {
        return None;
    }
    let key = string(item, "id");
    Some(CatalogItem {
        key: key.clone(),
        title: string(item, "title"),
        cover: opt_string(item, "posterId").map(|poster| format!("{IMAGE_STORAGE_URL}/{poster}")),
        url: Some(format!("{BASE_URL}/book/{key}")),
        tags: genres,
        language: Some("uk".to_string()),
        content_rating: Some("safe".to_string()),
        viewer: Some(Viewer::RightToLeft),
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, fallback_key: &str) -> CatalogItem {
    let root = parse_json(body);
    let key = opt_string(&root, "id").unwrap_or_else(|| fallback_key.to_string());
    let mut tags = vec![string(&root, "type")];
    tags.extend(value_strings(root.get("genresAndTags")));
    CatalogItem {
        key: key.clone(),
        title: string(&root, "title"),
        cover: opt_string(&root, "posterId").map(|poster| format!("{IMAGE_STORAGE_URL}/{poster}")),
        url: Some(format!("{BASE_URL}/book/{key}")),
        authors: value_strings(root.get("authors")),
        artists: value_strings(root.get("artists")),
        description: opt_string(&root, "description"),
        tags: tags.into_iter().filter(|value| !value.is_empty()).collect(),
        language: Some("uk".to_string()),
        content_rating: Some("safe".to_string()),
        status: match root.get("titleStatus").and_then(Value::as_str) {
            Some("Онгоінг") => ItemStatus::Ongoing,
            Some("Завершено") => ItemStatus::Completed,
            Some("Покинуто") => ItemStatus::Cancelled,
            Some("Призупинено") => ItemStatus::Hiatus,
            _ => ItemStatus::Unknown,
        },
        initialized: true,
        viewer: Some(Viewer::RightToLeft),
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    parse_json(body)
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| {
            !item
                .get("isMonetized")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .map(|item| {
            let id = string(item, "id");
            let manga_id = string(item, "mangaId");
            let chapter_num = item
                .get("chapterNum")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let sub = item
                .get("subChapterNum")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let number = if sub == 0.0 {
                chapter_num
            } else {
                chapter_num + sub / 10.0
            };
            let suffix = if sub == 0.0 {
                String::new()
            } else {
                format!(".{}", compact_float(sub))
            };
            MangaChapter {
                key: format!("{id}/{manga_id}"),
                title: Some(format!(
                    "Том {} - Розділ {}{}",
                    item.get("volume").and_then(Value::as_i64).unwrap_or(0),
                    compact_float(chapter_num),
                    suffix
                )),
                chapter_number: Some(number as f32),
                date_uploaded: item
                    .get("lastUpdated")
                    .and_then(Value::as_str)
                    .and_then(parse_iso_date),
                language: Some("uk".to_string()),
                url: Some(format!("{BASE_URL}/read/{id}/{manga_id}")),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str, key: &str) -> Vec<MangaPage> {
    let referer = format!("{BASE_URL}/read/{key}");
    let mut entries = parse_json(body)
        .get("resourceIds")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(index, image)| {
                    image
                        .as_str()
                        .map(|image| (index.parse::<usize>().unwrap_or(0), image.to_string()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    entries.sort_by_key(|(index, _)| *index);
    entries
        .into_iter()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: format!("{IMAGE_STORAGE_URL}/{image}"),
                context: Some(manga::image_headers(&referer)),
            },
            headers: manga::image_headers(&referer),
            description: Some(format!("Page {index}")),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_json(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn normalize_id(value: &str) -> String {
    value
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(value)
        .to_string()
}

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn filter_values(filters: &Value, key: &str) -> Option<Vec<String>> {
    filters.get(key).and_then(values_from_value)
}

fn preference_values(preferences: &Value, key: &str) -> Option<Vec<String>> {
    preferences.get(key).and_then(values_from_value)
}

fn values_from_value(value: &Value) -> Option<Vec<String>> {
    if let Some(array) = value.as_array() {
        return Some(
            array
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect(),
        );
    }
    value.as_str().map(|text| {
        text.split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(ToString::to_string)
            .collect()
    })
}

fn string(value: &Value, key: &str) -> String {
    opt_string(value, key).unwrap_or_default()
}

fn opt_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|value| {
        value
            .as_str()
            .map(ToString::to_string)
            .or_else(|| value.as_i64().map(|number| number.to_string()))
    })
}

fn value_strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn parse_iso_date(value: &str) -> Option<i64> {
    value.get(..10).and_then(dates::parse_ymd)
}

fn compact_float(value: f64) -> String {
    if value.fract() == 0.0 {
        (value as i64).to_string()
    } else {
        value.to_string()
    }
}

const LIST_FIXTURE: &str = r#"{"data":[{"id":"sample","posterId":"poster.jpg","title":"Sample HoneyManga","type":"Манґа","genres":["Фентезі"]}],"cursorNext":{}}"#;
const SEARCH_FIXTURE: &str = r#"[{"id":"sample","posterId":"poster.jpg","title":"Sample HoneyManga","type":"Манґа","genres":["Фентезі"]}]"#;
const DETAILS_FIXTURE: &str = r#"{"id":"sample","posterId":"poster.jpg","title":"Sample HoneyManga","description":"Fixture","type":"Манґа","authors":[],"artists":[],"genresAndTags":["Фентезі"],"titleStatus":"Онгоінг"}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"id":"chapter-id","volume":1,"chapterNum":1,"subChapterNum":0,"mangaId":"sample","lastUpdated":"2024-01-01T00:00:00.000Z","isMonetized":false}]}"#;
const PAGES_FIXTURE: &str = r#"{"resourceIds":{"1":"page-1.jpg","2":"page-2.jpg"}}"#;
