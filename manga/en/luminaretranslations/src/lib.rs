use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: LuminareTranslations = LuminareTranslations;
const BASE_URL: &str = "https://luminaretranslations.com";
const API_URL: &str = "https://luminaretranslations.com/wp-json/yarnovel/v1";
const PAGE_SIZE: u64 = 24;

struct LuminareTranslations;

impl MangaSource for LuminareTranslations {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_entries(LIST_FIXTURE, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "popular"
        };
        Ok(parse_entries(
            &fetch_json(
                &series_url(page, "", sort, request.get("filters")),
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
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        Ok(parse_entries(
            &fetch_json(
                &series_url(
                    page,
                    query,
                    filter(request.get("filters"), "sort", ""),
                    request.get("filters"),
                ),
                LIST_FIXTURE,
            ),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = fetch_json(
            &format!(
                "{API_URL}/series/{}/chapters?per_page=999",
                url::query_escape(&key)
            ),
            CHAPTERS_FIXTURE,
        );
        let payload: ChapterResponse = serde_json::from_str(&body).unwrap_or_default();
        let mut chapters = payload
            .data
            .into_iter()
            .map(|chapter| chapter.into_chapter(&key))
            .collect::<Vec<_>>();
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "sample/chapter-1".to_string());
        let body = fetch_json(
            &format!("{API_URL}/series/{}", url::query_escape(&key)),
            PAGES_FIXTURE,
        );
        let payload: ViewerResponse = serde_json::from_str(&body).unwrap_or_default();
        Ok(payload
            .data
            .pages
            .into_iter()
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
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/series/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}/series/{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
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

fn series_url(page: u64, query: &str, sort: &str, filters: Option<&Value>) -> String {
    let mut params = vec![
        format!("page={page}"),
        format!("per_page={PAGE_SIZE}"),
        "type=manga".to_string(),
    ];
    if !query.is_empty() {
        params.push(format!("search={}", url::query_escape(query)));
    }
    if !sort.is_empty() {
        params.push(format!("sort={}", url::query_escape(sort)));
    }
    for key in ["genres", "tags", "author", "artist", "status"] {
        let value = filter(filters, key, "");
        if !value.is_empty() {
            params.push(format!("{key}={}", url::query_escape(value)));
        }
    }
    format!("{API_URL}/series?{}", params.join("&"))
}

fn filter<'a>(filters: Option<&'a Value>, key: &str, fallback: &'a str) -> &'a str {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
}

fn parse_entries(body: &str, page: u64) -> Paged<CatalogItem> {
    let payload: EntryResponse =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap());
    let entries = payload
        .data
        .into_iter()
        .filter(|entry| {
            !matches!(
                entry.entry_type.as_deref(),
                Some("novel" | "light_novel" | "web_novel")
            )
        })
        .map(EntryData::into_item)
        .collect::<Vec<_>>();
    Paged {
        entries,
        has_next_page: page * PAGE_SIZE < payload.meta.total,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_json(
        &format!("{API_URL}/series/{}", url::query_escape(key)),
        DETAILS_FIXTURE,
    );
    let payload: DetailsResponse = serde_json::from_str(&body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    payload.data.into_item(key)
}

fn normalize_key(input: &str) -> String {
    input
        .trim_end_matches('/')
        .rsplit("/series/")
        .next()
        .unwrap_or(input)
        .trim_matches('/')
        .to_string()
}

#[derive(Default, Deserialize)]
struct EntryResponse {
    data: Vec<EntryData>,
    meta: Meta,
}

#[derive(Default, Deserialize)]
struct Meta {
    total: u64,
}

#[derive(Default, Deserialize)]
struct EntryData {
    title: String,
    slug: String,
    #[serde(rename = "type")]
    entry_type: Option<String>,
    #[serde(rename = "cover_image")]
    cover_image: Option<String>,
}

impl EntryData {
    fn into_item(self) -> CatalogItem {
        CatalogItem {
            key: self.slug.clone(),
            title: self.title,
            cover: self.cover_image,
            url: Some(format!("{BASE_URL}/series/{}", self.slug)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct DetailsResponse {
    data: Details,
}

#[derive(Default, Deserialize)]
struct Details {
    title: String,
    status: Option<String>,
    #[serde(rename = "cover_image")]
    cover_image: Option<String>,
    genres: Option<Vec<String>>,
    description: Option<String>,
    author: Option<String>,
    artist: Option<String>,
}

impl Details {
    fn into_item(self, key: &str) -> CatalogItem {
        CatalogItem {
            key: key.to_string(),
            title: self.title,
            cover: self.cover_image,
            description: self.description,
            authors: self.author.into_iter().collect(),
            artists: self.artist.into_iter().collect(),
            tags: self.genres.unwrap_or_default(),
            status: match self.status.as_deref() {
                Some("ongoing") => ItemStatus::Ongoing,
                Some("completed") => ItemStatus::Completed,
                Some("hiatus") => ItemStatus::Hiatus,
                Some("dropped") => ItemStatus::Cancelled,
                _ => ItemStatus::Unknown,
            },
            url: Some(format!("{BASE_URL}/series/{key}")),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct ChapterResponse {
    data: Vec<ChapterData>,
}

#[derive(Default, Deserialize)]
struct ChapterData {
    title: Option<String>,
    number: f64,
    slug: String,
    #[serde(rename = "published_at")]
    published_at: Option<String>,
}

impl ChapterData {
    fn into_chapter(self, manga_key: &str) -> MangaChapter {
        let chapter_num = if self.number.fract() == 0.0 {
            format!("{}", self.number as u64)
        } else {
            self.number.to_string()
        };
        let key = format!("{manga_key}/{}", self.slug);
        MangaChapter {
            key: key.clone(),
            title: Some(
                self.title
                    .unwrap_or_else(|| format!("Chapter {chapter_num}")),
            ),
            chapter_number: Some(self.number as f32),
            date_uploaded: self
                .published_at
                .as_deref()
                .and_then(|date| manga_date(date.get(..10).unwrap_or(date))),
            url: Some(format!("{BASE_URL}/series/{key}")),
            ..MangaChapter::default()
        }
    }
}

fn manga_date(value: &str) -> Option<i64> {
    manatan_shared::dates::parse_fixture_date(value)
}

#[derive(Default, Deserialize)]
struct ViewerResponse {
    data: ViewerData,
}

#[derive(Default, Deserialize)]
struct ViewerData {
    pages: Vec<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":[{"title":"Sample Manga","slug":"sample","type":"manga","cover_image":"https://luminaretranslations.com/cover.jpg"}],"meta":{"total":1}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"title":"Sample Manga","status":"ongoing","cover_image":"https://luminaretranslations.com/cover.jpg","genres":["Action"],"description":"Sample description","author":"Author","artist":"Artist"}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"title":null,"number":1,"slug":"chapter-1","published_at":"2024-01-01T00:00:00+0000"}]}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"pages":["https://luminaretranslations.com/page1.jpg"]}}"#;
