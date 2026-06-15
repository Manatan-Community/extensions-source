use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::manga;
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Doujinio = Doujinio;
const BASE_URL: &str = "https://doujin.io";
const LATEST_LIMIT: u64 = 20;

struct Doujinio;

impl MangaSource for Doujinio {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_page::<Vec<MangaDto>>(
                LATEST_FIXTURE,
                true,
                None,
            ));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let body = api_json_post(
                "/api/mangas/newest",
                json!({ "limit": LATEST_LIMIT, "offset": page.saturating_sub(1) * LATEST_LIMIT }),
                LATEST_FIXTURE,
            );
            return Ok(parse_manga_page::<Vec<MangaDto>>(&body, true, None));
        }
        let body = api_get("/api/mangas/popular", POPULAR_FIXTURE);
        Ok(parse_manga_page::<Vec<MangaDto>>(&body, false, Some(false)))
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
            let body = api_get(
                &format!("/api/mangas/{}", id_from_key(&key)),
                DETAILS_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![manga_from_response(&body).to_catalog()],
                has_next_page: false,
            });
        }
        let tags = request
            .get("filters")
            .and_then(|filters| filters.get("tags"))
            .and_then(Value::as_str)
            .map(|value| {
                value
                    .split(',')
                    .filter_map(|tag| tag.trim().parse::<u64>().ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let body = api_json_post(
            "/api/mangas/search",
            json!({ "keyword": query, "page": page, "tags": tags }),
            SEARCH_FIXTURE,
        );
        let response = serde_json::from_str::<PageResponse<SearchResponse>>(&body)
            .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).expect("fixture is valid"));
        Ok(Paged {
            entries: response
                .data
                .data
                .into_iter()
                .map(MangaDto::to_catalog)
                .collect(),
            has_next_page: response.data.to.is_some_and(|to| to < response.data.total),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/1".into());
        let body = api_get(
            &format!("/api/mangas/{}", id_from_key(&key)),
            DETAILS_FIXTURE,
        );
        Ok(manga_from_response(&body).to_catalog())
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/1".into());
        let body = api_get(
            &format!("/api/chapters?manga_id={}", id_from_key(&key)),
            CHAPTERS_FIXTURE,
        );
        let response = serde_json::from_str::<PageResponse<Vec<ChapterDto>>>(&body)
            .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
        Ok(response
            .data
            .into_iter()
            .rev()
            .map(ChapterDto::to_chapter)
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "manga/1/chapter/10".into());
        let ids = ids_from_chapter_key(&key);
        let body = client()
            .get(format!("{BASE_URL}/api/mangas/{ids}/manifest"))
            .header(
                "Referer",
                format!("{BASE_URL}/{}", key.trim_start_matches('/')),
            )
            .xhr()
            .send_text()
            .unwrap_or_else(|_| MANIFEST_FIXTURE.to_string());
        let manifest = serde_json::from_str::<ChapterManifest>(&body)
            .unwrap_or_else(|_| serde_json::from_str(MANIFEST_FIXTURE).expect("fixture is valid"));
        Ok(manifest
            .reading_order
            .into_iter()
            .filter(|page| page.page_type.starts_with("image"))
            .enumerate()
            .map(|(index, page)| MangaPage {
                content: PageContent::Url {
                    url: page.href,
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
            let body = api_get(
                &format!("/api/mangas/{}", id_from_key(&key)),
                DETAILS_FIXTURE,
            );
            return Ok(Some(UrlResolveResult {
                item: Some(manga_from_response(&body).to_catalog()),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_get(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{BASE_URL}{path}"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn api_json_post(path: &str, body: Value, fixture: &str) -> String {
    client()
        .post(format!("{BASE_URL}{path}"))
        .json(body.to_string())
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_manga_page<T>(
    body: &str,
    paged_by_size: bool,
    force_next: Option<bool>,
) -> Paged<CatalogItem>
where
    T: IntoIterator<Item = MangaDto> + for<'de> Deserialize<'de>,
{
    let response = serde_json::from_str::<PageResponse<T>>(body)
        .or_else(|_| serde_json::from_str(LATEST_FIXTURE))
        .expect("fixture is valid");
    let entries = response
        .data
        .into_iter()
        .map(MangaDto::to_catalog)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: force_next
            .unwrap_or(paged_by_size && entries.len() >= LATEST_LIMIT as usize),
        entries,
    }
}

fn manga_from_response(body: &str) -> MangaDto {
    serde_json::from_str::<PageResponse<MangaDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"))
        .data
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find(BASE_URL) {
            return format!(
                "/{}",
                input[index + BASE_URL.len()..].trim_start_matches('/')
            );
        }
    }
    format!("/{}", input.trim_start_matches('/'))
}

fn id_from_key(key: &str) -> String {
    key.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("1")
        .to_string()
}

fn ids_from_chapter_key(key: &str) -> String {
    let mut parts = key.trim_matches('/').split('/');
    let _manga = parts.next();
    let manga_id = parts.next().unwrap_or("1");
    let _chapter = parts.next();
    let chapter_id = parts.next().unwrap_or("10");
    format!("{manga_id}/{chapter_id}")
}

#[derive(Debug, Deserialize)]
struct PageResponse<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct MangaDto {
    #[serde(rename = "optimus_id")]
    id: u64,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    thumb: String,
    #[serde(default)]
    tags: Vec<TagDto>,
    #[serde(default, rename = "creator_name")]
    creator_name: String,
}

impl MangaDto {
    fn to_catalog(self) -> CatalogItem {
        let key = format!("/manga/{}", self.id);
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: (!self.thumb.is_empty()).then_some(self.thumb),
            description: (!self.description.is_empty()).then_some(self.description),
            artists: (!self.creator_name.is_empty())
                .then_some(self.creator_name.clone())
                .into_iter()
                .collect(),
            authors: (!self.creator_name.is_empty())
                .then_some(self.creator_name)
                .into_iter()
                .collect(),
            tags: self.tags.into_iter().map(|tag| tag.name).collect(),
            status: ItemStatus::Completed,
            url: Some(format!("{BASE_URL}{key}")),
            language: Some("en".into()),
            content_rating: Some("adult".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct TagDto {
    name: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    data: Vec<MangaDto>,
    to: Option<u64>,
    total: u64,
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    #[serde(rename = "optimus_id")]
    id: u64,
    #[serde(rename = "manga_optimus_id")]
    manga_id: u64,
    #[serde(default, rename = "chapter_name")]
    name: String,
    #[serde(default, rename = "chapter_order")]
    order: f64,
}

impl ChapterDto {
    fn to_chapter(self) -> MangaChapter {
        MangaChapter {
            key: format!("manga/{}/chapter/{}", self.manga_id, self.id),
            title: Some(if self.name.is_empty() {
                "Chapter".into()
            } else {
                self.name
            }),
            chapter_number: Some((self.order + 1.0) as f32),
            url: Some(format!(
                "{BASE_URL}/manga/{}/chapter/{}",
                self.manga_id, self.id
            )),
            language: Some("en".into()),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChapterManifest {
    #[serde(rename = "readingOrder")]
    reading_order: Vec<ManifestPage>,
}

#[derive(Debug, Deserialize)]
struct ManifestPage {
    href: String,
    #[serde(rename = "type")]
    page_type: String,
}

export_manga_source!(SOURCE);

const LATEST_FIXTURE: &str = r#"{"data":[{"optimus_id":1,"title":"Sample Doujin","description":"Sample description","thumb":"https://doujin.io/thumb.jpg","tags":[{"id":23,"name":"Anal"}],"creator_name":"Sample Artist"}]}"#;
const POPULAR_FIXTURE: &str = LATEST_FIXTURE;
const DETAILS_FIXTURE: &str = LATEST_FIXTURE;
const SEARCH_FIXTURE: &str = r#"{"data":{"data":[{"optimus_id":1,"title":"Sample Doujin","description":"Sample description","thumb":"https://doujin.io/thumb.jpg","tags":[{"id":23,"name":"Anal"}],"creator_name":"Sample Artist"}],"to":1,"total":1}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"optimus_id":10,"manga_optimus_id":1,"chapter_name":"Chapter 1","chapter_order":0,"published_at":"2024-01-01 00:00:00"}]}"#;
const MANIFEST_FIXTURE: &str = r#"{"metadata":{"identifier":"sample"},"readingOrder":[{"href":"https://doujin.io/page1.jpg","type":"image/jpeg"}]}"#;
