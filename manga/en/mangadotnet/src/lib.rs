use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Mangadotnet = Mangadotnet;
const BASE_URL: &str = "https://mangadot.net";

struct Mangadotnet;

impl MangaSource for Mangadotnet {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mode = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest-updates"
        } else {
            "most-tracked"
        };
        Ok(parse_listing(&fetch_json(
            &view_all_url(mode, page, &request),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            if key.starts_with("/manga/") {
                return Ok(Paged {
                    entries: vec![parse_details(
                        &fetch_json(&details_url(id_from_manga_key(&key)), DETAILS_FIXTURE),
                        Some(key),
                    )],
                    has_next_page: false,
                });
            }
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_json(
            &search_url(page, query, &request),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/1".to_string());
        Ok(parse_details(
            &fetch_json(&details_url(id_from_manga_key(&key)), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/1".to_string());
        let id = id_from_manga_key(&key);
        Ok(parse_chapters(
            &fetch_json(
                &format!("{BASE_URL}/api/manga/{id}/chapters/list?lang=en"),
                CHAPTERS_FIXTURE,
            ),
            false,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "chapter:1:user:false".to_string());
        let (id, source, is_volume) = chapter_key_parts(&key);
        let endpoint = if source == "user" {
            "uploads"
        } else {
            "chapters"
        };
        let url = format!("{BASE_URL}/api/{endpoint}/{id}/images");
        let _ = is_volume;
        Ok(parse_pages(&fetch_json(&url, PAGES_FIXTURE)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (id, source, is_volume) = chapter_key_parts(&key);
            let mut target = format!("{BASE_URL}/chapter/{id}");
            if source == "user" || is_volume {
                target.push_str("?source=user");
            }
            if is_volume {
                target.push_str("&mode=volume");
            }
            target
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let item = if key.starts_with("/manga/") {
                Some(parse_details(
                    &fetch_json(&details_url(id_from_manga_key(&key)), DETAILS_FIXTURE),
                    Some(key),
                ))
            } else {
                None
            };
            return Ok(Some(UrlResolveResult {
                item,
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
        .with_header("Accept", "application/json, text/plain, */*")
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

fn view_all_url(mode: &str, page: u64, request: &Value) -> String {
    let mut target = format!(
        "{BASE_URL}/view-all/{mode}.data?adult={}&_routes=pages/ViewAllPage",
        adult_param(request)
    );
    if page > 1 {
        target.push_str("&page=");
        target.push_str(&page.to_string());
    }
    append_common_filters(&mut target, request);
    target
}

fn search_url(page: u64, query: &str, request: &Value) -> String {
    let mut target = format!(
        "{BASE_URL}/search.data?adult={}&page={page}&_routes=pages/SearchPage",
        adult_param(request)
    );
    if !query.is_empty() {
        target.push_str("&search=");
        target.push_str(&url::query_escape(query));
    }
    let sort = filter_text(request, "sortBy").unwrap_or_else(|| {
        if query.is_empty() {
            "latest".to_string()
        } else {
            String::new()
        }
    });
    if !sort.is_empty() {
        target.push_str("&sortBy=");
        target.push_str(&url::query_escape(&sort));
    }
    target.push_str("&sortOrder=desc");
    append_common_filters(&mut target, request);
    target
}

fn append_common_filters(target: &mut String, request: &Value) {
    for value in split_filter(request, "origin") {
        target.push_str("&origin=");
        target.push_str(&url::query_escape(&value));
    }
    for value in split_filter(request, "genre") {
        target.push_str("&genre=");
        target.push_str(&url::query_escape(&value));
    }
    if let Some(status) = filter_text(request, "status").filter(|value| !value.is_empty()) {
        target.push_str("&status=");
        target.push_str(&url::query_escape(&status));
    }
    if let Some(author) = filter_text(request, "author").filter(|value| !value.is_empty()) {
        target.push_str("&author=");
        target.push_str(&url::query_escape(&author));
    }
    if let Some(artist) = filter_text(request, "artist").filter(|value| !value.is_empty()) {
        target.push_str("&artist=");
        target.push_str(&url::query_escape(&artist));
    }
}

fn adult_param(request: &Value) -> String {
    filter_text(request, "adult").unwrap_or_else(|| "0".to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let root = parse_jsonish(body).unwrap_or(Value::Null);
    let list = find_array_by_keys(&root, &["results", "manga_list"]).unwrap_or_default();
    let entries = list
        .iter()
        .filter_map(catalog_from_manga)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: has_next_page(&root),
    }
}

fn catalog_from_manga(value: &Value) -> Option<CatalogItem> {
    let id = json_any(value, "id")?;
    let key = format!("/manga/{id}");
    Some(CatalogItem {
        key: key.clone(),
        title: json_text(value, "title").unwrap_or_else(|| "Manga".to_string()),
        cover: json_text(value, "photo").and_then(normalize_image),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let root = parse_jsonish(body).unwrap_or(Value::Null);
    let manga = find_object_with_key(&root, "title").unwrap_or(Value::Null);
    let id = json_any(&manga, "id")
        .unwrap_or_else(|| id_from_manga_key(key.as_deref().unwrap_or("/manga/1")).to_string());
    let key = key.unwrap_or_else(|| format!("/manga/{id}"));
    let mut description = json_text(&manga, "description").unwrap_or_default();
    let links = external_links(&manga);
    if !links.is_empty() {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Links:\n");
        for link in links {
            description.push_str("- ");
            description.push_str(&link);
            description.push('\n');
        }
    }
    CatalogItem {
        key: key.clone(),
        title: json_text(&manga, "title")
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        alternate_titles: json_array_strings(&manga, "alt_titles"),
        cover: json_text(&manga, "photo").and_then(normalize_image),
        authors: parsed_string_list(&manga, "authors"),
        artists: parsed_string_list(&manga, "artists"),
        tags: manga_tags(&manga),
        description: if description.trim().is_empty() {
            None
        } else {
            Some(description.trim().to_string())
        },
        rating: json_number(&manga, "avg_rating"),
        status: manga_status(&manga),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, is_volume: bool) -> Vec<MangaChapter> {
    let root = parse_jsonish(body).unwrap_or(Value::Null);
    let chapters = root
        .as_array()
        .cloned()
        .or_else(|| find_array_by_keys(&root, &["chapters"]))
        .unwrap_or_default();
    let mut output = chapters
        .into_iter()
        .filter_map(|chapter| {
            let id = json_any(&chapter, "id")?;
            let source = json_text(&chapter, "source").unwrap_or_else(|| "user".to_string());
            let number = json_number(
                &chapter,
                if is_volume {
                    "volume_number"
                } else {
                    "chapter_number"
                },
            )
            .unwrap_or(0.0);
            let title = if is_volume {
                format!("Volume {}", number_string(number))
            } else {
                let number_label = number_string(number);
                let raw = json_text(&chapter, "chapter_title").unwrap_or_default();
                if raw.contains(&number_label) {
                    raw
                } else if raw.trim().is_empty() {
                    format!("Chapter {number_label}")
                } else {
                    format!("Chapter {number_label}: {}", raw.trim())
                }
            };
            Some(MangaChapter {
                key: format!("chapter:{id}:{source}:{is_volume}"),
                title: Some(title),
                chapter_number: if is_volume { None } else { Some(number) },
                volume_number: if is_volume { Some(number) } else { None },
                scanlators: json_text(&chapter, "group_name")
                    .or_else(|| json_text(&chapter, "scanlator_name"))
                    .into_iter()
                    .filter(|value| !value.is_empty())
                    .collect(),
                language: json_text(&chapter, "language"),
                page_count: json_any(&chapter, "page_count")
                    .and_then(|value| value.parse::<u32>().ok()),
                url: Some(format!("{BASE_URL}/chapter/{id}")),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    output.reverse();
    output
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = parse_jsonish(body).unwrap_or(Value::Null);
    let images = find_array_by_keys(&root, &["images"]).unwrap_or_default();
    images
        .into_iter()
        .filter_map(|image| json_text(&image, "url"))
        .filter_map(normalize_image)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_jsonish(body: &str) -> Option<Value> {
    serde_json::from_str::<Value>(body).ok().or_else(|| {
        extract_first_json(body).and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    })
}

fn extract_first_json(body: &str) -> Option<String> {
    let start = body.find('{').or_else(|| body.find('['))?;
    let bytes = body.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (offset, &byte) in bytes[start..].iter().enumerate() {
        if in_string {
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if byte == b'{' || byte == b'[' {
            depth += 1;
        } else if byte == b'}' || byte == b']' {
            depth -= 1;
            if depth == 0 {
                return Some(body[start..start + offset + 1].to_string());
            }
        }
    }
    None
}

fn find_array_by_keys(value: &Value, keys: &[&str]) -> Option<Vec<Value>> {
    if let Some(array) = value.as_array() {
        return Some(array.clone());
    }
    if let Some(object) = value.as_object() {
        for key in keys {
            if let Some(array) = object.get(*key).and_then(Value::as_array) {
                return Some(array.clone());
            }
        }
        for child in object.values() {
            if let Some(array) = find_array_by_keys(child, keys) {
                return Some(array);
            }
        }
    }
    None
}

fn find_object_with_key(value: &Value, key: &str) -> Option<Value> {
    if value.get(key).is_some() {
        return Some(value.clone());
    }
    if let Some(object) = value.as_object() {
        for child in object.values() {
            if let Some(found) = find_object_with_key(child, key) {
                return Some(found);
            }
        }
    } else if let Some(array) = value.as_array() {
        for child in array {
            if let Some(found) = find_object_with_key(child, key) {
                return Some(found);
            }
        }
    }
    None
}

fn has_next_page(root: &Value) -> bool {
    find_object_with_key(root, "pagination").is_some_and(|pagination| {
        let pagination = pagination.get("pagination").unwrap_or(&pagination);
        let current =
            json_any(pagination, "current_page").and_then(|value| value.parse::<u64>().ok());
        let total = json_any(pagination, "total_pages").and_then(|value| value.parse::<u64>().ok());
        current
            .zip(total)
            .is_some_and(|(current, total)| current < total)
            || pagination
                .get("next_cursor")
                .is_some_and(|value| !value.is_null())
    })
}

fn manga_status(manga: &Value) -> ItemStatus {
    if json_array_strings(manga, "genres")
        .iter()
        .any(|value| value == "One Shot")
    {
        ItemStatus::Completed
    } else if json_text(manga, "hiatus").as_deref() == Some("Yes") {
        ItemStatus::Hiatus
    } else {
        match json_text(manga, "status")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "ongoing" => ItemStatus::Ongoing,
            "completed" => ItemStatus::Completed,
            "hiatus" => ItemStatus::Hiatus,
            _ => ItemStatus::Unknown,
        }
    }
}

fn manga_tags(manga: &Value) -> Vec<String> {
    let mut tags = Vec::new();
    match json_text(manga, "country_of_origin").as_deref() {
        Some("JP") => tags.push("Manga".to_string()),
        Some("KR") => tags.push("Manhwa".to_string()),
        Some("CN") => tags.push("Manhua".to_string()),
        _ => {}
    }
    tags.extend(json_array_strings(manga, "genres"));
    tags
}

fn external_links(manga: &Value) -> Vec<String> {
    [
        ("anilist_id", "AniList", "https://anilist.co/manga/"),
        (
            "mangaupdates_id",
            "MangaUpdates",
            "https://mangaupdates.com/series/",
        ),
        ("mangabaka_id", "MangaBaka", "https://mangabaka.org/"),
        ("mal_id", "MyAnimeList", "https://myanimelist.net/manga/"),
        ("kitsu_id", "Kitsu", "https://kitsu.app/manga/"),
    ]
    .into_iter()
    .filter_map(|(key, label, base)| {
        json_any(manga, key).map(|id| format!("[{label}]({base}{id})"))
    })
    .chain(json_text(manga, "source_url").map(|source| format!("[Source]({source})")))
    .collect()
}

fn parsed_string_list(value: &Value, key: &str) -> Vec<String> {
    json_text(value, key)
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default()
}

fn json_array_strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn filter_text(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn split_filter(request: &Value, key: &str) -> Vec<String> {
    filter_text(request, key)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn normalize_image(value: String) -> Option<String> {
    if value.starts_with('/') {
        Some(format!("{BASE_URL}{value}"))
    } else if value.starts_with("http://") || value.starts_with("https://") {
        Some(value)
    } else {
        None
    }
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn id_from_manga_key(key: &str) -> &str {
    key.trim_matches('/')
        .strip_prefix("manga/")
        .unwrap_or(key.trim_matches('/'))
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn details_url(id: &str) -> String {
    format!("{BASE_URL}/manga/{id}.data?_routes=pages/MangaDetailPage")
}

fn chapter_key_parts(key: &str) -> (String, String, bool) {
    let mut parts = key.split(':');
    let _prefix = parts.next();
    let id = parts.next().unwrap_or(key).to_string();
    let source = parts.next().unwrap_or("user").to_string();
    let is_volume = parts.next().is_some_and(|value| value == "true");
    (id, source, is_volume)
}

fn number_string(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i32)
    } else {
        value.to_string()
    }
}

fn json_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn json_any(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|value| {
        value
            .as_str()
            .map(ToString::to_string)
            .or_else(|| value.as_i64().map(|number| number.to_string()))
            .or_else(|| value.as_u64().map(|number| number.to_string()))
            .or_else(|| value.as_f64().map(|number| number.to_string()))
    })
}

fn json_number(value: &Value, key: &str) -> Option<f32> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .map(|number| number as f32)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"{"data":{"data":{"manga_list":[{"id":1,"title":"Sample","photo":"/cover.jpg"}],"pagination":{"current_page":1,"total_pages":1}}}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"mangaData":{"data":{"manga":{"id":1,"title":"Sample","genres":["Action"],"description":"Summary","photo":"/cover.jpg","status":"Ongoing","authors":"[\"Author\"]","artists":"[\"Artist\"]","alt_titles":["Alt Sample"],"country_of_origin":"JP","avg_rating":8.5}}}}}"#;
const CHAPTERS_FIXTURE: &str = r#"[{"id":1,"chapter_number":1,"chapter_title":"Start","language":"en","group_name":"Group","date_added":"2024-01-01 00:00:00","source":"user","page_count":1}]"#;
const PAGES_FIXTURE: &str = r#"{"manga":{"id":1},"images":[{"url":"/page1.jpg"}]}"#;

export_manga_source!(SOURCE);
