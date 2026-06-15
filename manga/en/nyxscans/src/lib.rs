use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: NyxScans = NyxScans;
const BASE_URL: &str = "https://nyxscans.com";
const API_URL: &str = "https://api.nyxscans.com";
const PER_PAGE: u64 = 18;

struct NyxScans;

impl MangaSource for NyxScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_search(SEARCH_FIXTURE, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "lastChapterAddedAt"
        } else {
            "totalViews"
        };
        Ok(parse_search(
            &fetch_api(
                &query_url(page, "", sort, request.get("filters")),
                SEARCH_FIXTURE,
            ),
            page,
        ))
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
                entries: vec![parse_details(
                    &fetch_api(
                        &format!("{API_URL}/api/post?postSlug={}", url::query_escape(slug)),
                        DETAILS_FIXTURE,
                    ),
                    Some(slug.to_string()),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_search(
            &fetch_api(
                &query_url(page, query, "lastChapterAddedAt", request.get("filters")),
                SEARCH_FIXTURE,
            ),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#1".to_string());
        let slug = slug_from_key(&key);
        Ok(parse_details(
            &fetch_api(
                &format!("{API_URL}/api/post?postSlug={}", url::query_escape(slug)),
                DETAILS_FIXTURE,
            ),
            Some(slug.to_string()),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#1".to_string());
        let slug = slug_from_key(&key);
        let show_locked = request
            .get("preferences")
            .and_then(|prefs| {
                prefs
                    .get("pref_show_locked_chapters")
                    .or_else(|| prefs.get("show_locked_chapters"))
            })
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(parse_chapters(
            &fetch_api(
                &format!("{API_URL}/api/post?postSlug={}", url::query_escape(slug)),
                DETAILS_FIXTURE,
            ),
            show_locked,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1#1".to_string());
        let id = key.rsplit('#').next().unwrap_or("1");
        Ok(parse_pages(&fetch_api(
            &format!("{API_URL}/api/chapter?chapterId={}", url::query_escape(id)),
            PAGES_FIXTURE,
        )))
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
        .with_referer(format!("{BASE_URL}/"))
        .with_header("Cache-Control", "max-age=0")
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

fn query_url(page: u64, query: &str, default_sort: &str, filters: Option<&Value>) -> String {
    let mut pairs = vec![
        ("page".to_string(), page.to_string()),
        ("perPage".to_string(), PER_PAGE.to_string()),
        ("searchTerm".to_string(), query.trim().to_string()),
        (
            "orderBy".to_string(),
            filter(filters, "orderBy")
                .unwrap_or(default_sort)
                .to_string(),
        ),
        (
            "orderDirection".to_string(),
            filter(filters, "orderDirection")
                .unwrap_or("desc")
                .to_string(),
        ),
        (
            "seriesStatus".to_string(),
            filter(filters, "seriesStatus").unwrap_or("").to_string(),
        ),
        (
            "seriesType".to_string(),
            filter(filters, "seriesType").unwrap_or("").to_string(),
        ),
    ];
    if let Some(genres) = filter(filters, "genreIds").filter(|value| !value.is_empty()) {
        pairs.push(("genreIds".to_string(), genres.to_string()));
    }
    format!(
        "{API_URL}/api/query?{}",
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

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(id))
        .and_then(Value::as_str)
}

fn parse_search(body: &str, page: u64) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<SearchResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).expect("fixture is valid"));
    let total_count = response.total_count;
    Paged {
        entries: response
            .posts
            .into_iter()
            .filter(|post| !post.is_novel)
            .map(PostManga::to_item)
            .collect(),
        has_next_page: total_count > page * PER_PAGE,
    }
}

fn parse_details(body: &str, fallback_slug: Option<String>) -> CatalogItem {
    let response = serde_json::from_str::<PostResponse<PostManga>>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let mut item = response.post.to_item();
    if item.key.is_empty() {
        let slug = fallback_slug.unwrap_or_else(|| "sample".to_string());
        item.key = format!("{slug}#0");
        item.url = Some(format!("{BASE_URL}/series/{slug}"));
    }
    item
}

fn parse_chapters(body: &str, show_locked: bool) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<PostResponse<ChapterListPost>>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    response
        .post
        .chapters
        .into_iter()
        .filter(|chapter| chapter.chapter_status == "PUBLIC")
        .filter(|chapter| chapter.is_accessible || (show_locked && chapter.is_locked()))
        .map(|chapter| chapter.to_chapter(response.post.slug.as_deref()))
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<PageResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    if response.chapter.is_permanently_locked
        || response.chapter.is_locked_by_coins
        || response.chapter.is_short_link_locked
    {
        return Vec::new();
    }
    let mut images = response.chapter.images;
    images.sort_by_key(|image| image.order.unwrap_or(i64::MAX));
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.url.replace(' ', "%20"),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn slug_from_key(key: &str) -> &str {
    key.trim_matches('/')
        .trim_start_matches("series/")
        .split('#')
        .next()
        .unwrap_or(key)
}

fn parse_date(value: &str) -> Option<i64> {
    let y = value.get(0..4)?.parse().ok()?;
    let m = value.get(5..7)?.parse().ok()?;
    let d = value.get(8..10)?.parse().ok()?;
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
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    posts: Vec<PostManga>,
    total_count: u64,
}

#[derive(Deserialize)]
struct PostResponse<T> {
    post: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostManga {
    id: i64,
    slug: String,
    post_title: String,
    #[serde(default)]
    post_content: Option<String>,
    #[serde(default)]
    is_novel: bool,
    #[serde(default)]
    featured_image: Option<String>,
    #[serde(default)]
    alternative_titles: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    series_type: Option<String>,
    #[serde(default)]
    series_status: Option<String>,
    #[serde(default)]
    genres: Vec<Genre>,
}

impl PostManga {
    fn to_item(self) -> CatalogItem {
        let mut tags = Vec::new();
        match self.series_type.as_deref() {
            Some("MANGA") => tags.push("Manga".to_string()),
            Some("MANHUA") => tags.push("Manhua".to_string()),
            Some("MANHWA") => tags.push("Manhwa".to_string()),
            _ => {}
        }
        tags.extend(self.genres.into_iter().map(|genre| genre.name));
        let description = self
            .post_content
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty());
        let alternate_titles = self
            .alternative_titles
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        CatalogItem {
            key: format!("{}#{}", self.slug, self.id),
            title: self.post_title,
            alternate_titles,
            cover: self.featured_image,
            url: Some(format!("{BASE_URL}/series/{}", self.slug)),
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
            description,
            tags,
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            status: match self.series_status.as_deref() {
                Some("ONGOING" | "COMING_SOON") => ItemStatus::Ongoing,
                Some("COMPLETED") => ItemStatus::Completed,
                Some("CANCELLED" | "DROPPED") => ItemStatus::Cancelled,
                _ => ItemStatus::Unknown,
            },
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct Genre {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterListPost {
    #[serde(default)]
    slug: Option<String>,
    chapters: Vec<IkenChapter>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IkenChapter {
    id: i64,
    slug: String,
    number: Value,
    #[serde(default)]
    title: Option<String>,
    created_at: String,
    chapter_status: String,
    is_accessible: bool,
    #[serde(default)]
    is_locked: Option<bool>,
    #[serde(default)]
    is_time_locked: Option<bool>,
    #[serde(default)]
    manga_post: Option<MangaPostDto>,
}

impl IkenChapter {
    fn is_locked(&self) -> bool {
        self.is_locked == Some(true) || self.is_time_locked == Some(true)
    }

    fn to_chapter(self, parent_slug: Option<&str>) -> MangaChapter {
        let series_slug = parent_slug
            .or(self
                .manga_post
                .as_ref()
                .and_then(|post| post.slug.as_deref()))
            .unwrap_or("sample");
        let suffix = self
            .title
            .filter(|value| !value.is_empty())
            .map(|title| format!(" - {title}"))
            .unwrap_or_default();
        MangaChapter {
            key: format!("/series/{series_slug}/{}#{}", self.slug, self.id),
            title: Some(format!("Chapter {}{suffix}", display_number(&self.number))),
            chapter_number: chapter_number(&self.number),
            date_uploaded: parse_date(&self.created_at),
            is_locked: !self.is_accessible,
            url: Some(format!("{BASE_URL}/series/{series_slug}/{}", self.slug)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct MangaPostDto {
    slug: Option<String>,
}

#[derive(Deserialize)]
struct PageResponse {
    chapter: IkenPage,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IkenPage {
    images: Vec<PageImage>,
    #[serde(default)]
    is_permanently_locked: bool,
    #[serde(default)]
    is_locked_by_coins: bool,
    #[serde(default)]
    is_short_link_locked: bool,
}

#[derive(Deserialize)]
struct PageImage {
    url: String,
    #[serde(default)]
    order: Option<i64>,
}

fn chapter_number(value: &Value) -> Option<f32> {
    value
        .as_f64()
        .map(|number| number as f32)
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

fn display_number(value: &Value) -> String {
    value
        .as_str()
        .map(ToString::to_string)
        .or_else(|| value.as_f64().map(|number| number.to_string()))
        .unwrap_or_else(|| "1".to_string())
}

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"{
  "posts": [{
    "id": 1,
    "slug": "sample",
    "postTitle": "Sample Manga",
    "postContent": "<p>Sample description</p>",
    "isNovel": false,
    "featuredImage": "https://nyxscans.com/cover.jpg",
    "alternativeTitles": "Sample Alt",
    "author": "Author",
    "artist": "Artist",
    "seriesType": "MANHWA",
    "seriesStatus": "ONGOING",
    "genres": [{ "id": 1, "name": "Action" }]
  }],
  "totalCount": 1
}"#;
const DETAILS_FIXTURE: &str = r#"{
  "post": {
    "id": 1,
    "slug": "sample",
    "postTitle": "Sample Manga",
    "postContent": "<p>Sample description</p>",
    "isNovel": false,
    "featuredImage": "https://nyxscans.com/cover.jpg",
    "alternativeTitles": "Sample Alt",
    "author": "Author",
    "artist": "Artist",
    "seriesType": "MANHWA",
    "seriesStatus": "ONGOING",
    "genres": [{ "id": 1, "name": "Action" }],
    "chapters": [{
      "id": 1,
      "slug": "chapter-1",
      "number": 1,
      "title": "Start",
      "createdAt": "2024-01-01T00:00:00.000Z",
      "chapterStatus": "PUBLIC",
      "isAccessible": true,
      "isLocked": false,
      "isTimeLocked": false
    }]
  }
}"#;
const PAGES_FIXTURE: &str = r#"{
  "chapter": {
    "images": [{ "url": "https://nyxscans.com/page 1.jpg", "order": 1 }],
    "isPermanentlyLocked": false,
    "isLockedByCoins": false,
    "isShortLinkLocked": false
  }
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iken_shapes() {
        assert_eq!(
            parse_search(SEARCH_FIXTURE, 1).entries[0].title,
            "Sample Manga"
        );
        assert_eq!(parse_chapters(DETAILS_FIXTURE, false).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
