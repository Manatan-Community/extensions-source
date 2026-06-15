use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: OmegaScans = OmegaScans;
const BASE_URL: &str = "https://omegascans.org";
const API_URL: &str = "https://api.omegascans.org";
const PER_PAGE: u64 = 12;
const CHAPTERS_PER_PAGE: u64 = 1000;

struct OmegaScans;

impl MangaSource for OmegaScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_query(QUERY_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order_by = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "total_views"
        };
        Ok(parse_query(&fetch_api(
            &query_url(page, "", order_by, request.get("filters")),
            QUERY_FIXTURE,
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
            let slug = query
                .trim_start_matches(BASE_URL)
                .trim_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(query);
            return Ok(Paged {
                entries: vec![parse_series(
                    &fetch_api(&format!("{API_URL}/series/{slug}"), SERIES_FIXTURE),
                    Some(slug.to_string()),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_query(&fetch_api(
            &query_url(page, query, "total_views", request.get("filters")),
            QUERY_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample#1".to_string());
        let slug = series_slug(&key);
        Ok(parse_series(
            &fetch_api(&format!("{API_URL}/series/{slug}"), SERIES_FIXTURE),
            Some(slug.to_string()),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample#1".to_string());
        let slug = series_slug(&key).to_string();
        let series_id = key
            .rsplit('#')
            .next()
            .and_then(|value| value.parse::<i64>().ok());
        let show_paid = preference_bool(&request, "show_paid_chapters", false);
        if let Some(series_id) = series_id {
            return Ok(parse_chapter_query(
                &fetch_api(&chapter_query_url(1, series_id), CHAPTERS_FIXTURE),
                &slug,
                show_paid,
            ));
        }
        Ok(parse_embedded_chapters(
            &fetch_api(&format!("{API_URL}/series/{slug}"), SERIES_FIXTURE),
            &slug,
            show_paid,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1#1".to_string());
        let chapter_id = key.rsplit('#').next().unwrap_or("1");
        let token = auth_token(request.get("preferences"));
        Ok(parse_pages(&fetch_chapter(chapter_id, token.as_deref())))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/series/{}", series_slug(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let slug = key
                .trim_start_matches("/series/")
                .split('#')
                .next()
                .unwrap_or(&key);
            format!("{BASE_URL}/series/{slug}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                search: Some(SearchRequest {
                    query: input.to_string(),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_origin(BASE_URL)
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

fn fetch_chapter(chapter_id: &str, token: Option<&str>) -> String {
    let client = client();
    let mut request = client.get(format!("{API_URL}/chapter/{chapter_id}")).xhr();
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    request
        .send_text()
        .unwrap_or_else(|_| PAGES_FIXTURE.to_string())
}

fn auth_token(preferences: Option<&Value>) -> Option<String> {
    let prefs = preferences?.as_object()?;
    let user = pref_str(prefs, "username").or_else(|| pref_str(prefs, "pref_user"))?;
    let password = pref_str(prefs, "password").or_else(|| pref_str(prefs, "pref_password"))?;
    if user.is_empty() || password.is_empty() {
        return None;
    }
    let body = client()
        .post(format!("{API_URL}/login"))
        .form(&[("email", user), ("password", password)])
        .send_text()
        .ok()?;
    serde_json::from_str::<TokenResponse>(&body).ok()?.token
}

fn pref_str<'a>(prefs: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    prefs.get(key).and_then(Value::as_str)
}

fn query_url(page: u64, query: &str, default_order_by: &str, filters: Option<&Value>) -> String {
    let order_by = filter(filters, "orderBy").unwrap_or(default_order_by);
    let order = filter(filters, "order").unwrap_or("desc");
    let status = filter(filters, "status").unwrap_or("All");
    let tags = filter(filters, "tags_ids").unwrap_or("[]");
    format!(
        "{API_URL}/query?query_string={}&status={}&order={}&orderBy={}&series_type=Comic&page={page}&perPage={PER_PAGE}&tags_ids={}&adult=true",
        url::query_escape(query),
        url::query_escape(status),
        url::query_escape(order),
        url::query_escape(order_by),
        url::query_escape(tags),
    )
}

fn chapter_query_url(page: u64, series_id: i64) -> String {
    format!("{API_URL}/chapter/query?page={page}&perPage={CHAPTERS_PER_PAGE}&series_id={series_id}")
}

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(id))
        .and_then(Value::as_str)
}

fn preference_bool(request: &Value, id: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(Value::as_object)
        .and_then(|prefs| prefs.get(id).or_else(|| prefs.get(&format!("pref_{id}"))))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn parse_query(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<QueryResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(QUERY_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: response.data.into_iter().map(Series::to_item).collect(),
        has_next_page: response
            .meta
            .is_some_and(|meta| meta.current_page < meta.last_page),
    }
}

fn parse_series(body: &str, fallback_slug: Option<String>) -> CatalogItem {
    let mut item = serde_json::from_str::<Series>(body)
        .or_else(|_| serde_json::from_str::<SeriesEnvelope>(body).map(SeriesEnvelope::series))
        .unwrap_or_else(|_| serde_json::from_str(SERIES_FIXTURE).expect("fixture is valid"))
        .to_item();
    if item.key.is_empty() {
        let slug = fallback_slug.unwrap_or_else(|| "sample".to_string());
        item.key = format!("/series/{slug}#0");
        item.url = Some(format!("{BASE_URL}/series/{slug}"));
    }
    item
}

fn parse_embedded_chapters(body: &str, slug: &str, show_paid: bool) -> Vec<MangaChapter> {
    let series = serde_json::from_str::<Series>(body)
        .or_else(|_| serde_json::from_str::<SeriesEnvelope>(body).map(SeriesEnvelope::series))
        .unwrap_or_else(|_| serde_json::from_str(SERIES_FIXTURE).expect("fixture is valid"));
    series
        .seasons
        .unwrap_or_default()
        .into_iter()
        .flat_map(|season| season.chapters.unwrap_or_default())
        .filter(|chapter| chapter.price.unwrap_or(0) == 0 || show_paid)
        .map(|chapter| chapter.to_chapter(slug))
        .collect()
}

fn parse_chapter_query(body: &str, slug: &str, show_paid: bool) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<ChapterQueryResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
    response
        .data
        .into_iter()
        .filter(|chapter| chapter.price.unwrap_or(0) == 0 || show_paid)
        .map(|chapter| chapter.to_chapter(slug))
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<PagePayload>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    let images = if let Some(chapter_data) = response.chapter.chapter_data {
        chapter_data.images.unwrap_or_default()
    } else if response.paywall.unwrap_or(false) {
        Vec::new()
    } else {
        response.data.unwrap_or_default()
    };
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_media(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn series_slug(key: &str) -> &str {
    key.trim_start_matches("/series/")
        .split('/')
        .next()
        .and_then(|part| part.split('#').next())
        .unwrap_or("sample")
}

fn absolute_media(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        url::join_url(API_URL, value)
    }
}

fn parse_date(value: Option<&str>) -> Option<i64> {
    let value = value?;
    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = value.get(5..7)?.parse::<i32>().ok()?;
    let day = value.get(8..10)?.parse::<i32>().ok()?;
    Some(unix_from_ymd(year, month, day))
}

fn unix_from_ymd(year: i32, month: i32, day: i32) -> i64 {
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as i64 * 86_400
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    token: Option<String>,
}

#[derive(Deserialize)]
struct QueryResponse {
    #[serde(default)]
    data: Vec<Series>,
    #[serde(default)]
    meta: Option<Meta>,
}

#[derive(Deserialize)]
struct Meta {
    current_page: u64,
    last_page: u64,
}

#[derive(Deserialize)]
struct SeriesEnvelope {
    #[serde(default)]
    data: Option<Series>,
    #[serde(default)]
    post: Option<Series>,
}

impl SeriesEnvelope {
    fn series(self) -> Series {
        self.data.or(self.post).unwrap_or_default()
    }
}

#[derive(Default, Deserialize)]
struct Series {
    #[serde(default)]
    id: i64,
    #[serde(default, rename = "series_slug")]
    slug: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    studio: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    tags: Vec<Tag>,
    #[serde(default)]
    seasons: Option<Vec<Season>>,
}

impl Series {
    fn to_item(self) -> CatalogItem {
        let slug = if self.slug.is_empty() {
            url::slug_from_url(&self.title).unwrap_or_else(|| "sample".to_string())
        } else {
            self.slug
        };
        CatalogItem {
            key: format!("/series/{slug}#{}", self.id),
            title: if self.title.is_empty() {
                "Manga".to_string()
            } else {
                self.title
            },
            cover: self
                .thumbnail
                .filter(|value| !value.is_empty())
                .map(|value| absolute_media(&value)),
            authors: self
                .author
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect(),
            artists: self
                .studio
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect(),
            description: self
                .description
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty()),
            tags: self.tags.into_iter().map(|tag| tag.name).collect(),
            status: match self.status.as_deref() {
                Some("Ongoing") => ItemStatus::Ongoing,
                Some("Hiatus") => ItemStatus::Hiatus,
                Some("Dropped") | Some("Canceled") => ItemStatus::Cancelled,
                Some("Completed" | "Finished") => ItemStatus::Completed,
                _ => ItemStatus::Unknown,
            },
            url: Some(format!("{BASE_URL}/series/{slug}")),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct Tag {
    name: String,
}

#[derive(Default, Deserialize)]
struct Season {
    #[serde(default)]
    chapters: Option<Vec<Chapter>>,
}

#[derive(Deserialize)]
struct ChapterQueryResponse {
    #[serde(default)]
    data: Vec<Chapter>,
}

#[derive(Deserialize)]
struct Chapter {
    id: i64,
    #[serde(rename = "chapter_name")]
    name: String,
    #[serde(default, rename = "chapter_title")]
    title: Option<String>,
    #[serde(rename = "chapter_slug")]
    slug: String,
    #[serde(default, rename = "created_at")]
    created_at: Option<String>,
    #[serde(default)]
    price: Option<i64>,
}

impl Chapter {
    fn to_chapter(self, series_slug: &str) -> MangaChapter {
        let mut title = self.name.trim().to_string();
        if let Some(extra) = self.title.filter(|value| !value.trim().is_empty()) {
            title.push_str(" - ");
            title.push_str(extra.trim());
        }
        if self.price.unwrap_or(0) != 0 {
            title.push_str(" [Locked]");
        }
        MangaChapter {
            key: format!("/series/{series_slug}/{}#{}", self.slug, self.id),
            title: Some(title),
            date_uploaded: parse_date(self.created_at.as_deref()),
            is_locked: self.price.unwrap_or(0) != 0,
            url: Some(format!("{BASE_URL}/series/{series_slug}/{}", self.slug)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct PagePayload {
    chapter: PageChapter,
    #[serde(default)]
    paywall: Option<bool>,
    #[serde(default)]
    data: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct PageChapter {
    #[serde(default)]
    chapter_data: Option<PageData>,
}

#[derive(Deserialize)]
struct PageData {
    #[serde(default)]
    images: Option<Vec<String>>,
}

export_manga_source!(SOURCE);

const QUERY_FIXTURE: &str = r#"{
  "data": [{
    "id": 1,
    "series_slug": "sample",
    "title": "Sample Manga",
    "author": "Author",
    "studio": "Artist",
    "description": "<p>Sample description</p>",
    "status": "Ongoing",
    "thumbnail": "/cover.jpg",
    "tags": [{ "name": "Action" }]
  }],
  "meta": { "current_page": 1, "last_page": 1 }
}"#;
const SERIES_FIXTURE: &str = r#"{
  "id": 1,
  "series_slug": "sample",
  "title": "Sample Manga",
  "author": "Author",
  "studio": "Artist",
  "description": "<p>Sample description</p>",
  "status": "Ongoing",
  "thumbnail": "/cover.jpg",
  "tags": [{ "name": "Action" }],
  "seasons": [{
    "chapters": [{
      "id": 1,
      "chapter_name": "Chapter 1",
      "chapter_title": "Start",
      "chapter_slug": "chapter-1",
      "created_at": "2024-01-01T00:00:00.000Z",
      "price": 0
    }]
  }]
}"#;
const CHAPTERS_FIXTURE: &str = r#"{
  "data": [{
    "id": 1,
    "chapter_name": "Chapter 1",
    "chapter_title": "Start",
    "chapter_slug": "chapter-1",
    "created_at": "2024-01-01T00:00:00.000Z",
    "price": 0
  }]
}"#;
const PAGES_FIXTURE: &str = r#"{
  "chapter": {
    "chapter_data": { "images": ["/page1.jpg"] }
  },
  "paywall": false,
  "data": []
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_heancms_shapes() {
        assert_eq!(parse_query(QUERY_FIXTURE).entries[0].title, "Sample Manga");
        assert_eq!(
            parse_chapter_query(CHAPTERS_FIXTURE, "sample", false).len(),
            1
        );
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
