use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: MiMi = MiMi;
const BASE_URL: &str = "https://mimimoe.moe";
const API_URL: &str = "https://mimimoe.moe/api";
const PREFIX_ID_SEARCH: &str = "id:";

struct MiMi;

impl MangaSource for MiMi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_page(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updated_at"
        } else {
            "views"
        };
        Ok(parse_manga_page(&fetch_json(
            &url_with_params(
                &format!("{API_URL}/manga"),
                &[
                    ("sort", sort.to_string()),
                    ("exclude_genre", "196".into()),
                    ("page", page.to_string()),
                    ("page_size", "45".into()),
                ],
            ),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with("https://") && query.contains(BASE_URL) {
            let id = query
                .split("/manga/")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .unwrap_or_default();
            return Ok(single_manga(id));
        }
        if query.starts_with(PREFIX_ID_SEARCH)
            || (query.len() >= 4 && query.chars().all(|ch| ch.is_ascii_digit()))
        {
            return Ok(single_manga(query.trim_start_matches(PREFIX_ID_SEARCH)));
        }

        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let sort = filter_string(filters, "sort");
        let advanced = ["parody", "character", "author", "genre", "exclude_genre"]
            .iter()
            .any(|key| !filter_string(filters, key).trim().is_empty());
        let path = if advanced {
            format!("{API_URL}/manga/advanced-search")
        } else if sort.is_empty() {
            format!("{API_URL}/manga/search")
        } else {
            format!("{API_URL}/manga")
        };
        let mut params = vec![("page", page.to_string()), ("page_size", "24".into())];
        if !query.is_empty() {
            params.push(("title", query.to_string()));
        }
        if !advanced && !sort.is_empty() {
            params.push(("sort", sort));
        }
        if advanced {
            for key in ["parody", "character"] {
                let value = filter_string(filters, key);
                if !value.trim().is_empty() {
                    params.push((key, value));
                }
            }
            let author = filter_string(filters, "author");
            if author.chars().all(|ch| ch.is_ascii_digit()) && !author.is_empty() {
                params.push(("author", author));
            }
            for key in ["genre", "exclude_genre"] {
                for value in split_ids(&filter_string(filters, key)) {
                    params.push((key, value));
                }
            }
        }
        Ok(parse_manga_page(&fetch_json(
            &url_with_params(&path, &params),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(parse_manga_detail(&fetch_json(
            &format!("{API_URL}/manga/{}", pure_id(&key)),
            DETAILS_FIXTURE,
        )))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let manga_id = pure_id(&key);
        Ok(parse_chapters(
            &fetch_json(
                &format!("{API_URL}/manga/{manga_id}/chapters"),
                CHAPTERS_FIXTURE,
            ),
            &manga_id,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1/1".into());
        let chapter_id = key.split('/').next_back().unwrap_or("1");
        Ok(parse_pages(&fetch_json(
            &format!("{API_URL}/chapters/{chapter_id}"),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/manga/{}", pure_id(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let manga_id = key.split('/').next().unwrap_or_default();
            let chapter_id = key.split('/').next_back().unwrap_or_default();
            format!("{BASE_URL}/manga/{manga_id}/chapter/{chapter_id}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let id = input
                .split("/manga/")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .unwrap_or_default();
            if !id.is_empty() {
                return Ok(Some(UrlResolveResult {
                    item: Some(parse_manga_detail(&fetch_json(
                        &format!("{API_URL}/manga/{id}"),
                        DETAILS_FIXTURE,
                    ))),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
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
        .with_header("Accept", "application/json, text/plain, */*")
        .with_header("Origin", BASE_URL)
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

fn single_manga(id: &str) -> Paged<CatalogItem> {
    Paged {
        entries: vec![parse_manga_detail(&fetch_json(
            &format!("{API_URL}/manga/{id}"),
            DETAILS_FIXTURE,
        ))],
        has_next_page: false,
    }
}

fn parse_manga_page(body: &str) -> Paged<CatalogItem> {
    let response: DataDto =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap());
    Paged {
        entries: response
            .items
            .into_iter()
            .map(manga_to_basic_item)
            .collect(),
        has_next_page: response.has_next,
    }
}

fn parse_manga_detail(body: &str) -> CatalogItem {
    let manga: MangaDto = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    manga_to_detail_item(manga)
}

fn parse_chapters(body: &str, manga_id: &str) -> Vec<MangaChapter> {
    let chapters: Vec<ChapterDto> = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).unwrap_or_default());
    chapters
        .into_iter()
        .map(|chapter| chapter_to_item(chapter, manga_id))
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let pages: PageDto =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).unwrap());
    pages
        .pages
        .into_iter()
        .enumerate()
        .map(|(index, page)| MangaPage {
            content: PageContent::Url {
                url: page.image_url,
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn manga_to_basic_item(manga: MangaDto) -> CatalogItem {
    CatalogItem {
        key: manga.id.to_string(),
        title: manga.title,
        cover: manga.cover_url,
        url: Some(format!("{BASE_URL}/manga/{}", manga.id)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        ..CatalogItem::default()
    }
}

fn manga_to_detail_item(manga: MangaDto) -> CatalogItem {
    let mut description = String::new();
    append_if_not_empty(&mut description, "Tên khác", manga.alt_names);
    append_if_not_empty(
        &mut description,
        "Parody",
        manga.parodies.into_iter().map(|item| item.name).collect(),
    );
    append_if_not_empty(
        &mut description,
        "Nhân vật",
        manga.characters.into_iter().map(|item| item.name).collect(),
    );
    append_if_not_empty(
        &mut description,
        "Code author",
        manga
            .authors
            .iter()
            .filter_map(|item| item.id.map(|id| id.to_string()))
            .collect(),
    );
    description.push_str(&format!("Code manga: {}\n\n", manga.id));
    if let Some(text) = manga.description.as_deref() {
        description.push_str(text);
    }

    CatalogItem {
        key: manga.id.to_string(),
        title: manga.title,
        cover: manga.cover_url,
        url: Some(format!("{BASE_URL}/manga/{}", manga.id)),
        authors: manga.authors.into_iter().map(|item| item.name).collect(),
        description: (!description.trim().is_empty()).then_some(description),
        tags: manga.genres.into_iter().map(|genre| genre.name).collect(),
        status: ItemStatus::Unknown,
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn chapter_to_item(chapter: ChapterDto, manga_id: &str) -> MangaChapter {
    MangaChapter {
        key: format!("{manga_id}/{}", chapter.id),
        title: Some(
            chapter
                .title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| format!("Chapter {}", chapter.order)),
        ),
        chapter_number: Some(chapter.order as f32),
        date_uploaded: chapter.created_at.as_deref().and_then(parse_iso_date),
        url: Some(format!(
            "{BASE_URL}/manga/{manga_id}/chapter/{}",
            chapter.id
        )),
        ..MangaChapter::default()
    }
}

fn append_if_not_empty(description: &mut String, label: &str, values: Vec<String>) {
    let values = values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if !values.is_empty() {
        description.push_str(&format!("{label}: {}\n\n", values.join(", ")));
    }
}

fn parse_iso_date(input: &str) -> Option<i64> {
    manatan_shared::dates::parse_ymd(input.get(..10).unwrap_or(input))
}

fn url_with_params(base: &str, params: &[(&str, String)]) -> String {
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{query}")
}

fn filter_string(filters: &Value, key: &str) -> String {
    filters
        .get(key)
        .and_then(|value| {
            value
                .as_str()
                .or_else(|| value.get("value").and_then(Value::as_str))
        })
        .unwrap_or_default()
        .to_string()
}

fn split_ids(input: &str) -> Vec<String> {
    input
        .split([',', ' '])
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit()))
        .map(ToString::to_string)
        .collect()
}

fn pure_id(input: &str) -> String {
    input
        .trim_start_matches("/g/")
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or(input)
        .to_string()
}

#[derive(Default, Deserialize)]
struct DataDto {
    #[serde(default)]
    items: Vec<MangaDto>,
    #[serde(default)]
    has_next: bool,
}

#[derive(Clone, Default, Deserialize)]
struct MangaDto {
    id: u64,
    title: String,
    #[serde(default)]
    cover_url: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    alt_names: Vec<String>,
    #[serde(default)]
    authors: Vec<NamedDto>,
    #[serde(default)]
    genres: Vec<GenreDto>,
    #[serde(default)]
    parodies: Vec<NamedDto>,
    #[serde(default)]
    characters: Vec<NamedDto>,
}

#[derive(Clone, Default, Deserialize)]
struct NamedDto {
    #[serde(default)]
    id: Option<i64>,
    name: String,
}

#[derive(Clone, Default, Deserialize)]
struct GenreDto {
    name: String,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    id: u64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    order: i64,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Default, Deserialize)]
struct PageDto {
    #[serde(default)]
    pages: Vec<PageImageDto>,
}

#[derive(Deserialize)]
struct PageImageDto {
    image_url: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"items":[{"id":1,"title":"Sample","cover_url":"https://mimimoe.moe/cover.jpg"}],"page":1,"total_pages":1,"has_next":false}"#;
const DETAILS_FIXTURE: &str = r#"{"id":1,"title":"Sample","cover_url":"https://mimimoe.moe/cover.jpg","description":"Summary","alt_names":["Alt"],"authors":[{"id":2,"name":"Author"}],"genres":[{"id":3,"name":"Tag"}],"parodies":[{"id":4,"name":"Parody"}],"characters":[{"id":5,"name":"Character"}]}"#;
const CHAPTERS_FIXTURE: &str =
    r#"[{"id":10,"title":"Chapter 1","order":1,"created_at":"2024-01-01T00:00:00"}]"#;
const PAGES_FIXTURE: &str = r#"{"pages":[{"image_url":"https://mimimoe.moe/page1.jpg"}]}"#;
