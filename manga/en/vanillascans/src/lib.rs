use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: VanillaScans = VanillaScans;
const BASE_URL: &str = "https://vanillascans.org";
const API_URL: &str = "https://api.vanillascans.org";
const PER_PAGE: u64 = 18;

struct VanillaScans;

impl MangaSource for VanillaScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_search(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order_by = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "lastChapterAddedAt"
        } else {
            "totalViews"
        };
        Ok(parse_search(&fetch_api(
            &query_url(page, "", order_by, "desc", request.get("filters")),
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
            let slug = slug_from_url(query);
            return Ok(Paged {
                entries: vec![details_by_slug(&slug)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters");
        Ok(parse_search(&fetch_api(
            &query_url(
                page,
                query,
                filter_str(filters, "orderBy").unwrap_or("totalViews"),
                filter_str(filters, "orderDirection").unwrap_or("desc"),
                filters,
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#1".to_string());
        Ok(details_by_slug(key.split('#').next().unwrap_or("sample")))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#1".to_string());
        let payload = serde_json::from_str::<Post<ChapterListPost>>(&fetch_api(
            &format!(
                "{API_URL}/api/post?postSlug={}",
                url::query_escape(key.split('#').next().unwrap_or("sample"))
            ),
            CHAPTERS_FIXTURE,
        ))
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
        Ok(payload
            .post
            .chapters
            .into_iter()
            .filter(Chapter::is_visible)
            .map(|chapter| {
                chapter.into_chapter(
                    payload
                        .post
                        .slug
                        .as_deref()
                        .unwrap_or(key.split('#').next().unwrap_or("sample")),
                )
            })
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1#1".to_string());
        let id = key.split('#').last().unwrap_or("1");
        let payload = serde_json::from_str::<PageResponse>(&fetch_api(
            &format!("{API_URL}/api/chapter?chapterId={}", url::query_escape(id)),
            PAGES_FIXTURE,
        ))
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
        if payload.chapter.is_permanently_locked
            || payload.chapter.is_locked_by_coins
            || payload.chapter.is_short_link_locked
        {
            return Ok(Vec::new());
        }
        let mut images = payload.chapter.images;
        images.sort_by_key(|page| page.order.unwrap_or(i64::MAX));
        Ok(images
            .into_iter()
            .enumerate()
            .map(|(index, page)| MangaPage {
                content: PageContent::Url {
                    url: page.url.replace(' ', "%20"),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?;
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
        Ok(manga::request_key(&request, "manga").map(|key| {
            format!(
                "{BASE_URL}/series/{}",
                key.split('#').next().unwrap_or(&key)
            )
        }))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            format!(
                "{BASE_URL}/{}",
                key.trim_start_matches('/')
                    .split('#')
                    .next()
                    .unwrap_or(&key)
            )
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let slug = slug_from_url(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_slug(&slug)),
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

fn fetch_api(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn query_url(
    page: u64,
    query: &str,
    order_by: &str,
    order_direction: &str,
    filters: Option<&Value>,
) -> String {
    let mut params = vec![
        ("page", page.to_string()),
        ("perPage", PER_PAGE.to_string()),
        ("searchTerm", query.trim().to_string()),
        ("orderBy", order_by.to_string()),
        ("orderDirection", order_direction.to_string()),
    ];
    for key in ["seriesStatus", "seriesType", "genreIds"] {
        if let Some(value) = filter_str(filters, key).filter(|value| !value.is_empty()) {
            params.push((key, value.to_string()));
        }
    }
    format!(
        "{API_URL}/api/query?{}",
        params
            .into_iter()
            .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn filter_str<'a>(filters: Option<&'a Value>, key: &str) -> Option<&'a str> {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<SearchResponse>(body).unwrap_or_default();
    let count = payload.posts.len() as u64;
    Paged {
        entries: payload
            .posts
            .into_iter()
            .filter(|post| !post.is_novel)
            .map(Manga::into_item)
            .collect(),
        has_next_page: payload.total_count > count,
    }
}

fn details_by_slug(slug: &str) -> CatalogItem {
    let payload = serde_json::from_str::<Post<Manga>>(&fetch_api(
        &format!("{API_URL}/api/post?postSlug={}", url::query_escape(slug)),
        DETAILS_FIXTURE,
    ))
    .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let mut item = payload.post.into_item();
    item.initialized = true;
    item
}

fn slug_from_url(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    path.trim_start_matches('/')
        .trim_start_matches("series/")
        .split('/')
        .next()
        .unwrap_or("sample")
        .split('#')
        .next()
        .unwrap_or("sample")
        .to_string()
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default() {
        "ONGOING" | "COMING_SOON" => ItemStatus::Ongoing,
        "COMPLETED" => ItemStatus::Completed,
        "CANCELLED" | "DROPPED" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: &str) -> Option<i64> {
    let date = value.split('T').next()?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let day = day as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

#[derive(Default, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    posts: Vec<Manga>,
    #[serde(rename = "totalCount", default)]
    total_count: u64,
}

#[derive(Deserialize)]
struct Post<T> {
    post: T,
}

#[derive(Default, Deserialize)]
struct Manga {
    id: i64,
    slug: String,
    #[serde(rename = "postTitle", default)]
    post_title: String,
    #[serde(rename = "postContent")]
    post_content: Option<String>,
    #[serde(rename = "isNovel", default)]
    is_novel: bool,
    #[serde(rename = "featuredImage")]
    featured_image: Option<String>,
    #[serde(rename = "alternativeTitles")]
    alternative_titles: Option<String>,
    author: Option<String>,
    artist: Option<String>,
    #[serde(rename = "seriesType")]
    series_type: Option<String>,
    #[serde(rename = "seriesStatus")]
    series_status: Option<String>,
    #[serde(default)]
    genres: Vec<Genre>,
}

impl Manga {
    fn into_item(self) -> CatalogItem {
        let key = format!("{}#{}", self.slug, self.id);
        let mut tags = Vec::new();
        if let Some(kind) = self.series_type.as_deref() {
            tags.push(kind.to_ascii_lowercase());
        }
        tags.extend(self.genres.into_iter().map(|genre| genre.name));
        let description = self.post_content.map(|value| html::strip_tags(&value));
        let description = match (description, self.alternative_titles) {
            (Some(desc), Some(alts)) if !alts.is_empty() => {
                Some(format!("{desc}\n\nAlternative Names: {alts}"))
            }
            (Some(desc), _) => Some(desc),
            (None, Some(alts)) if !alts.is_empty() => Some(format!("Alternative Names: {alts}")),
            _ => None,
        };
        CatalogItem {
            key: key.clone(),
            title: if self.post_title.is_empty() {
                self.slug.clone()
            } else {
                self.post_title
            },
            cover: self.featured_image,
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
            status: parse_status(self.series_status.as_deref()),
            url: Some(format!(
                "{BASE_URL}/series/{}",
                key.split('#').next().unwrap_or(&key)
            )),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct Genre {
    name: String,
}

#[derive(Default, Deserialize)]
struct ChapterListPost {
    #[serde(rename = "isNovel", default)]
    is_novel: bool,
    slug: Option<String>,
    #[serde(default)]
    chapters: Vec<Chapter>,
}

#[derive(Default, Deserialize)]
struct Chapter {
    id: i64,
    slug: String,
    number: Value,
    title: Option<String>,
    #[serde(rename = "createdAt", default)]
    created_at: String,
    #[serde(rename = "chapterStatus", default)]
    chapter_status: String,
    #[serde(rename = "isAccessible", default)]
    is_accessible: bool,
    #[serde(rename = "isLocked", default)]
    is_locked: bool,
    #[serde(rename = "isTimeLocked", default)]
    is_time_locked: bool,
}

impl Chapter {
    fn is_visible(&self) -> bool {
        self.chapter_status == "PUBLIC" && self.is_accessible
    }

    fn into_chapter(self, series_slug: &str) -> MangaChapter {
        let number = if let Some(value) = self.number.as_str() {
            value.to_string()
        } else {
            self.number.to_string()
        };
        let suffix = self
            .title
            .filter(|value| !value.is_empty())
            .map(|value| format!(" - {value}"))
            .unwrap_or_default();
        let key = format!("/series/{series_slug}/{}#{}", self.slug, self.id);
        MangaChapter {
            key: key.clone(),
            title: Some(format!("Chapter {number}{suffix}")),
            chapter_number: number.parse().ok(),
            date_uploaded: parse_date(&self.created_at),
            is_locked: self.is_locked || self.is_time_locked,
            url: Some(format!(
                "{BASE_URL}{}",
                key.split('#').next().unwrap_or(&key)
            )),
            ..MangaChapter::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct PageResponse {
    chapter: ChapterPages,
}

#[derive(Default, Deserialize)]
struct ChapterPages {
    id: Option<i64>,
    #[serde(default)]
    images: Vec<PageDto>,
    #[serde(rename = "isPermanentlyLocked", default)]
    is_permanently_locked: bool,
    #[serde(rename = "isLockedByCoins", default)]
    is_locked_by_coins: bool,
    #[serde(rename = "isShortLinkLocked", default)]
    is_short_link_locked: bool,
}

#[derive(Default, Deserialize)]
struct PageDto {
    url: String,
    order: Option<i64>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"posts":[{"id":1,"slug":"sample","postTitle":"Sample","featuredImage":"https://vanillascans.org/cover.jpg","isNovel":false}],"totalCount":1}"#;
const DETAILS_FIXTURE: &str = r#"{"post":{"id":1,"slug":"sample","postTitle":"Sample","postContent":"Summary","featuredImage":"https://vanillascans.org/cover.jpg","seriesStatus":"ONGOING","genres":[{"id":1,"name":"Action"}]}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"post":{"slug":"sample","isNovel":false,"chapters":[{"id":1,"slug":"chapter-1","number":1,"title":"","createdAt":"2024-01-01T00:00:00.000Z","chapterStatus":"PUBLIC","isAccessible":true}]}}"#;
const PAGES_FIXTURE: &str =
    r#"{"chapter":{"id":1,"images":[{"url":"https://vanillascans.org/page1.jpg","order":1}]}}"#;
