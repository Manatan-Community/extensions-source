use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ScansFr = ScansFr;
const BASE_URL: &str = "https://scansfr.com";
const API_URL: &str = "https://api.scansfr.com";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "adult";

struct ScansFr;

impl MangaSource for ScansFr {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list(LIST_FIXTURE, "", false));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updated"
        } else {
            "popular"
        };
        Ok(parse_list(
            &fetch_json(
                &manga_list_url(page, sort, "", None, show_nsfw(&request)),
                LIST_FIXTURE,
            ),
            "",
            false,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) || query.starts_with(API_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_json(&detail_api_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let filters = request.get("filters");
        Ok(parse_list(
            &fetch_json(
                &manga_list_url(
                    page,
                    sort_filter(filters),
                    query,
                    filters,
                    show_nsfw(&request),
                ),
                LIST_FIXTURE,
            ),
            query,
            filter_bool(filters, "has_chapters"),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_json(&detail_api_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(
            &fetch_json(&detail_api_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/1".to_string());
        Ok(parse_pages(
            &fetch_chapter_token(&key).unwrap_or_else(|| TOKEN_FIXTURE.to_string()),
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_json(&detail_api_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
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

fn fetch_chapter_token(chapter_key: &str) -> Option<String> {
    let chapter_id = chapter_key.trim_matches('/').replace('/', "-");
    client()
        .post(format!("{API_URL}/api/v1/chapters/{chapter_id}/token"))
        .header("X-Session-ID", "manatan-rust")
        .json("{}")
        .xhr()
        .send_text()
        .ok()
}

fn manga_list_url(
    page: u64,
    sort: &str,
    query: &str,
    filters: Option<&Value>,
    show_nsfw: bool,
) -> String {
    let mut pairs = vec![
        ("page", page.to_string()),
        (
            if show_nsfw { "nsfw" } else { "isNsfw" },
            show_nsfw.to_string(),
        ),
    ];
    if !query.is_empty() {
        pairs.push(("search", query.to_string()));
    } else {
        pairs.push(("sort", sort.to_string()));
    }
    for key in ["type", "status", "genre"] {
        if let Some(value) = filter_str(filters, key).filter(|value| !value.is_empty()) {
            pairs.push((key, value));
        }
    }
    format!(
        "{API_URL}/api/v1/mangas?{}",
        pairs
            .into_iter()
            .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn detail_api_url(key: &str) -> String {
    format!("{API_URL}/api/v1/mangas/{}", slug_from_key(key))
}

fn parse_list(body: &str, query: &str, has_chapters_only: bool) -> Paged<CatalogItem> {
    let data = serde_json::from_str::<MangaListDto>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("ScansFR list fixture"));
    let mut entries = data
        .mangas
        .into_iter()
        .filter(|manga| !has_chapters_only || manga.chapters.unwrap_or(0) > 0)
        .map(|manga| manga.into_item(false))
        .collect::<Vec<_>>();
    let q = query.trim().to_ascii_lowercase();
    if !q.is_empty() {
        entries.sort_by_key(|item| {
            let title = item.title.trim().to_ascii_lowercase();
            if title == q {
                0
            } else if title.starts_with(&q) {
                1
            } else if title.contains(&format!(" {q}")) || title.contains(&format!("-{q}")) {
                2
            } else if title.contains(&q) {
                3
            } else {
                4
            }
        });
    }
    Paged {
        entries,
        has_next_page: data.page < data.total_pages,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let dto = serde_json::from_str::<MangaDetailDto>(body).unwrap_or_else(|_| {
        serde_json::from_str(DETAILS_FIXTURE).expect("ScansFR details fixture")
    });
    let key = key.unwrap_or_else(|| format!("/manga/{}", dto.slug));
    let author = dto.author.clone();
    CatalogItem {
        key: normalize_key(&key),
        title: dto.title,
        cover: Some(url::join_url(API_URL, &dto.cover)),
        description: dto.description.filter(|value| !value.trim().is_empty()),
        authors: author.clone().into_iter().collect(),
        artists: dto
            .artist
            .filter(|artist| author.as_deref() != Some(artist))
            .into_iter()
            .collect(),
        tags: dto.tags,
        status: parse_status(&dto.status),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let dto = serde_json::from_str::<MangaDetailDto>(body).unwrap_or_else(|_| {
        serde_json::from_str(DETAILS_FIXTURE).expect("ScansFR details fixture")
    });
    let slug = slug_from_key(manga_key);
    let mut chapters = dto
        .chapters_list
        .into_iter()
        .map(|chapter| {
            let key = format!("/{slug}/{}", format_number(chapter.number));
            MangaChapter {
                key: key.clone(),
                title: Some(chapter.title),
                chapter_number: Some(chapter.number as f32),
                date_uploaded: chapter.date.as_deref().and_then(parse_iso_date),
                page_count: chapter.page_count,
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|left, right| {
        right
            .chapter_number
            .partial_cmp(&left.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let dto = serde_json::from_str::<ChapterTokenDto>(body)
        .unwrap_or_else(|_| serde_json::from_str(TOKEN_FIXTURE).expect("ScansFR token fixture"));
    (1..=dto.page_count)
        .map(|page| {
            let image = format!(
                "{API_URL}/api/v1/images/{}/{page}?sig={}&exp={}&s={}",
                dto.chapter_id, dto.sig, dto.exp, dto.session_hash
            );
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {page}")),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" | "en cours" => ItemStatus::Ongoing,
        "completed" | "termine" | "terminé" => ItemStatus::Completed,
        "hiatus" | "en pause" => ItemStatus::Hiatus,
        "cancelled" | "abandonne" | "abandonné" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value.get(..10)?)
}

fn format_number(number: f64) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i64)
    } else {
        number.to_string()
    }
}

fn slug_from_key(key: &str) -> String {
    normalize_key(key)
        .trim_start_matches("/manga/")
        .trim_matches('/')
        .to_string()
}

fn normalize_key(input: &str) -> String {
    let path = input
        .strip_prefix(BASE_URL)
        .or_else(|| input.strip_prefix(API_URL))
        .unwrap_or(input);
    if path.contains("/manga/") {
        format!(
            "/manga/{}",
            path.split("/manga/")
                .nth(1)
                .unwrap_or(path)
                .trim_matches('/')
        )
    } else {
        format!("/manga/{}", path.trim_matches('/'))
    }
}

fn show_nsfw(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("show_nsfw"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn sort_filter(filters: Option<&Value>) -> &str {
    filter_str(filters, "sort")
        .filter(|value| !value.is_empty())
        .map(|value| match value.as_str() {
            "latest" => "latest",
            "updated" => "updated",
            "rating" => "rating",
            "alphabetical" => "alphabetical",
            _ => "popular",
        })
        .unwrap_or("popular")
}

fn filter_str(filters: Option<&Value>, key: &str) -> Option<String> {
    filters?
        .get(key)?
        .as_str()
        .map(|value| value.trim().to_string())
}

fn filter_bool(filters: Option<&Value>, key: &str) -> bool {
    filters
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaListDto {
    mangas: Vec<MangaBriefDto>,
    page: i64,
    total_pages: i64,
}

#[derive(Deserialize)]
struct MangaBriefDto {
    slug: String,
    title: String,
    cover: String,
    chapters: Option<i64>,
}

impl MangaBriefDto {
    fn into_item(self, initialized: bool) -> CatalogItem {
        let key = format!("/manga/{}", self.slug);
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: Some(url::join_url(API_URL, &self.cover)),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some(LANG.to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaDetailDto {
    slug: String,
    title: String,
    description: Option<String>,
    cover: String,
    status: String,
    #[serde(default)]
    tags: Vec<String>,
    author: Option<String>,
    artist: Option<String>,
    #[serde(default)]
    chapters_list: Vec<ChapterBriefDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterBriefDto {
    number: f64,
    title: String,
    date: Option<String>,
    page_count: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterTokenDto {
    sig: String,
    exp: i64,
    session_hash: String,
    chapter_id: String,
    page_count: u32,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"mangas":[{"slug":"sample","title":"Sample ScansFR","cover":"/covers/sample/cover.jpg","chapters":1}],"page":1,"totalPages":1}"#;
const DETAILS_FIXTURE: &str = r#"{"slug":"sample","title":"Sample ScansFR","description":"Resume","cover":"/covers/sample/cover.jpg","status":"En cours","tags":["Action"],"author":"Auteur","artist":"Artiste","chaptersList":[{"number":1,"title":"Chapitre 1","date":"2024-01-01T00:00:00.000Z","pageCount":2}]}"#;
const TOKEN_FIXTURE: &str = r#"{"sig":"sig","exp":1893456000,"sessionHash":"session","chapterId":"sample-1","pageCount":2}"#;
