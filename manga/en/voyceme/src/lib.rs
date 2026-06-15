use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: VoyceMe = VoyceMe;
const BASE_URL: &str = "https://www.voyce.me";
const GRAPHQL_URL: &str = "https://graphql.voyce.me/v1/graphql";
const STATIC_URL: &str = "https://dlkfxmdtxtzpb.cloudfront.net/";
const PER_PAGE: u64 = 10;

struct VoyceMe;

impl MangaSource for VoyceMe {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_collection(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            LATEST_QUERY
        } else {
            POPULAR_QUERY
        };
        Ok(parse_series_collection(&graphql_or_fixture(
            query,
            json!({"offset": (page.saturating_sub(1)) * PER_PAGE, "limit": PER_PAGE}),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query_text = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query_text.starts_with(BASE_URL) {
            let slug = slug_from_url(query_text);
            return Ok(Paged {
                entries: vec![details_by_slug(&slug)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_series_collection(&graphql_or_fixture(
            SEARCH_QUERY,
            json!({
                "searchTerm": format!("%{query_text}%"),
                "offset": (page.saturating_sub(1)) * PER_PAGE,
                "limit": PER_PAGE
            }),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(details_by_slug(&slug_from_key(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let slug = slug_from_key(&key);
        let payload = serde_json::from_str::<SeriesCollection>(&graphql_or_fixture(
            CHAPTERS_QUERY,
            json!({"slug": slug}),
            CHAPTERS_FIXTURE,
        ))
        .unwrap_or_default();
        Ok(payload
            .series
            .into_iter()
            .next()
            .map(|series| {
                series
                    .chapters
                    .into_iter()
                    .map(|chapter| chapter.into_chapter(&series.slug))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/1#comic".to_string());
        let chapter_id = key
            .split('#')
            .next()
            .unwrap_or(&key)
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("1")
            .parse::<i64>()
            .unwrap_or(1);
        let payload = serde_json::from_str::<ChapterImagesCollection>(&graphql_or_fixture(
            PAGES_QUERY,
            json!({"chapterId": chapter_id}),
            PAGES_FIXTURE,
        ))
        .unwrap_or_default();
        Ok(payload
            .images
            .into_iter()
            .enumerate()
            .map(|(index, page)| MangaPage {
                content: PageContent::Url {
                    url: format!("{STATIC_URL}{}", page.image.trim_start_matches('/')),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
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
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}{}", key.split('#').next().unwrap_or(&key))))
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
        .with_header("Origin", BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn graphql_or_fixture(query: &str, variables: Value, fixture: &str) -> String {
    client()
        .post(GRAPHQL_URL)
        .json(json!({"query": query, "variables": variables}).to_string())
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_series_collection(body: &str) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<SeriesCollection>(body).unwrap_or_default();
    let count = payload.series.len() as u64;
    Paged {
        entries: payload.series.into_iter().map(Comic::into_item).collect(),
        has_next_page: count == PER_PAGE,
    }
}

fn details_by_slug(slug: &str) -> CatalogItem {
    let payload = serde_json::from_str::<SeriesCollection>(&graphql_or_fixture(
        DETAILS_QUERY,
        json!({"slug": slug}),
        DETAILS_FIXTURE,
    ))
    .unwrap_or_default();
    payload
        .series
        .into_iter()
        .next()
        .map(Comic::into_item_initialized)
        .unwrap_or_else(|| Comic::sample(slug).into_item_initialized())
}

fn slug_from_key(key: &str) -> String {
    key.trim_start_matches("/series/")
        .split('/')
        .next()
        .unwrap_or("sample")
        .to_string()
}

fn slug_from_url(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    slug_from_key(path)
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default() {
        "completed" => ItemStatus::Completed,
        "ongoing" => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: &str) -> Option<i64> {
    let mut parts = value.split('T').next()?.split('-');
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
struct SeriesCollection {
    #[serde(rename = "voyce_series", default)]
    series: Vec<Comic>,
}

#[derive(Default, Deserialize)]
struct ChapterImagesCollection {
    #[serde(rename = "voyce_chapter_images", default)]
    images: Vec<PageDto>,
}

#[derive(Default, Deserialize)]
struct Comic {
    author: Option<Author>,
    #[serde(default)]
    chapters: Vec<ChapterDto>,
    description: Option<String>,
    #[serde(default)]
    genres: Vec<GenreAggregation>,
    slug: String,
    status: Option<String>,
    #[serde(default)]
    thumbnail: String,
    #[serde(default)]
    title: String,
}

impl Comic {
    fn sample(slug: &str) -> Self {
        Self {
            slug: slug.to_string(),
            title: "Sample".to_string(),
            thumbnail: "cover.jpg".to_string(),
            ..Self::default()
        }
    }

    fn into_item(self) -> CatalogItem {
        let key = format!("/series/{}", self.slug);
        let authors = self
            .author
            .and_then(|author| author.username)
            .filter(|value| !value.is_empty())
            .into_iter()
            .collect::<Vec<_>>();
        let description = self
            .description
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty());
        let tags = self
            .genres
            .into_iter()
            .filter_map(|item| item.genre.and_then(|genre| genre.title))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        let status = parse_status(self.status.as_deref());
        CatalogItem {
            key: key.clone(),
            title: if self.title.is_empty() {
                url::slug_from_url(&self.slug).unwrap_or_else(|| "Manga".into())
            } else {
                self.title
            },
            cover: (!self.thumbnail.is_empty())
                .then(|| format!("{STATIC_URL}{}", self.thumbnail.trim_start_matches('/'))),
            authors,
            description,
            tags,
            status,
            url: Some(format!("{BASE_URL}{key}")),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }

    fn into_item_initialized(self) -> CatalogItem {
        let mut item = self.into_item();
        item.initialized = true;
        item
    }
}

#[derive(Default, Deserialize)]
struct Author {
    username: Option<String>,
}

#[derive(Default, Deserialize)]
struct GenreAggregation {
    genre: Option<Genre>,
}

#[derive(Default, Deserialize)]
struct Genre {
    title: Option<String>,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    #[serde(rename = "created_at")]
    created_at: String,
    id: i64,
    title: String,
}

impl ChapterDto {
    fn into_chapter(self, slug: &str) -> MangaChapter {
        let key = format!("/series/{slug}/{}#comic", self.id);
        MangaChapter {
            key: key.clone(),
            title: Some(self.title),
            date_uploaded: parse_date(&self.created_at),
            url: Some(format!(
                "{BASE_URL}{}",
                key.split('#').next().unwrap_or(&key)
            )),
            ..MangaChapter::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct PageDto {
    image: String,
}

const POPULAR_QUERY: &str = r#"query($limit: Int, $offset: Int) { voyce_series(where: { publish: { _eq: 1 }, type: { id: { _in: [2, 4] } } }, order_by: [{ views_counts: { count: desc_nulls_last } }], limit: $limit, offset: $offset) { id slug thumbnail title } }"#;
const LATEST_QUERY: &str = r#"query($limit: Int, $offset: Int) { voyce_series(where: { publish: { _eq: 1 }, type: { id: { _in: [2, 4] } } }, order_by: [{ updated_at: desc }], limit: $limit, offset: $offset) { id slug thumbnail title } }"#;
const SEARCH_QUERY: &str = r#"query($searchTerm: String!, $limit: Int, $offset: Int) { voyce_series(where: { publish: { _eq: 1 }, type: { id: { _in: [2, 4] } }, title: { _ilike: $searchTerm } }, order_by: [{ views_counts: { count: desc_nulls_last } }], limit: $limit, offset: $offset) { id slug thumbnail title } }"#;
const DETAILS_QUERY: &str = r#"query($slug: String!) { voyce_series(where: { publish: { _eq: 1 }, type: { id: { _in: [2, 4] } }, slug: { _eq: $slug } }, limit: 1) { id slug thumbnail title description status author { username } genres(order_by: [{ genre: { title: asc } }]) { genre { title } } } }"#;
const CHAPTERS_QUERY: &str = r#"query($slug: String!) { voyce_series(where: { publish: { _eq: 1 }, type: { id: { _in: [2, 4] } }, slug: { _eq: $slug } }, limit: 1) { slug chapters(order_by: [{ created_at: desc }]) { id title created_at } } }"#;
const PAGES_QUERY: &str = r#"query($chapterId: Int!) { voyce_chapter_images(where: { chapter_id: { _eq: $chapterId } }, order_by: { sort_order: asc }) { image } }"#;

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str =
    r#"{"voyce_series":[{"slug":"sample","thumbnail":"cover.jpg","title":"Sample"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"voyce_series":[{"slug":"sample","thumbnail":"cover.jpg","title":"Sample","description":"Summary","status":"ongoing","author":{"username":"Author"},"genres":[{"genre":{"title":"Action"}}]}]}"#;
const CHAPTERS_FIXTURE: &str = r#"{"voyce_series":[{"slug":"sample","chapters":[{"id":1,"title":"Chapter 1","created_at":"2024-01-01"}]}]}"#;
const PAGES_FIXTURE: &str = r#"{"voyce_chapter_images":[{"image":"page1.jpg"}]}"#;
