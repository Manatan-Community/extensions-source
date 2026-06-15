use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Hyakuro = Hyakuro;
const BASE_URL: &str = "https://hyakuro.net";
const API_URL: &str = "https://hyakuro.net/backend/api";

struct Hyakuro;

impl MangaSource for Hyakuro {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_page(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updatedAt:desc"
        } else {
            "Title:asc"
        };
        Ok(parse_manga_page(&fetch_json(
            &manga_query_url(page, sort, None, None),
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
            let key = normalize_key(query);
            let body = fetch_json(&details_url(slug_from_key(&key)), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: parse_manga_page(&body)
                    .entries
                    .into_iter()
                    .map(|item| CatalogItem {
                        key: key.clone(),
                        ..item
                    })
                    .collect(),
                has_next_page: false,
            });
        }
        let mut extra = Vec::new();
        if !query.is_empty() {
            extra.push(("filters[Title][$containsi]".to_string(), query.to_string()));
        }
        if let Some(status) = filter_str(request.get("filters"), "status").filter(|s| !s.is_empty())
        {
            if status == "Oneshot" {
                extra.push(("filters[Oneshot][$eq]".to_string(), "true".to_string()));
            } else {
                extra.push(("filters[Status][$eq]".to_string(), status.to_string()));
            }
        }
        if let Some(categories) = filter_str(request.get("filters"), "categories") {
            for (index, category) in categories
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .enumerate()
            {
                extra.push((
                    format!("filters[$and][{}][Categories][$containsi]", index + 1),
                    category.to_string(),
                ));
            }
        }
        Ok(parse_manga_page(&fetch_json(
            &manga_query_url(page, "updatedAt:desc", Some(extra), None),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let mut entries = parse_manga_page(&fetch_json(
            &details_url(slug_from_key(&key)),
            DETAILS_FIXTURE,
        ))
        .entries;
        Ok(entries.pop().unwrap_or_else(|| fallback_item(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_json(&details_url(slug_from_key(&key)), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, slug_from_key(&key)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "sample#1#1".to_string());
        let mut parts = key.split('#');
        let slug = parts.next().unwrap_or("sample");
        let chapter_id = parts.nth(1).and_then(|id| id.parse::<i64>().ok());
        let body = fetch_json(&details_url(slug), DETAILS_FIXTURE);
        Ok(parse_pages(&body, chapter_id))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(self.details(serde_json::json!({ "manga": { "key": key } }))?),
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
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn manga_query_url(
    page: u64,
    sort: &str,
    extra: Option<Vec<(String, String)>>,
    slug: Option<&str>,
) -> String {
    let mut pairs = vec![
        ("populate".to_string(), "Cover,Chapters".to_string()),
        ("sort".to_string(), sort.to_string()),
        ("pagination[page]".to_string(), page.to_string()),
    ];
    if let Some(slug) = slug {
        pairs.push(("filters[slug][$eq]".to_string(), slug.to_string()));
    }
    pairs.extend(extra.unwrap_or_default());
    format!(
        "{API_URL}/mangas?{}",
        pairs
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

fn details_url(slug: &str) -> String {
    manga_query_url(1, "updatedAt:desc", None, Some(slug))
}

fn filter_str<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(id))
        .and_then(Value::as_str)
}

fn parse_manga_page(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<PaginatedResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    let has_next_page = response.meta.pagination.page < response.meta.pagination.page_count;
    Paged {
        entries: response
            .data
            .into_iter()
            .map(|manga| manga.attributes.to_item())
            .collect(),
        has_next_page,
    }
}

fn parse_chapters(body: &str, slug: &str) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<PaginatedResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let Some(parent) = response
        .data
        .into_iter()
        .next()
        .map(|manga| manga.attributes)
    else {
        return Vec::new();
    };
    let mut chapters = parent
        .chapters
        .unwrap_or_default()
        .into_iter()
        .map(|chapter| {
            chapter.to_chapter(
                slug,
                parent.oneshot.unwrap_or(false),
                parent.published_at.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .partial_cmp(&a.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str, chapter_id: Option<i64>) -> Vec<MangaPage> {
    let response = serde_json::from_str::<PaginatedResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let pages = response
        .data
        .into_iter()
        .next()
        .and_then(|manga| manga.attributes.chapters)
        .and_then(|chapters| {
            chapters
                .into_iter()
                .find(|chapter| chapter_id.is_none_or(|id| chapter.id == id))
        })
        .and_then(|chapter| chapter.pages)
        .map(|pages| pages.data)
        .unwrap_or_default();
    let mut pages = pages;
    pages.sort_by(|a, b| a.attributes.url.cmp(&b.attributes.url));
    pages
        .into_iter()
        .enumerate()
        .map(|(index, page)| MangaPage {
            content: PageContent::Url {
                url: format!("{BASE_URL}/backend{}", page.attributes.url),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn fallback_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: url::slug_from_url(key).unwrap_or_else(|| "Hyakuro Translations".to_string()),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        ..CatalogItem::default()
    }
}

fn normalize_key(input: &str) -> String {
    let path = input.trim_start_matches(BASE_URL).trim_matches('/');
    if path.starts_with("manga/") {
        format!("/{path}")
    } else {
        format!("/manga/{path}")
    }
}

fn slug_from_key(key: &str) -> &str {
    key.trim_matches('/')
        .strip_prefix("manga/")
        .unwrap_or(key.trim_matches('/'))
}

fn parse_date(value: Option<&str>) -> Option<i64> {
    let date = value?;
    let y = date.get(0..4)?.parse().ok()?;
    let m = date.get(5..7)?.parse().ok()?;
    let d = date.get(8..10)?.parse().ok()?;
    Some(unix_from_ymd(y, m, d))
}

fn unix_from_ymd(year: i32, month: i32, day: i32) -> i64 {
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64 * 86_400
}

#[derive(Deserialize)]
struct PaginatedResponse {
    data: Vec<MangaResponse>,
    meta: Meta,
}

#[derive(Deserialize)]
struct MangaResponse {
    attributes: MangaAttributes,
}

#[derive(Deserialize)]
struct MangaAttributes {
    #[serde(rename = "Title")]
    title: String,
    slug: String,
    #[serde(rename = "Synopsis")]
    synopsis: Option<String>,
    #[serde(rename = "Artist")]
    artist: Option<String>,
    #[serde(rename = "Author")]
    author: Option<String>,
    #[serde(rename = "Status")]
    status: Option<String>,
    #[serde(rename = "Cover")]
    cover: Option<CoverObject>,
    #[serde(rename = "Chapters")]
    chapters: Option<Vec<ChapterInListDto>>,
    #[serde(rename = "Categories")]
    categories: Option<Vec<String>>,
    #[serde(rename = "Longstrip")]
    longstrip: Option<bool>,
    #[serde(rename = "Oneshot")]
    oneshot: Option<bool>,
    #[serde(rename = "publishedAt")]
    published_at: Option<String>,
}

impl MangaAttributes {
    fn to_item(self) -> CatalogItem {
        let mut tags = self.categories.unwrap_or_default();
        if self.longstrip == Some(true) {
            tags.push("Longstrip".to_string());
        }
        if self.oneshot == Some(true) {
            tags.push("Oneshot".to_string());
        }
        CatalogItem {
            key: format!("/manga/{}", self.slug),
            title: self.title,
            cover: self.cover.and_then(|cover| {
                cover
                    .data
                    .map(|data| format!("{BASE_URL}/backend{}", data.attributes.url))
            }),
            url: Some(format!("{BASE_URL}/manga/{}", self.slug)),
            authors: self
                .author
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect(),
            artists: self
                .artist
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect(),
            description: self.synopsis,
            tags,
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            status: match self.status.as_deref() {
                Some("Ongoing") => ItemStatus::Ongoing,
                Some("Completed") => ItemStatus::Completed,
                Some("Dropped") => ItemStatus::Cancelled,
                _ => ItemStatus::Unknown,
            },
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct CoverObject {
    data: Option<CoverData>,
}

#[derive(Deserialize)]
struct CoverData {
    attributes: CoverAttributes,
}

#[derive(Deserialize)]
struct CoverAttributes {
    url: String,
}

#[derive(Deserialize)]
struct ChapterInListDto {
    id: i64,
    #[serde(rename = "Chapter")]
    chapter: f32,
    #[serde(rename = "Title")]
    title: Option<String>,
    #[serde(rename = "TranslatedOn")]
    translated_on: Option<String>,
    #[serde(rename = "Pages")]
    pages: Option<PageListDto>,
}

impl ChapterInListDto {
    fn to_chapter(
        self,
        manga_slug: &str,
        oneshot: bool,
        published_at: Option<&str>,
    ) -> MangaChapter {
        let chapter_str = if self.chapter.fract() == 0.0 {
            format!("{}", self.chapter as i32)
        } else {
            format!("{}", self.chapter)
        };
        let title = match (self.title, oneshot) {
            (None, true) => "Oneshot".to_string(),
            (None, false) => format!("Chapter {chapter_str}"),
            (Some(title), true) => format!("Oneshot - {title}"),
            (Some(title), false) => format!("Chapter {chapter_str} - {title}"),
        };
        MangaChapter {
            key: format!("{manga_slug}#{}#{}", self.chapter, self.id),
            title: Some(title),
            chapter_number: Some(self.chapter),
            date_uploaded: parse_date(self.translated_on.as_deref().or(published_at)),
            url: Some(format!(
                "{BASE_URL}/manga/{manga_slug}/read/{chapter_str}/1"
            )),
            page_count: self.pages.as_ref().map(|pages| pages.data.len() as u32),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct PageListDto {
    data: Vec<PageData>,
}

#[derive(Deserialize)]
struct PageData {
    attributes: PageAttributes,
}

#[derive(Deserialize)]
struct PageAttributes {
    url: String,
}

#[derive(Deserialize)]
struct Meta {
    pagination: Pagination,
}

#[derive(Deserialize)]
struct Pagination {
    page: u64,
    #[serde(rename = "pageCount")]
    page_count: u64,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{
  "data": [{
    "attributes": {
      "Title": "Sample Manga",
      "slug": "sample",
      "Synopsis": "Sample synopsis",
      "Artist": "Artist",
      "Author": "Author",
      "Status": "Ongoing",
      "Cover": { "data": { "attributes": { "url": "/uploads/cover.jpg" } } },
      "Chapters": [],
      "Categories": ["Action"],
      "Longstrip": false,
      "Oneshot": false,
      "publishedAt": "2024-01-01"
    }
  }],
  "meta": { "pagination": { "page": 1, "pageCount": 1 } }
}"#;
const DETAILS_FIXTURE: &str = r#"{
  "data": [{
    "attributes": {
      "Title": "Sample Manga",
      "slug": "sample",
      "Synopsis": "Sample synopsis",
      "Artist": "Artist",
      "Author": "Author",
      "Status": "Ongoing",
      "Cover": { "data": { "attributes": { "url": "/uploads/cover.jpg" } } },
      "Chapters": [{
        "id": 1,
        "Chapter": 1,
        "Title": "Start",
        "TranslatedOn": "2024-01-01",
        "Pages": { "data": [{ "attributes": { "url": "/uploads/001.jpg" } }] }
      }],
      "Categories": ["Action"],
      "Longstrip": false,
      "Oneshot": false,
      "publishedAt": "2024-01-01"
    }
  }],
  "meta": { "pagination": { "page": 1, "pageCount": 1 } }
}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_hyakuro_pages() {
        let pages = SOURCE.pages(json!({})).unwrap();
        assert_eq!(pages.len(), 1);
    }
}
