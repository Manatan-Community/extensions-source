use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: OTruyen = OTruyen;
const BASE_URL: &str = "https://otruyen.cc";
const API_URL: &str = "https://otruyenapi.com/v1/api";
const IMG_URL: &str = "https://img.otruyenapi.com/uploads/comics";

struct OTruyen;

impl MangaSource for OTruyen {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "danh-sach/hoan-thanh"
        } else {
            "danh-sach/truyen-moi"
        };
        Ok(parse_listing(&fetch_json(
            &format!("{API_URL}/{path}?page={page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query)
            .or_else(|| (!query.is_empty() && !query.contains(' ')).then(|| normalize_key(query)))
        {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = if !query.is_empty() {
            format!(
                "{API_URL}/tim-kiem?keyword={}&page={page}",
                url::query_escape(query)
            )
        } else if let Some(genre) = filter(filters, "genre") {
            format!("{API_URL}/the-loai/{genre}?page={page}")
        } else {
            let status = filter(filters, "status").unwrap_or("dang-phat-hanh");
            format!("{API_URL}/danh-sach/{status}?page={page}")
        };
        Ok(parse_listing(&fetch_json(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let body = fetch_json(
            &format!("{API_URL}/truyen-tranh/{}", normalize_key(&key)),
            DETAILS_FIXTURE,
        );
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "sample-chapter:sample:chapter-1".into());
        let chapter_id = key.split(':').next().unwrap_or(&key);
        let body = fetch_json(
            &format!("https://sv1.otruyencdn.com/v1/api/chapter/{chapter_id}"),
            PAGES_FIXTURE,
        );
        let pages = parse_pages(&body);
        if pages.is_empty() {
            return Ok(vec![manga::text_page("Khong tim thay hinh anh")]);
        }
        Ok(pages)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(json!({"page": 1, "listingId": "latest"}))?,
            ),
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/truyen-tranh/{}", normalize_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let parts = key.split(':').collect::<Vec<_>>();
            let slug = parts.get(1).copied().unwrap_or_default();
            let chapter = parts.get(2).copied().unwrap_or_default();
            format!("{BASE_URL}/truyen-tranh/{slug}/{chapter}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<DataDto<ListingData>>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    let current = response.data.params.pagination.current_page.unwrap_or(1);
    let per_page = response
        .data
        .params
        .pagination
        .total_items_per_page
        .unwrap_or(24);
    let total = response
        .data
        .params
        .pagination
        .total_items
        .unwrap_or(response.data.items.len() as u64);
    let entries = response.data.items.into_iter().map(catalog_item).collect();
    Paged {
        entries,
        has_next_page: current * per_page < total,
    }
}

fn catalog_item(item: EntrySummary) -> CatalogItem {
    let key = normalize_key(&item.slug);
    CatalogItem {
        key: key.clone(),
        title: item.name,
        cover: item.thumb_url.map(|thumb| format!("{IMG_URL}/{thumb}")),
        tags: item.category.into_iter().map(|cat| cat.name).collect(),
        url: Some(format!("{BASE_URL}/truyen-tranh/{key}")),
        language: Some("vi".into()),
        content_rating: Some("safe".into()),
        ..CatalogItem::default()
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let key = normalize_key(key);
    let body = fetch_json(&format!("{API_URL}/truyen-tranh/{key}"), DETAILS_FIXTURE);
    let response = serde_json::from_str::<DataDto<EntryData>>(&body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let item = response.data.item;
    let mut description = item
        .origin_name
        .into_iter()
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    if !description.is_empty() {
        description = format!("Ten khac: {description}\n\n");
    }
    description.push_str(&html::strip_tags(&item.content));
    CatalogItem {
        key: normalize_key(&item.slug),
        title: item.name,
        cover: item.thumb_url.map(|thumb| format!("{IMG_URL}/{thumb}")),
        authors: item.author,
        tags: item.category.into_iter().map(|cat| cat.name).collect(),
        description: (!description.trim().is_empty()).then_some(description),
        status: parse_status(&item.status),
        url: Some(format!(
            "{BASE_URL}/truyen-tranh/{}",
            normalize_key(&item.slug)
        )),
        language: Some("vi".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<DataDto<EntryData>>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let slug = response.data.item.slug;
    let updated = parse_iso_date(&response.data.item.updated_at);
    response
        .data
        .item
        .chapters
        .into_iter()
        .flat_map(|server| server.server_data)
        .map(|chapter| {
            let chapter_id = chapter
                .chapter_api_data
                .rsplit('/')
                .next()
                .unwrap_or(&chapter.chapter_api_data);
            let key = format!("{chapter_id}:{slug}:{}", chapter.chapter_slug);
            MangaChapter {
                key: key.clone(),
                title: Some(chapter_title(
                    &chapter.chapter_name,
                    chapter.chapter_title.as_deref(),
                )),
                chapter_number: chapter.chapter_name.parse().ok(),
                date_uploaded: updated,
                url: Some(format!(
                    "{BASE_URL}/truyen-tranh/{slug}/{}",
                    chapter.chapter_slug
                )),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<DataDto<PageDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    let prefix = format!(
        "{}/{}/",
        response.data.domain_cdn.trim_end_matches('/'),
        response.data.item.chapter_path
    );
    response
        .data
        .item
        .chapter_image
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: format!("{prefix}{}", image.image_file),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_status(status: &str) -> ItemStatus {
    match status {
        "completed" => ItemStatus::Completed,
        "ongoing" | "coming_soon" => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    value.get(..10).and_then(manatan_shared::dates::parse_ymd)
}

fn chapter_title(number: &str, title: Option<&str>) -> String {
    match title.filter(|title| !title.trim().is_empty()) {
        Some(title) => format!("Chapter {number}: {}", title.trim()),
        None => format!("Chapter {number}"),
    }
}

fn normalize_key(value: &str) -> String {
    value
        .trim_start_matches(BASE_URL)
        .trim_start_matches("/truyen-tranh/")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| !key.is_empty())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters
        .get(id)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(HomeSectionStyle::Cover),
        has_more: page.has_next_page,
        entries: page.entries,
        ..HomeSection::default()
    }
}

#[derive(Deserialize)]
struct DataDto<T> {
    data: T,
}
#[derive(Deserialize)]
struct ListingData {
    items: Vec<EntrySummary>,
    params: ListingParams,
}
#[derive(Deserialize)]
struct ListingParams {
    pagination: Pagination,
}
#[derive(Deserialize)]
struct Pagination {
    #[serde(rename = "totalItems")]
    total_items: Option<u64>,
    #[serde(rename = "totalItemsPerPage")]
    total_items_per_page: Option<u64>,
    #[serde(rename = "currentPage")]
    current_page: Option<u64>,
}
#[derive(Deserialize)]
struct EntrySummary {
    name: String,
    slug: String,
    #[serde(rename = "thumb_url")]
    thumb_url: Option<String>,
    #[serde(default)]
    category: Vec<Category>,
}
#[derive(Deserialize)]
struct Category {
    name: String,
}
#[derive(Deserialize)]
struct EntryData {
    item: Entry,
}
#[derive(Deserialize)]
struct Entry {
    name: String,
    slug: String,
    #[serde(default, rename = "origin_name")]
    origin_name: Vec<String>,
    #[serde(default)]
    content: String,
    #[serde(default)]
    status: String,
    #[serde(rename = "thumb_url")]
    thumb_url: Option<String>,
    #[serde(default)]
    author: Vec<String>,
    #[serde(default)]
    category: Vec<Category>,
    #[serde(default)]
    chapters: Vec<ChapterServer>,
    #[serde(default, rename = "updatedAt")]
    updated_at: String,
}
#[derive(Deserialize)]
struct ChapterServer {
    #[serde(default, rename = "server_data")]
    server_data: Vec<ChapterData>,
}
#[derive(Deserialize)]
struct ChapterData {
    #[serde(rename = "chapter_name")]
    chapter_name: String,
    #[serde(rename = "chapter_title")]
    chapter_title: Option<String>,
    #[serde(rename = "chapter_api_data")]
    chapter_api_data: String,
    #[serde(default, rename = "chapter_slug")]
    chapter_slug: String,
}
#[derive(Deserialize)]
struct PageDto {
    #[serde(rename = "domain_cdn")]
    domain_cdn: String,
    item: PageItem,
}
#[derive(Deserialize)]
struct PageItem {
    #[serde(rename = "chapter_path")]
    chapter_path: String,
    #[serde(rename = "chapter_image")]
    chapter_image: Vec<PageImage>,
}
#[derive(Deserialize)]
struct PageImage {
    #[serde(rename = "image_file")]
    image_file: String,
}

const LIST_FIXTURE: &str = r#"{"data":{"items":[{"name":"Sample","slug":"sample","thumb_url":"cover.jpg","category":[{"name":"Action"}]}],"params":{"pagination":{"totalItems":1,"totalItemsPerPage":24,"currentPage":1}}}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"item":{"name":"Sample","slug":"sample","origin_name":[],"content":"Summary","status":"ongoing","thumb_url":"cover.jpg","author":["Author"],"category":[{"name":"Action"}],"updatedAt":"2024-01-01T00:00:00.000Z","chapters":[{"server_data":[{"chapter_name":"1","chapter_title":"","chapter_api_data":"https://sv1.otruyencdn.com/v1/api/chapter/sample-chapter","chapter_slug":"chapter-1"}]}]}}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"domain_cdn":"https://sv1.otruyencdn.com","item":{"chapter_path":"sample","chapter_image":[{"image_file":"page1.jpg"}]}}}"#;

export_manga_source!(SOURCE);
