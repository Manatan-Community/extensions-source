use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: VortexScans = VortexScans;
const BASE_URL: &str = "https://vortexscans.org";
const API_URL: &str = "https://api.vortexscans.org";
const PER_PAGE: u64 = 18;

struct VortexScans;

impl MangaSource for VortexScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_query_response(LIST_FIXTURE, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order_by = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "lastChapterAddedAt"
        } else {
            "totalViews"
        };
        Ok(parse_query_response(
            &fetch_api_or_fixture(
                &format!(
                    "/api/query?page={page}&perPage={PER_PAGE}&searchTerm=&orderBy={order_by}&orderDirection=desc"
                ),
                LIST_FIXTURE,
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
            if let Some(slug) = extract_series_slug(query) {
                if let Some(post) = find_post_by_slug(&slug) {
                    return Ok(Paged {
                        entries: vec![post.into_catalog()],
                        has_next_page: false,
                    });
                }
            }
        }
        Ok(parse_query_response(
            &fetch_api_or_fixture(
                &format!(
                    "/api/query?page={page}&perPage={PER_PAGE}&searchTerm={}",
                    url::query_escape(query)
                ),
                LIST_FIXTURE,
            ),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#1".to_string());
        let post_id = resolve_post_id(&key).unwrap_or(1);
        Ok(parse_post_response(&fetch_api_or_fixture(
            &format!("/api/post?postId={post_id}"),
            DETAILS_FIXTURE,
        )))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#1".to_string());
        let post_id = resolve_post_id(&key).unwrap_or(1);
        Ok(parse_chapters(&fetch_api_or_fixture(
            &format!("/api/post?postId={post_id}"),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/chapter-1#10".into());
        let chapter_id = resolve_chapter_id(&key).unwrap_or(10);
        Ok(parse_pages(&fetch_api_or_fixture(
            &format!("/api/chapter?chapterId={chapter_id}"),
            CHAPTER_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL)
            && let Some(slug) = extract_series_slug(input)
            && let Some(post) = find_post_by_slug(&slug)
        {
            return Ok(Some(UrlResolveResult {
                item: Some(post.into_catalog()),
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
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api_or_fixture(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{API_URL}{path}"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_query_response(body: &str, page: u64) -> Paged<CatalogItem> {
    let payload: SearchResponse = serde_json::from_str(body).unwrap_or_default();
    Paged {
        has_next_page: payload.total_count as u64 > page * PER_PAGE,
        entries: payload
            .posts
            .into_iter()
            .filter(|post| !post.post_title.trim().is_empty())
            .map(PostSummary::into_catalog)
            .collect(),
    }
}

fn parse_post_response(body: &str) -> CatalogItem {
    let payload: PostResponse = serde_json::from_str(body).unwrap_or_default();
    payload.post.into_catalog()
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let payload: PostResponse = serde_json::from_str(body).unwrap_or_default();
    let manga_slug = payload.post.slug.clone();
    payload
        .post
        .chapters
        .into_iter()
        .filter(|chapter| chapter.is_accessible != Some(false) && chapter.is_locked != Some(true))
        .map(|chapter| chapter.into_chapter(&manga_slug))
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let payload: ChapterResponse = serde_json::from_str(body).unwrap_or_default();
    let chapter = payload.chapter;
    if chapter.is_accessible == Some(false) || chapter.is_locked == Some(true) {
        return Vec::new();
    }
    let mut images = chapter.images;
    images.sort_by_key(|image| image.order.unwrap_or(i32::MAX));
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.url,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn resolve_post_id(key: &str) -> Option<i64> {
    key.split('#')
        .nth(1)
        .and_then(|value| value.parse().ok())
        .or_else(|| {
            let slug = extract_series_slug(key)?;
            find_post_by_slug(&slug).map(|post| post.id)
        })
}

fn resolve_chapter_id(key: &str) -> Option<i64> {
    key.split('#')
        .nth(1)
        .and_then(|value| value.parse().ok())
        .or_else(|| {
            let (manga_slug, chapter_slug) = extract_chapter_slugs(key)?;
            let post_id = resolve_post_id(&manga_slug)?;
            let payload =
                fetch_api_or_fixture(&format!("/api/post?postId={post_id}"), DETAILS_FIXTURE);
            let post: PostResponse = serde_json::from_str(&payload).ok()?;
            post.post
                .chapters
                .into_iter()
                .find(|chapter| chapter.slug == chapter_slug)
                .map(|chapter| chapter.id)
        })
}

fn find_post_by_slug(slug: &str) -> Option<PostSummary> {
    for term in build_slug_search_terms(slug) {
        let body = fetch_api_or_fixture(
            &format!(
                "/api/query?page=1&perPage={PER_PAGE}&searchTerm={}",
                url::query_escape(&term)
            ),
            LIST_FIXTURE,
        );
        let payload: SearchResponse = serde_json::from_str(&body).ok()?;
        if let Some(post) = payload
            .posts
            .into_iter()
            .find(|post| post.slug.eq_ignore_ascii_case(slug))
        {
            return Some(post);
        }
    }
    None
}

fn extract_series_slug(input: &str) -> Option<String> {
    let path = input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches('/')
        .split('#')
        .next()
        .unwrap_or_default();
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    match parts.as_slice() {
        ["series", slug, ..] => Some((*slug).to_string()),
        [slug] => Some((*slug).to_string()),
        _ => None,
    }
}

fn extract_chapter_slugs(input: &str) -> Option<(String, String)> {
    let path = input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches('/')
        .split('#')
        .next()
        .unwrap_or_default();
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    match parts.as_slice() {
        ["series", manga, chapter, ..] => Some(((*manga).to_string(), (*chapter).to_string())),
        [manga, chapter, ..] => Some(((*manga).to_string(), (*chapter).to_string())),
        _ => None,
    }
}

fn build_slug_search_terms(slug: &str) -> Vec<String> {
    let with_spaces = slug.replace('-', " ");
    let without_apostrophe = with_spaces.replace('\'', " ");
    [
        with_spaces.clone(),
        without_apostrophe.clone(),
        collapse_spaces(&without_apostrophe),
        slug.to_string(),
    ]
    .into_iter()
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty())
    .fold(Vec::new(), |mut values, value| {
        if !values.contains(&value) {
            values.push(value);
        }
        values
    })
}

fn collapse_spaces(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    #[serde(default)]
    posts: Vec<PostSummary>,
    #[serde(default)]
    total_count: i64,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostSummary {
    id: i64,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    post_title: String,
    #[serde(default)]
    featured_image: Option<String>,
    #[serde(default)]
    series_status: Option<String>,
    #[serde(default)]
    genres: Vec<Genre>,
}

impl PostSummary {
    fn into_catalog(self) -> CatalogItem {
        CatalogItem {
            key: format!("{}#{}", self.slug, self.id),
            title: self.post_title.trim().to_string(),
            cover: self.featured_image,
            tags: self.genres.into_iter().map(|genre| genre.name).collect(),
            status: map_status(self.series_status.as_deref()),
            url: Some(format!("{BASE_URL}/series/{}", self.slug)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct Genre {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct PostResponse {
    #[serde(default)]
    post: Post,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Post {
    id: i64,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    post_title: String,
    #[serde(default)]
    post_content: Option<String>,
    #[serde(default)]
    alternative_titles: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    featured_image: Option<String>,
    #[serde(default)]
    series_type: Option<String>,
    #[serde(default)]
    series_status: Option<String>,
    #[serde(default)]
    genres: Vec<Genre>,
    #[serde(default)]
    chapters: Vec<PostChapter>,
}

impl Post {
    fn into_catalog(self) -> CatalogItem {
        let mut tags = Vec::new();
        if let Some(series_type) = self.series_type.as_deref().and_then(map_type) {
            tags.push(series_type.to_string());
        }
        tags.extend(self.genres.into_iter().map(|genre| genre.name));
        CatalogItem {
            key: format!("{}#{}", self.slug, self.id),
            title: self.post_title.trim().to_string(),
            cover: self.featured_image,
            description: build_description(self.post_content, self.alternative_titles),
            authors: self
                .author
                .into_iter()
                .filter(|value| !value.trim().is_empty())
                .collect(),
            artists: self
                .artist
                .into_iter()
                .filter(|value| !value.trim().is_empty())
                .collect(),
            tags,
            status: map_status(self.series_status.as_deref()),
            url: Some(format!("{BASE_URL}/series/{}", self.slug)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostChapter {
    id: i64,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    number: Value,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    is_locked: Option<bool>,
    #[serde(default)]
    is_accessible: Option<bool>,
}

impl PostChapter {
    fn into_chapter(self, manga_slug: &str) -> MangaChapter {
        let number_text = json_number_text(&self.number)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                self.slug
                    .strip_prefix("chapter-")
                    .unwrap_or(&self.slug)
                    .to_string()
            });
        let locked = self.is_accessible == Some(false) || self.is_locked == Some(true);
        let mut title = format!("Chapter {number_text}");
        if let Some(chapter_title) = self.title.filter(|value| !value.trim().is_empty()) {
            title.push_str(" - ");
            title.push_str(chapter_title.trim());
        }
        if locked {
            title = format!("[LOCKED] {title}");
        }
        MangaChapter {
            key: format!("{manga_slug}/{}#{}", self.slug, self.id),
            title: Some(title),
            chapter_number: number_text.parse().ok(),
            date_uploaded: manatan_shared::dates::parse_fixture_date(&self.created_at),
            url: Some(format!("{BASE_URL}/series/{manga_slug}/{}", self.slug)),
            is_locked: locked,
            ..MangaChapter::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct ChapterResponse {
    #[serde(default)]
    chapter: Chapter,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Chapter {
    #[serde(default)]
    is_locked: Option<bool>,
    #[serde(default)]
    is_accessible: Option<bool>,
    #[serde(default)]
    images: Vec<ChapterImage>,
}

#[derive(Default, Deserialize)]
struct ChapterImage {
    url: String,
    #[serde(default)]
    order: Option<i32>,
}

fn json_number_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn build_description(
    content: Option<String>,
    alternative_titles: Option<String>,
) -> Option<String> {
    let synopsis = content
        .map(|value| {
            html::strip_tags(
                &value
                    .replace("<br>", "\n")
                    .replace("<br/>", "\n")
                    .replace("<br />", "\n"),
            )
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let alternatives = alternative_titles
        .map(|value| {
            value
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|value| !value.is_empty());
    match (synopsis, alternatives) {
        (Some(synopsis), Some(alternatives)) => {
            Some(format!("{synopsis}\n\nAlternative titles:\n{alternatives}"))
        }
        (Some(synopsis), None) => Some(synopsis),
        (None, Some(alternatives)) => Some(format!("Alternative titles:\n{alternatives}")),
        (None, None) => None,
    }
}

fn map_type(value: &str) -> Option<&'static str> {
    match value {
        "MANGA" => Some("Manga"),
        "MANHUA" => Some("Manhua"),
        "MANHWA" => Some("Manhwa"),
        _ => None,
    }
}

fn map_status(value: Option<&str>) -> ItemStatus {
    match value {
        Some("ONGOING" | "COMING_SOON" | "MASS_RELEASED") => ItemStatus::Ongoing,
        Some("COMPLETED") => ItemStatus::Completed,
        Some("CANCELLED" | "DROPPED") => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

const LIST_FIXTURE: &str = r#"{"posts":[{"id":1,"slug":"sample","postTitle":"Sample Manga","featuredImage":"https://img.example/cover.jpg","seriesStatus":"ONGOING","genres":[{"name":"Action"}]}],"totalCount":19}"#;
const DETAILS_FIXTURE: &str = r#"{"post":{"id":1,"slug":"sample","postTitle":"Sample Manga","postContent":"Sample<br>description.","alternativeTitles":"Sample Alt","author":"Writer","artist":"Artist","featuredImage":"https://img.example/cover.jpg","seriesType":"MANHWA","seriesStatus":"COMPLETED","genres":[{"name":"Action"}],"chapters":[{"id":10,"slug":"chapter-1","number":1,"title":"Start","createdAt":"2024-01-01T00:00:00.000Z","isLocked":false,"isAccessible":true},{"id":11,"slug":"chapter-2","number":"2","title":"Locked","createdAt":"2024-01-02T00:00:00.000Z","isLocked":true,"isAccessible":false}]}}"#;
const CHAPTER_FIXTURE: &str = r#"{"chapter":{"isLocked":false,"isAccessible":true,"images":[{"url":"https://img.example/page2.jpg","order":2},{"url":"https://img.example/page1.jpg","order":1}]}}"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vortex_api() {
        let list = parse_query_response(LIST_FIXTURE, 1);
        assert_eq!(list.entries[0].key, "sample#1");

        let details = parse_post_response(DETAILS_FIXTURE);
        assert_eq!(details.status, ItemStatus::Completed);
        assert_eq!(details.tags[0], "Manhwa");

        let chapters = parse_chapters(DETAILS_FIXTURE);
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].key, "sample/chapter-1#10");

        let pages = parse_pages(CHAPTER_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
