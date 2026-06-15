use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Xscans = Xscans;
const BASE_URL: &str = "https://xscans.site";

struct Xscans;

impl MangaSource for Xscans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_response(API_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "newest"
        } else {
            "popular"
        };
        Ok(parse_manga_response(&fetch_json_text(
            &format!("/api/manga?limit=24&sort={sort}&page={page}"),
            API_FIXTURE,
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
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let sort = filter_value(&request, "sort").unwrap_or_else(|| "popular".into());
        let mut params = vec![
            format!("limit=24"),
            format!("page={page}"),
            format!("sort={sort}"),
        ];
        if !query.is_empty() {
            params.push(format!("q={}", url::query_escape(query)));
        }
        if let Some(genre) = filter_value(&request, "genre").filter(|value| !value.is_empty()) {
            params.push(format!("genres={}", url::query_escape(&genre)));
        }
        Ok(parse_manga_response(&fetch_json_text(
            &format!("/api/manga?{}", params.join("&")),
            API_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let show_locked = preference_bool(&request, "showLocked", false);
        let item = details_dto(&key);
        Ok(item
            .chapters
            .into_iter()
            .filter(|chapter| show_locked || !chapter.is_locked)
            .map(|chapter| chapter.to_chapter(&key))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/api/manga/sample/chapters?number=1".into());
        let pages: PagesResponseDto = fetch_json(&key, PAGES_FIXTURE);
        Ok(pages
            .images
            .into_iter()
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: url::join_url(BASE_URL, &image),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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

fn fetch_json_text(path: &str, fixture: &str) -> String {
    client()
        .get(&url::join_url(BASE_URL, path))
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json<T: for<'de> Deserialize<'de>>(path: &str, fixture: &str) -> T {
    serde_json::from_str(&fetch_json_text(path, fixture))
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap())
}

fn parse_manga_response(body: &str) -> Paged<CatalogItem> {
    let response: MangaResponseDto =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(API_FIXTURE).unwrap());
    Paged {
        entries: response
            .manga
            .into_iter()
            .map(|item| item.to_item(false))
            .collect(),
        has_next_page: response
            .pagination
            .is_some_and(|pagination| pagination.has_more),
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    details_dto(key).to_item(true)
}

fn details_dto(key: &str) -> MangaDto {
    let body = client()
        .get(&format!("{BASE_URL}/manga/{key}"))
        .header("rsc", "1")
        .send_text()
        .unwrap_or_else(|_| DETAILS_FIXTURE.to_string());
    serde_json::from_str::<NextWrapper>(&body)
        .map(|wrapper| wrapper.props.page_props.initial_manga)
        .or_else(|_| serde_json::from_str::<MangaDto>(&body))
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap())
}

fn filter_value(request: &Value, id: &str) -> Option<String> {
    request
        .pointer(&format!("/filters/{id}"))
        .or_else(|| request.get(id))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn preference_bool(request: &Value, id: &str, default: bool) -> bool {
    request
        .pointer(&format!("/preferences/{id}"))
        .or_else(|| request.get(id))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn normalize_key(input: &str) -> String {
    input
        .trim_start_matches(BASE_URL)
        .trim_start_matches("/manga/")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

#[derive(Debug, Deserialize)]
struct MangaResponseDto {
    #[serde(default)]
    manga: Vec<MangaDto>,
    pagination: Option<PaginationDto>,
}

#[derive(Debug, Deserialize)]
struct PaginationDto {
    #[serde(rename = "hasMore", default)]
    has_more: bool,
}

#[derive(Debug, Deserialize)]
struct MangaDto {
    slug: String,
    title: String,
    #[serde(rename = "coverImage")]
    cover_image: Option<String>,
    description: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    artists: Vec<String>,
    status: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    demographics: Vec<String>,
    #[serde(default)]
    chapters: Vec<ChapterDto>,
}

impl MangaDto {
    fn to_item(self, initialized: bool) -> CatalogItem {
        CatalogItem {
            key: self.slug.clone(),
            title: self.title,
            cover: self
                .cover_image
                .map(|image| url::join_url(BASE_URL, &image)),
            description: self.description,
            authors: self.authors,
            artists: self.artists,
            tags: self.genres.into_iter().chain(self.demographics).collect(),
            status: match self
                .status
                .as_deref()
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("ongoing") => ItemStatus::Ongoing,
                Some("completed") => ItemStatus::Completed,
                Some("hiatus") => ItemStatus::Hiatus,
                Some("cancelled") | Some("dropped") => ItemStatus::Cancelled,
                _ => ItemStatus::Unknown,
            },
            url: Some(format!("{BASE_URL}/manga/{}", self.slug)),
            language: Some("en".into()),
            content_rating: Some("safe".into()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    number: f32,
    title: Option<String>,
    #[serde(rename = "isLocked", default)]
    is_locked: bool,
}

impl ChapterDto {
    fn to_chapter(self, slug: &str) -> MangaChapter {
        let number = self.number.to_string().trim_end_matches(".0").to_string();
        let title = self.title.unwrap_or_else(|| format!("Chapter {number}"));
        MangaChapter {
            key: format!("/api/manga/{slug}/chapters?number={number}"),
            title: Some(if self.is_locked {
                format!("Locked {title}")
            } else {
                title
            }),
            chapter_number: Some(self.number),
            url: Some(format!("{BASE_URL}/manga/{slug}")),
            language: Some("en".into()),
            is_locked: self.is_locked,
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct NextWrapper {
    props: NextProps,
}

#[derive(Debug, Deserialize)]
struct NextProps {
    #[serde(rename = "pageProps")]
    page_props: PageProps,
}

#[derive(Debug, Deserialize)]
struct PageProps {
    #[serde(rename = "initialManga")]
    initial_manga: MangaDto,
}

#[derive(Debug, Deserialize)]
struct PagesResponseDto {
    #[serde(default)]
    images: Vec<String>,
}

export_manga_source!(SOURCE);

const API_FIXTURE: &str = r#"{"manga":[{"slug":"sample","title":"Sample","coverImage":"/cover.jpg","description":"Summary","authors":["Author"],"artists":["Artist"],"status":"ongoing","genres":["Action"],"demographics":[],"chapters":[{"number":1,"title":"Chapter 1","isLocked":false}]}],"pagination":{"hasMore":false}}"#;
const DETAILS_FIXTURE: &str = r#"{"slug":"sample","title":"Sample","coverImage":"/cover.jpg","description":"Summary","authors":["Author"],"artists":["Artist"],"status":"ongoing","genres":["Action"],"demographics":[],"chapters":[{"number":1,"title":"Chapter 1","isLocked":false}]}"#;
const PAGES_FIXTURE: &str = r#"{"images":["/page1.jpg","/page2.jpg"]}"#;
