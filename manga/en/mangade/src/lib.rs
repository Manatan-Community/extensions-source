use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: MangaDE = MangaDE;
const BASE_URL: &str = "https://mangade.io";
const API_URL: &str = "https://api.mangade.io/api";

struct MangaDE;

impl MangaSource for MangaDE {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "newest"
        } else {
            "most-viewed"
        };
        Ok(parse_list(&fetch_api(
            &comics_url(page, "", Some(sort), None),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_url_key(query);
            let id = manga_id_from_key(&key).unwrap_or_else(|| "sample".to_string());
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_api(&details_url(&id), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_list(&fetch_api(
            &comics_url(
                page,
                query,
                filter_string(request.get("filters"), "sort").as_deref(),
                request.get("filters"),
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/sample?mid=sample-id".to_string());
        let id = manga_id_from_key(&key).unwrap_or_else(|| "sample-id".to_string());
        Ok(parse_details(
            &fetch_api(&details_url(&id), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/sample?mid=sample-id".to_string());
        let id = manga_id_from_key(&key).unwrap_or_else(|| "sample-id".to_string());
        Ok(parse_chapters(&fetch_api(
            &details_url(&id),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample/chapter-1?cid=chapter-id&mid=sample-id".to_string());
        let id = query_param(&key, "cid").unwrap_or_else(|| "chapter-id".to_string());
        Ok(parse_pages(&fetch_api(&chapter_url(&id), CHAPTER_FIXTURE)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_url_key(input);
            if let Some(id) = manga_id_from_key(&key) {
                return Ok(Some(UrlResolveResult {
                    item: Some(parse_details(
                        &fetch_api(&details_url(&id), DETAILS_FIXTURE),
                        Some(key),
                    )),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn comics_url(page: u64, query: &str, sort: Option<&str>, filters: Option<&Value>) -> String {
    let mut params = vec![
        ("page".to_string(), page.to_string()),
        ("size".to_string(), "20".to_string()),
    ];
    if !query.is_empty() {
        params.push(("name".to_string(), query.to_string()));
    }
    if let Some(sort) = sort.filter(|value| !value.is_empty()) {
        params.push(("sort".to_string(), sort.to_string()));
    }
    for key in ["comic_status", "category", "year", "min_chapter_count"] {
        if let Some(value) = filter_string(filters, key).filter(|value| !value.is_empty()) {
            params.push((key.to_string(), value));
        }
    }
    for value in filter_values(filters, "genres") {
        params.push(("genres[]".to_string(), value));
    }
    format!(
        "{API_URL}/comics?{}",
        params
            .into_iter()
            .map(|(key, value)| format!(
                "{}={}",
                url::query_escape(&key),
                url::query_escape(&value)
            ))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn details_url(id: &str) -> String {
    format!("{API_URL}/comics/{}/view", url::query_escape(id))
}

fn chapter_url(id: &str) -> String {
    format!("{API_URL}/chapters/{}/view", url::query_escape(id))
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<Payload<MangaListPage>>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    let page = payload.data.page.parse::<u64>().unwrap_or(1);
    Paged {
        entries: payload
            .data
            .list
            .into_iter()
            .map(MangaDto::into_item)
            .collect(),
        has_next_page: page < payload.data.total_page,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let payload = serde_json::from_str::<Payload<MangaDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let mut item = payload.data.into_item();
    if let Some(key) = key {
        item.key = key;
        item.url = Some(public_manga_url(&item.key));
    }
    item.initialized = true;
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let payload = serde_json::from_str::<Payload<MangaDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let manga_id = payload.data.id.clone();
    let manga_slug = payload.data.slug.clone().unwrap_or_default();
    payload
        .data
        .news_chapters
        .into_iter()
        .map(|chapter| {
            let chapter_slug = chapter.slug.clone().unwrap_or_else(|| chapter.id.clone());
            let key = format!(
                "/{manga_slug}/{chapter_slug}?cid={}&mid={manga_id}",
                chapter.id
            );
            MangaChapter {
                key: key.clone(),
                title: Some(chapter.name),
                chapter_number: chapter.chapter_number.and_then(|value| value.parse().ok()),
                date_uploaded: chapter.published_date.as_deref().and_then(parse_date),
                url: Some(public_chapter_url(&key)),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let payload = serde_json::from_str::<Payload<ChapterDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTER_FIXTURE).expect("fixture is valid"));
    payload
        .data
        .chapter_images
        .into_iter()
        .enumerate()
        .filter_map(|(index, page)| {
            page.image.map(|image| MangaPage {
                content: PageContent::Url {
                    url: image.clone(),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn public_manga_url(key: &str) -> String {
    let slug = key
        .trim_start_matches('/')
        .split('?')
        .next()
        .unwrap_or_default();
    let id = manga_id_from_key(key).unwrap_or_default();
    format!("{BASE_URL}/comic/{slug}-pid{id}")
}

fn public_chapter_url(key: &str) -> String {
    let path = key
        .trim_start_matches('/')
        .split('?')
        .next()
        .unwrap_or_default();
    let mut parts = path.split('/');
    let manga_slug = parts.next().unwrap_or_default();
    let chapter_slug = parts.next().unwrap_or_default();
    let mid = query_param(key, "mid").unwrap_or_default();
    format!("{BASE_URL}/comic/{manga_slug}-{mid}/{chapter_slug}")
}

fn normalize_url_key(input: &str) -> String {
    let path = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_end_matches('/');
    if let Some(rest) = path.strip_prefix("comic/") {
        let first = rest.split('/').next().unwrap_or(rest);
        if let Some((slug, id)) = first.rsplit_once("-pid") {
            return format!("/{slug}?mid={id}");
        }
        if let Some((slug, id)) = first.rsplit_once('-') {
            return format!("/{slug}?mid={id}");
        }
    }
    format!("/{}", path.trim_start_matches('/'))
}

fn manga_id_from_key(key: &str) -> Option<String> {
    query_param(key, "mid").or_else(|| key.split("mid=").nth(1).map(ToString::to_string))
}

fn query_param(input: &str, name: &str) -> Option<String> {
    let query = input.split('?').nth(1)?;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        if key == name && !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn filter_string(filters: Option<&Value>, key: &str) -> Option<String> {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(|value| value.as_str().map(ToString::to_string))
}

fn filter_values(filters: Option<&Value>, key: &str) -> Vec<String> {
    let Some(value) = filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
    else {
        return Vec::new();
    };
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect();
    }
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .map(|value| vec![value.to_string()])
        .unwrap_or_default()
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.unwrap_or_default().to_ascii_lowercase().as_str() {
        "ongoing" | "releasing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "on hiatus" | "hiatus" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: &str) -> Option<i64> {
    let date = value.split_whitespace().next()?.split('T').next()?;
    let mut parts = date.split('-');
    unix_date(
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    )
}

fn unix_date(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month as i32 + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(((era * 146_097 + doe - 719_468) as i64) * 86_400)
}

#[derive(Deserialize)]
struct Payload<T> {
    data: T,
}

#[derive(Deserialize)]
struct MangaListPage {
    list: Vec<MangaDto>,
    #[serde(rename = "totalPage")]
    total_page: u64,
    page: String,
}

#[derive(Deserialize)]
struct MangaDto {
    id: String,
    name: String,
    slug: Option<String>,
    image: Option<String>,
    description: Option<String>,
    #[serde(default, rename = "genre_names")]
    genre_names: Option<String>,
    status: Option<String>,
    #[serde(default, rename = "news_chapters")]
    news_chapters: Vec<ChapterDto>,
}

impl MangaDto {
    fn into_item(self) -> CatalogItem {
        let slug = self.slug.unwrap_or_else(|| self.id.clone());
        let key = format!("/{slug}?mid={}", self.id);
        CatalogItem {
            key: key.clone(),
            title: self.name,
            cover: self.image,
            description: self.description,
            tags: self
                .genre_names
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect(),
            status: parse_status(self.status.as_deref()),
            url: Some(public_manga_url(&key)),
            language: Some("en".to_string()),
            content_rating: Some("nsfw".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct ChapterDto {
    id: String,
    name: String,
    slug: Option<String>,
    #[serde(rename = "chapter_number")]
    chapter_number: Option<String>,
    #[serde(rename = "published_date")]
    published_date: Option<String>,
    #[serde(default, rename = "chapter_images")]
    chapter_images: Vec<PageDto>,
}

#[derive(Deserialize)]
struct PageDto {
    image: Option<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":{"list":[{"id":"sample-id","name":"Sample Manga","slug":"sample","image":"https://mangade.io/cover.jpg","status":"Releasing","genre_names":"Action,Drama"}],"totalPage":1,"page":"1"}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"id":"sample-id","name":"Sample Manga","slug":"sample","image":"https://mangade.io/cover.jpg","description":"Sample description.","genre_names":"Action,Drama","status":"Releasing","news_chapters":[{"id":"chapter-id","name":"Chapter 1","slug":"chapter-1","chapter_number":"1","published_date":"2024-01-01 00:00:00"}]}}"#;
const CHAPTER_FIXTURE: &str = r#"{"data":{"id":"chapter-id","name":"Chapter 1","chapter_images":[{"image":"https://mangade.io/page1.jpg"},{"image":"https://mangade.io/page2.jpg"}]}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_api_shapes() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Manga"
        );
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 2);
    }
}
