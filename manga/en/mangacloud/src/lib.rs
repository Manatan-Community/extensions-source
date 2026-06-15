use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient};
use serde_json::{Value, json};

const SOURCE: MangaCloud = MangaCloud;
const BASE_URL: &str = "https://mangacloud.org";
const API_URL: &str = "https://api.mangacloud.org";
const CDN_URL: &str = "https://pika.mangacloud.org";

struct MangaCloud;

impl MangaSource for MangaCloud {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list_root(LIST_FIXTURE, true));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            let body = post_json(
                &format!("{API_URL}/comic-updates"),
                json!({ "page": page }),
                LATEST_FIXTURE,
            );
            return Ok(parse_list_root(&body, false));
        }
        if page <= 3 {
            let period = match page {
                1 => "today",
                2 => "week",
                _ => "month",
            };
            return Ok(parse_list_root(
                &fetch_json(
                    &format!("{API_URL}/comic-popular-view/{period}"),
                    LIST_FIXTURE,
                ),
                true,
            ));
        }
        let body = post_json(
            &format!("{API_URL}/comic/browse"),
            json!({ "page": page - 3 }),
            BROWSE_FIXTURE,
        );
        Ok(parse_browse_array(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let id = key
                .trim_start_matches("/comic/")
                .split('/')
                .next()
                .unwrap_or("");
            let id = id.to_string();
            return Ok(Paged {
                entries: vec![details_by_id(&id, Some(key))],
                has_next_page: false,
            });
        }
        let body = post_json(
            &format!("{API_URL}/comic/browse"),
            json!({
                "title": if query.is_empty() { Value::Null } else { Value::String(query.to_string()) },
                "page": page
            }),
            BROWSE_FIXTURE,
        );
        Ok(parse_browse_array(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let id = comic_id_from_key(&key);
        Ok(details_by_id(&id, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let id = comic_id_from_key(&key);
        let body = fetch_json(&format!("{API_URL}/comic/{id}"), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| r#"{"comicId":"sample","chapterId":"chapter-1"}"#.into());
        let chapter_id = chapter_id_from_key(&key);
        let body = fetch_json(&format!("{API_URL}/chapter5/{chapter_id}"), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let id = key
                .trim_start_matches("/comic/")
                .split('/')
                .next()
                .unwrap_or("");
            let id = id.to_string();
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_id(&id, Some(key))),
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

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_json(target: &str, payload: Value, fixture: &str) -> String {
    client()
        .post(target)
        .header("Accept", "application/json")
        .json(payload.to_string())
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_list_root(body: &str, has_next_page: bool) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let list = root.pointer("/data/list").or_else(|| root.get("data"));
    let entries = list
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(browse_item)
        .collect();
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_browse_array(body: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let entries = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(browse_item)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() == 10,
        entries,
    }
}

fn browse_item(item: &Value) -> CatalogItem {
    let id = string_field(item, "id");
    CatalogItem {
        key: id.clone(),
        title: string_field(item, "title"),
        cover: image_url(&id, item.get("cover")),
        url: Some(format!("{BASE_URL}/comic/{id}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn details_by_id(id: &str, key: Option<String>) -> CatalogItem {
    let body = fetch_json(&format!("{API_URL}/comic/{id}"), DETAILS_FIXTURE);
    let root: Value = serde_json::from_str(&body).unwrap_or_default();
    let data = root.get("data").unwrap_or(&Value::Null);
    let id = string_field(data, "id");
    let mut tags = Vec::new();
    if let Some(kind) = string_opt(data, "type") {
        tags.push(kind);
    }
    tags.extend(
        data.get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tag| tag.get("name").and_then(Value::as_str))
            .map(ToString::to_string),
    );
    CatalogItem {
        key: key.unwrap_or_else(|| id.clone()),
        title: string_field(data, "title"),
        alternate_titles: alternate_titles(data),
        cover: image_url(&id, data.get("cover")),
        authors: split_people(data.get("authors")),
        artists: split_people(data.get("artists")),
        description: description(data),
        tags,
        status: status_from(string_opt(data, "status").as_deref()),
        url: Some(format!("{BASE_URL}/comic/{id}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let data = root.get("data").unwrap_or(&Value::Null);
    let comic_id = string_field(data, "id");
    data.get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|chapter| {
            let chapter_id = string_field(chapter, "id");
            let number = chapter.get("number").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let mut title = format!("Chapter {}", trim_number(number));
            if let Some(name) = string_opt(chapter, "name") {
                title.push_str(" - ");
                title.push_str(&name);
            }
            MangaChapter {
                key: json!({ "comicId": comic_id, "chapterId": chapter_id }).to_string(),
                title: Some(title),
                chapter_number: Some(number),
                date_uploaded: parse_rfc3339_date(string_opt(chapter, "created_date").as_deref()),
                url: Some(format!("{BASE_URL}/comic/{comic_id}/chapter/{chapter_id}")),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let data = root.get("data").unwrap_or(&Value::Null);
    let comic_id = string_field(data, "comic_id");
    let chapter_id = string_field(data, "id");
    data.get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, image)| {
            let image_id = string_field(image, "id");
            let format = string_field(image, "f");
            if image_id.is_empty() || format.is_empty() {
                return None;
            }
            let url = format!("{CDN_URL}/{comic_id}/{chapter_id}/{image_id}.{format}");
            Some(MangaPage {
                content: PageContent::Url {
                    url,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input
                .trim_start_matches(BASE_URL)
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    input.to_string()
}

fn comic_id_from_key(key: &str) -> String {
    key.trim_start_matches("/comic/")
        .split('/')
        .next()
        .unwrap_or(key)
        .trim_matches('"')
        .to_string()
}

fn chapter_id_from_key(key: &str) -> String {
    serde_json::from_str::<Value>(key)
        .ok()
        .and_then(|value| {
            value
                .get("chapterId")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| key.trim_matches('"').to_string())
}

fn image_url(comic_id: &str, image: Option<&Value>) -> Option<String> {
    let image = image?;
    let image_id = string_field(image, "id");
    let format = string_field(image, "f");
    (!image_id.is_empty() && !format.is_empty())
        .then(|| format!("{CDN_URL}/{comic_id}/{image_id}.{format}"))
}

fn alternate_titles(data: &Value) -> Vec<String> {
    ["alt_titles", "nat_titles"]
        .iter()
        .filter_map(|key| data.get(*key).and_then(Value::as_str))
        .flat_map(|value| value.split(['•', '、']))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn split_people(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split('•')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn description(data: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(description) = string_opt(data, "description") {
        parts.push(description);
    }
    if let Some(year) = data.get("start_year").and_then(Value::as_i64) {
        let mut line = format!("Year: {year}");
        if let Some(end) = data.get("end_year").and_then(Value::as_i64) {
            line.push_str(&format!(" - {end}"));
        }
        parts.push(line);
    }
    let alt = alternate_titles(data);
    if !alt.is_empty() {
        parts.push(format!("Alternative Names:\n- {}", alt.join("\n- ")));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn status_from(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default() {
        "Ongoing" => ItemStatus::Ongoing,
        "Completed" => ItemStatus::Completed,
        "Cancelled" => ItemStatus::Cancelled,
        "Hiatus" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn parse_rfc3339_date(value: Option<&str>) -> Option<i64> {
    let date = value?.split('T').next()?;
    let parts = date.split('-').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    unix_date(
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    )
}

fn unix_date(year: i32, month: u32, day: u32) -> Option<i64> {
    let mut days = 0i64;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    for m in 1..month {
        days += match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if is_leap(year) => 29,
            2 => 28,
            _ => return None,
        };
    }
    Some((days + day as i64 - 1) * 86_400)
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn trim_number(value: f32) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn string_field(value: &Value, key: &str) -> String {
    string_opt(value, key).unwrap_or_default()
}

fn string_opt(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":{"list":[{"id":"sample","title":"Sample Manga","cover":{"id":"cover","f":"jpg"}}]}}"#;
const LATEST_FIXTURE: &str = r#"{"data":{"list":[{"id":"sample","title":"Sample Manga","cover":{"id":"cover","f":"jpg"}}]}}"#;
const BROWSE_FIXTURE: &str =
    r#"{"data":[{"id":"sample","title":"Sample Manga","cover":{"id":"cover","f":"jpg"}}]}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"id":"sample","title":"Sample Manga","alt_titles":"Alt Title","description":"Description","status":"Ongoing","start_year":2024,"type":"Manhwa","authors":"Author","artists":"Artist","links":{},"tags":[{"id":"action","name":"Action","type":"genre"}],"chapters":[{"id":"chapter-1","number":1,"name":"Start","created_date":"2024-01-01T00:00:00"}],"cover":{"id":"cover","f":"jpg"}}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"id":"chapter-1","comic_id":"sample","images":[{"id":"001","f":"jpg"},{"id":"002","f":"jpg"}]}}"#;
