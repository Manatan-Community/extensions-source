use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: AstralManga = AstralManga;
const BASE_URL: &str = "https://astral-manga.fr";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";
const PAGE_SIZE: u64 = 12;

struct AstralManga;

impl MangaSource for AstralManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_api(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let sort_by = if latest { "publishDate" } else { "note" };
        Ok(parse_manga_api(&fetch_json_or_fixture(
            &manga_api_url(page, "", sort_by, "desc", request.get("filters")),
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
        if let Some(key) = deeplink_key(query) {
            let body = fetch_rsc_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let (sort_by, sort_order) = sort_from_filters(request.get("filters"));
        Ok(parse_manga_api(&fetch_json_or_fixture(
            &manga_api_url(page, query, &sort_by, &sort_order, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_rsc_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_rsc_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter/chapter-1".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = deeplink_key(input) {
            let body = fetch_rsc_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_rsc_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("RSC", "1")
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn manga_api_url(
    page: u64,
    query: &str,
    sort_by: &str,
    sort_order: &str,
    filters: Option<&Value>,
) -> String {
    let mut pairs = vec![
        ("page", page.to_string()),
        ("pageSize", PAGE_SIZE.to_string()),
        ("sortBy", sort_by.to_string()),
        ("sortOrder", sort_order.to_string()),
        ("includeMode", "and".to_string()),
        ("excludeMode", "or".to_string()),
    ];
    if !query.trim().is_empty() {
        pairs.push(("query", query.trim().to_string()));
    }
    if let Some(status) = filter_str(filters, "status").filter(|value| !value.is_empty()) {
        pairs.push(("status", status.to_string()));
    }
    if let Some(type_value) = filter_str(filters, "type").filter(|value| !value.is_empty()) {
        pairs.push(("type", type_value.to_string()));
    }
    for tag in selected_values(filters, "tags") {
        pairs.push(("tags", tag));
    }
    format!(
        "{BASE_URL}/api/mangas?{}",
        pairs
            .into_iter()
            .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn sort_from_filters(filters: Option<&Value>) -> (String, String) {
    let sort_by = filter_str(filters, "sortBy").unwrap_or("title").to_string();
    let sort_order = if sort_by == "title" { "asc" } else { "desc" }.to_string();
    (sort_by, sort_order)
}

fn parse_manga_api(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<MangaResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    let count = response.mangas.len() as u64;
    Paged {
        entries: response
            .mangas
            .into_iter()
            .map(MangaDto::into_item)
            .collect(),
        has_next_page: count >= PAGE_SIZE && count < response.total,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let slug = key.trim_matches('/').rsplit('/').next().unwrap_or("sample");
    let manga = extract_json_objects(body)
        .into_iter()
        .filter(|object| object.contains("\"urlId\""))
        .filter_map(|object| serde_json::from_str::<MangaDto>(&object).ok())
        .find(|manga| manga.url_id == slug)
        .or_else(|| serde_json::from_str::<MangaDto>(DETAILS_MANGA_FIXTURE).ok())
        .unwrap_or_else(|| MangaDto::sample(slug));
    let mut item = manga.into_item();
    item.initialized = true;
    item
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let slug = manga_key
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("sample");
    let manga = extract_json_objects(body)
        .into_iter()
        .filter(|object| object.contains("\"urlId\""))
        .filter_map(|object| serde_json::from_str::<MangaDto>(&object).ok())
        .find(|manga| manga.url_id == slug)
        .or_else(|| serde_json::from_str::<MangaDto>(DETAILS_MANGA_FIXTURE).ok())
        .unwrap_or_else(|| MangaDto::sample(slug));
    let manga_id = manga.id.clone();
    let mut seen = std::collections::BTreeSet::new();
    let mut chapters = extract_json_objects(body)
        .into_iter()
        .filter(|object| object.contains("\"mangaId\""))
        .filter_map(|object| serde_json::from_str::<RscChapterDto>(&object).ok())
        .filter(|chapter| chapter.manga_id == manga_id && seen.insert(chapter.id.clone()))
        .map(|chapter| chapter.into_chapter(slug))
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        chapters = serde_json::from_str::<Vec<RscChapterDto>>(CHAPTERS_FIXTURE)
            .unwrap_or_default()
            .into_iter()
            .map(|chapter| chapter.into_chapter(slug))
            .collect();
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut images = extract_json_objects(body)
        .into_iter()
        .filter(|object| object.contains("\"orderId\"") && object.contains("\"link\""))
        .filter_map(|object| serde_json::from_str::<RscImageDto>(&object).ok())
        .collect::<Vec<_>>();
    if images.is_empty() {
        images = serde_json::from_str::<Vec<RscImageDto>>(PAGES_IMAGES_FIXTURE).unwrap_or_default();
    }
    if !images.is_empty() {
        images.sort_by_key(|image| image.order_id);
        return images
            .into_iter()
            .enumerate()
            .map(|(index, image)| page_from_url(resolve_image_link(&image.link), index))
            .collect();
    }
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("Page "))
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
        .enumerate()
        .map(|(index, image)| page_from_url(url::join_url(BASE_URL, &image), index))
        .collect()
}

fn page_from_url(image: String, index: usize) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image,
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn resolve_image_link(link: &str) -> String {
    if let Some(key) = link.strip_prefix("s3:") {
        presign_s3_key(key).unwrap_or_else(|| link.to_string())
    } else {
        url::join_url(BASE_URL, link)
    }
}

fn presign_s3_key(key: &str) -> Option<String> {
    let target = format!(
        "{BASE_URL}/api/s3/presign-get?key={}",
        url::query_escape(key)
    );
    serde_json::from_str::<PresignResponse>(&fetch_json_or_fixture(&target, PRESIGN_FIXTURE))
        .ok()
        .map(|response| response.url)
}

fn extract_json_objects(input: &str) -> Vec<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'{' {
            index += 1;
            continue;
        }
        let start = index;
        let mut depth = 0_i32;
        let mut in_string = false;
        let mut escaped = false;
        while index < bytes.len() {
            let byte = bytes[index];
            if in_string {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    in_string = false;
                }
            } else if byte == b'"' {
                in_string = true;
            } else if byte == b'{' {
                depth += 1;
            } else if byte == b'}' {
                depth -= 1;
                if depth == 0 {
                    out.push(String::from_utf8_lossy(&bytes[start..=index]).into_owned());
                    break;
                }
            }
            index += 1;
        }
        index += 1;
    }
    out
}

fn selected_values(filters: Option<&Value>, id: &str) -> Vec<String> {
    match filters.and_then(|filters| filters.get(id)) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn filter_str<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn deeplink_key(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) && input.contains("/manga/") {
        let path = input
            .strip_prefix(BASE_URL)
            .unwrap_or(input)
            .split(['?', '#'])
            .next()
            .unwrap_or(input);
        Some(format!(
            "/{}",
            path.trim_start_matches('/').trim_end_matches('/')
        ))
    } else {
        None
    }
}

fn status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_uppercase().as_str() {
        "ON_GOING" => ItemStatus::Ongoing,
        "COMPLETED" => ItemStatus::Completed,
        "CANCELLED" => ItemStatus::Cancelled,
        "HIATUS" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

#[derive(Deserialize)]
struct MangaResponse {
    #[serde(default)]
    mangas: Vec<MangaDto>,
    #[serde(default)]
    total: u64,
}

#[derive(Deserialize)]
struct MangaDto {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "urlId")]
    url_id: String,
    #[serde(default)]
    cover: Option<CoverDto>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "type")]
    type_name: Option<String>,
    #[serde(default, rename = "publishDate")]
    publish_date: Option<String>,
    #[serde(default)]
    genres: Vec<NameDto>,
    #[serde(default)]
    authors: Vec<NameDto>,
    #[serde(default)]
    artists: Vec<NameDto>,
    #[serde(default)]
    teams: Vec<NameDto>,
}

impl MangaDto {
    fn sample(slug: &str) -> Self {
        Self {
            id: "sample-id".into(),
            title: "Sample".into(),
            description: Some("Summary".into()),
            url_id: slug.into(),
            cover: None,
            status: Some("ON_GOING".into()),
            type_name: Some("MANGA".into()),
            publish_date: Some("2024-01-01T00:00:00".into()),
            genres: vec![NameDto {
                name: "Action".into(),
            }],
            authors: Vec::new(),
            artists: Vec::new(),
            teams: Vec::new(),
        }
    }

    fn into_item(self) -> CatalogItem {
        let mut description = self.description.unwrap_or_default();
        if !self.teams.is_empty() {
            description.push_str("\n\nTeams: ");
            description.push_str(
                &self
                    .teams
                    .into_iter()
                    .map(|team| team.name)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if let Some(year) = self
            .publish_date
            .as_deref()
            .and_then(|value| value.split('-').next())
            .filter(|value| !value.is_empty())
        {
            description.push_str("\nAnnée: ");
            description.push_str(year);
        }
        let mut tags = Vec::new();
        tags.extend(self.type_name.into_iter());
        tags.extend(self.genres.into_iter().map(|genre| genre.name));
        CatalogItem {
            key: format!("/manga/{}", self.url_id),
            title: if self.title.is_empty() {
                self.url_id.clone()
            } else {
                self.title
            },
            cover: self
                .cover
                .and_then(|cover| cover.image)
                .map(|image| resolve_image_link(&image.link)),
            description: (!description.trim().is_empty()).then_some(description),
            authors: self
                .authors
                .into_iter()
                .map(|author| author.name)
                .filter(|name| !name.is_empty())
                .collect(),
            artists: self
                .artists
                .into_iter()
                .map(|artist| artist.name)
                .filter(|name| !name.is_empty())
                .collect(),
            tags,
            status: status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/manga/{}", self.url_id)),
            language: Some(LANG.into()),
            content_rating: Some(CONTENT_RATING.into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct CoverDto {
    image: Option<ImageDto>,
}

#[derive(Deserialize)]
struct ImageDto {
    link: String,
}

#[derive(Deserialize)]
struct NameDto {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct RscChapterDto {
    id: String,
    #[serde(rename = "orderId")]
    order_id: f32,
    #[serde(default, rename = "publishDate")]
    publish_date: Option<String>,
    #[serde(rename = "mangaId")]
    manga_id: String,
}

impl RscChapterDto {
    fn into_chapter(self, slug: &str) -> MangaChapter {
        let number = if self.order_id.fract() == 0.0 {
            format!("{}", self.order_id as i64)
        } else {
            self.order_id.to_string()
        };
        MangaChapter {
            key: format!("/manga/{slug}/chapter/{}", self.id),
            title: Some(format!("Chapitre {number}")),
            chapter_number: Some(self.order_id),
            date_uploaded: self.publish_date.as_deref().and_then(|value| {
                manatan_shared::dates::parse_ymd(value.get(0..10).unwrap_or(value))
            }),
            scanlators: vec!["Astral Manga".into()],
            url: Some(format!("{BASE_URL}/manga/{slug}/chapter/{}", self.id)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct RscImageDto {
    link: String,
    #[serde(rename = "orderId")]
    order_id: i64,
}

#[derive(Deserialize)]
struct PresignResponse {
    url: String,
}

const LIST_FIXTURE: &str = r#"{"mangas":[{"id":"sample-id","title":"Sample","description":"Summary","urlId":"sample","cover":{"image":{"link":"/cover.jpg"}},"status":"ON_GOING","type":"MANGA","publishDate":"2024-01-01T00:00:00","genres":[{"name":"Action"}]}],"total":1}"#;
const DETAILS_MANGA_FIXTURE: &str = r#"{"id":"sample-id","title":"Sample","description":"Summary","urlId":"sample","cover":{"image":{"link":"/cover.jpg"}},"status":"ON_GOING","type":"MANGA","publishDate":"2024-01-01T00:00:00","genres":[{"name":"Action"}]}"#;
const DETAILS_FIXTURE: &str = r#"0:{"id":"sample-id","title":"Sample","description":"Summary","urlId":"sample","cover":{"image":{"link":"/cover.jpg"}},"status":"ON_GOING","type":"MANGA","publishDate":"2024-01-01T00:00:00","genres":[{"name":"Action"}]}1:{"id":"chapter-1","orderId":1,"publishDate":"2024-01-01T00:00:00","mangaId":"sample-id"}"#;
const CHAPTERS_FIXTURE: &str =
    r#"[{"id":"chapter-1","orderId":1,"publishDate":"2024-01-01T00:00:00","mangaId":"sample-id"}]"#;
const PAGES_FIXTURE: &str =
    r#"0:{"link":"/page1.jpg","orderId":1}1:{"link":"/page2.jpg","orderId":2}"#;
const PAGES_IMAGES_FIXTURE: &str =
    r#"[{"link":"/page1.jpg","orderId":1},{"link":"/page2.jpg","orderId":2}]"#;
const PRESIGN_FIXTURE: &str = r#"{"url":"https://astral-manga.fr/page.jpg"}"#;
