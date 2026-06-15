use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: YaoiLib = YaoiLib;
const DEFAULT_BASE_URL: &str = "https://v2.shlib.life";
const DEFAULT_API_URL: &str = "https://api.cdnlibs.org";
const SITE_ID: i32 = 2;

struct YaoiLib;

impl MangaSource for YaoiLib {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_page(LIST_FIXTURE, &request));
        }
        let api = api_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{api}/api/latest-updates?page={page}")
        } else {
            format!("{api}/api/manga?site_id[]={SITE_ID}&page={page}")
        };
        Ok(parse_page(
            &fetch_json(&request, &target, LIST_FIXTURE),
            &request,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with("http://")
            || query.starts_with("https://")
            || query.starts_with("slug:")
        {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_json(&request, &details_url(&request, &key), DETAILS_FIXTURE),
                    Some(key),
                    &request,
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_page(
            &fetch_json(&request, &search_url(&request, page, query), LIST_FIXTURE),
            &request,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/1--sample".into());
        Ok(parse_details(
            &fetch_json(&request, &details_url(&request, &key), DETAILS_FIXTURE),
            Some(key),
            &request,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/1--sample".into());
        Ok(parse_chapters(
            &fetch_json(
                &request,
                &format!("{}/api/manga{}/chapters", api_url(&request), key),
                CHAPTERS_FIXTURE,
            ),
            &key,
            &request,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/1--sample/chapter?&volume=1&number=1".into());
        Ok(parse_pages(
            &fetch_json(
                &request,
                &format!("{}/api/manga{}", api_url(&request), key),
                PAGES_FIXTURE,
            ),
            &request,
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| format!("{base}/ru/manga{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let slug = key.trim_start_matches('/').split('/').next().unwrap_or("");
            let volume = param(&key, "volume").unwrap_or("1");
            let number = param(&key, "number").unwrap_or("1");
            let branch = param(&key, "branch_id")
                .map(|id| format!("&bid={id}"))
                .unwrap_or_default();
            let user = pref(&request, "userId")
                .filter(|id| !id.is_empty())
                .map(|id| format!("&ui={id}"))
                .unwrap_or_default();
            format!("{base}/ru/{slug}/read/v{volume}/c{number}?{branch}{user}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(&base_url(&request)) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_json(&request, &details_url(&request, &key), DETAILS_FIXTURE),
                    Some(key),
                    &request,
                )),
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

fn client(request: &Value) -> HttpClient {
    let mut client = HttpClient::browser()
        .with_desktop_user_agent()
        .with_header("Accept", "text/html,application/json,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8")
        .with_header("Site-Id", SITE_ID.to_string())
        .with_referer(base_url(request))
        .with_cookies_for(&base_url(request))
        .with_cookies_for(&api_url(request))
        .with_webview_challenge_fallback();
    if let Some(token) = pref(request, "bearerToken").filter(|token| !token.is_empty()) {
        client = client.with_header("Authorization", format!("Bearer {token}"));
    }
    client
}

fn fetch_json(request: &Value, target: &str, fixture: &str) -> String {
    client(request)
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn base_url(request: &Value) -> String {
    pref(request, "domain")
        .filter(|v| v.starts_with("http"))
        .unwrap_or_else(|| DEFAULT_BASE_URL.into())
        .trim_end_matches('/')
        .to_string()
}
fn api_url(request: &Value) -> String {
    pref(request, "apiDomain")
        .filter(|v| v.starts_with("http"))
        .unwrap_or_else(|| DEFAULT_API_URL.into())
        .trim_end_matches('/')
        .to_string()
}
fn pref(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|p| p.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn search_url(request: &Value, page: u64, query: &str) -> String {
    let filters = request.get("filters").unwrap_or(&Value::Null);
    let mut params = vec![format!("site_id[]={SITE_ID}"), format!("page={page}")];
    if !query.is_empty() {
        params.push(format!("q={}", url::query_escape(query)));
    }
    for (filter, param) in [
        ("types", "types[]"),
        ("genres", "genres[]"),
        ("genresExclude", "genres_exclude[]"),
        ("scanlateStatus", "scanlate_status[]"),
        ("status", "status[]"),
    ] {
        for value in selected_values(filters.get(filter)) {
            params.push(format!("{param}={value}"));
        }
    }
    if filters.get("requireChapters").and_then(Value::as_bool) == Some(true) {
        params.push("chap_count_min=1".into());
    }
    let sort_by = filter_id(filters, "sortBy").unwrap_or("popular");
    let sort_type = filter_id(filters, "sortType").unwrap_or("desc");
    params.push(format!("sort_type={sort_type}"));
    if sort_by != "popular" {
        params.push(format!("sort_by={sort_by}"));
    }
    format!("{}/api/manga?{}", api_url(request), params.join("&"))
}

fn details_url(request: &Value, key: &str) -> String {
    let fields = [
        "eng_name",
        "otherNames",
        "summary",
        "rate",
        "genres",
        "tags",
        "teams",
        "authors",
        "publisher",
        "userRating",
        "manga_status_id",
        "status_id",
        "artists",
    ];
    format!(
        "{}/api/manga{}?{}",
        api_url(request),
        key,
        fields
            .iter()
            .map(|field| format!("fields[]={field}"))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn parse_page(body: &str, request: &Value) -> Paged<CatalogItem> {
    let page: ApiPage =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap());
    Paged {
        entries: page
            .data
            .into_iter()
            .map(|manga| short_to_item(manga, request))
            .collect(),
        has_next_page: page.meta.map(|m| m.has_next_page).unwrap_or(false),
    }
}

fn short_to_item(manga: MangaShort, request: &Value) -> CatalogItem {
    let key = format!("/{}", manga.slug_url);
    CatalogItem {
        key: key.clone(),
        title: selected_title(
            &manga.name,
            manga.rus_name.as_deref(),
            manga.eng_name.as_deref(),
            request,
        ),
        cover: manga.cover.default,
        url: Some(format!("{}/ru/manga{key}", base_url(request))),
        language: Some("ru".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>, request: &Value) -> CatalogItem {
    let root: ApiData<MangaFull> = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let manga = root.data;
    let key = key.unwrap_or_else(|| "/1--sample".into());
    let rating = manga
        .rating
        .as_ref()
        .map(|r| format!("Рейтинг: {} (голосов: {})", r.average, r.votes))
        .unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: selected_title(
            &manga.name,
            manga.rus_name.as_deref(),
            manga.eng_name.as_deref(),
            request,
        ),
        cover: manga.cover.default,
        authors: manga.authors.into_iter().map(|v| v.name).collect(),
        artists: manga.artists.into_iter().map(|v| v.name).collect(),
        tags: manga
            .genres
            .into_iter()
            .chain(manga.tags)
            .map(|v| v.name)
            .collect(),
        description: Some(
            [
                manga.other_names.join(" / "),
                rating,
                summary_text(&manga.summary),
            ]
            .into_iter()
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        )
        .filter(|v| !v.is_empty()),
        status: parse_status(
            manga.is_licensed,
            manga.scanlate_status.map(|v| v.label).as_deref(),
            manga.status.map(|v| v.label).as_deref(),
        ),
        url: Some(format!("{}/ru/manga{key}", base_url(request))),
        language: Some("ru".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str, request: &Value) -> Vec<MangaChapter> {
    let root: ApiData<Vec<ChapterDto>> = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).unwrap());
    let show_paid = request
        .get("preferences")
        .and_then(|p| p.get("showPaidChapters"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let slug = manga_key.trim_start_matches('/');
    root.data
        .into_iter()
        .rev()
        .filter_map(|chapter| {
            let branch = chapter.branches.first()?;
            let locked = branch.restricted_view.as_ref().is_some_and(|v| !v.is_open);
            if locked && !show_paid {
                return None;
            }
            let branch_part = branch
                .branch_id
                .map(|id| format!("&branch_id={id}"))
                .unwrap_or_default();
            let title = format!(
                "{}Том {}. Глава {}{}",
                if locked { "$$ " } else { "" },
                chapter.volume,
                chapter.number,
                chapter
                    .name
                    .map(|name| format!(" - {name}"))
                    .unwrap_or_default()
            );
            Some(MangaChapter {
                key: format!(
                    "/{slug}/chapter?{branch_part}&volume={}&number={}",
                    chapter.volume, chapter.number
                ),
                title: Some(title),
                chapter_number: chapter.number.parse::<f32>().ok(),
                scanlators: branch
                    .teams
                    .first()
                    .map(|team| vec![team.name.clone()])
                    .unwrap_or_default(),
                date_uploaded: branch
                    .created_at
                    .get(0..10)
                    .and_then(manatan_shared::dates::parse_ymd),
                is_locked: locked,
                url: Some(format!(
                    "{}/ru/{slug}/read/v{}/c{}",
                    base_url(request),
                    chapter.volume,
                    chapter.number
                )),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, request: &Value) -> Vec<MangaPage> {
    let root: ApiData<PagesDto> =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).unwrap());
    let server = image_server(request);
    let mut pages = root.data.pages;
    pages.sort_by_key(|page| page.slug);
    pages
        .into_iter()
        .map(|page| {
            let image = if page.url.starts_with("http") {
                page.url
            } else {
                format!("{server}{}", page.url)
            };
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(&base_url(request))),
                },
                headers: manga::image_headers(&base_url(request)),
                description: Some(format!("Page {}", page.slug)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn image_server(request: &Value) -> String {
    match pref(request, "imageServer").as_deref() {
        Some("main") => "https://img2.hlib.cc".into(),
        Some("secondary") => "https://img3.hlib.cc".into(),
        _ => "https://img33.imgslib.link".into(),
    }
}

fn normalize_key(value: &str) -> String {
    if let Some(slug) = value.strip_prefix("slug:") {
        return format!("/{slug}");
    }
    let path = value
        .split("/ru/manga/")
        .nth(1)
        .or_else(|| value.split("/manga/").nth(1))
        .unwrap_or(value);
    format!(
        "/{}",
        path.trim_matches('/').split('?').next().unwrap_or(path)
    )
}

fn selected_title(name: &str, rus: Option<&str>, eng: Option<&str>, request: &Value) -> String {
    match pref(request, "titleLanguage").as_deref() {
        Some("rus") => rus.filter(|v| !v.is_empty()).unwrap_or(name).to_string(),
        _ => eng.filter(|v| !v.is_empty()).unwrap_or(name).to_string(),
    }
}

fn parse_status(licensed: bool, scanlate: Option<&str>, title: Option<&str>) -> ItemStatus {
    if licensed {
        return ItemStatus::Unknown;
    }
    match scanlate.or(title).unwrap_or("") {
        "Продолжается" | "Выходит" | "Онгоинг" | "Анонс" => {
            ItemStatus::Ongoing
        }
        "Завершён" | "Вышло" => ItemStatus::Completed,
        "Заморожен" | "Заброшен" | "Приостановлен" => {
            ItemStatus::Hiatus
        }
        "Выпуск прекращён" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn summary_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(summary_text).collect::<Vec<_>>().join(""),
        Value::Object(map) => {
            let mut out = map
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if map.get("type").and_then(Value::as_str) == Some("hardBreak") {
                out.push('\n');
            }
            if let Some(content) = map.get("content") {
                out.push_str(&summary_text(content));
            }
            out
        }
        _ => String::new(),
    }
}

fn param<'a>(input: &'a str, key: &str) -> Option<&'a str> {
    input
        .split(&format!("{key}="))
        .nth(1)
        .map(|v| v.split('&').next().unwrap_or(v))
}
fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .filter_map(option_id)
            .collect(),
        Some(Value::String(value)) => value.split(',').filter_map(option_id).collect(),
        _ => Vec::new(),
    }
}
fn filter_id<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .and_then(|value| value.split_once(':').map(|(id, _)| id).or(Some(value)))
        .filter(|value| !value.is_empty())
}
fn option_id(value: &str) -> Option<String> {
    let id = value
        .trim()
        .split_once(':')
        .map(|(id, _)| id)
        .unwrap_or_else(|| value.trim());
    (!id.is_empty()).then(|| id.to_string())
}

#[derive(Deserialize)]
struct ApiData<T> {
    data: T,
}
#[derive(Deserialize)]
struct ApiPage {
    data: Vec<MangaShort>,
    meta: Option<PageMeta>,
}
#[derive(Deserialize)]
struct PageMeta {
    #[serde(rename = "has_next_page")]
    has_next_page: bool,
}
#[derive(Deserialize)]
struct MangaShort {
    name: String,
    #[serde(rename = "rus_name")]
    rus_name: Option<String>,
    #[serde(rename = "eng_name")]
    eng_name: Option<String>,
    #[serde(rename = "slug_url")]
    slug_url: String,
    cover: Cover,
}
#[derive(Deserialize)]
struct Cover {
    default: Option<String>,
}
#[derive(Deserialize)]
struct MangaFull {
    name: String,
    #[serde(rename = "rus_name")]
    rus_name: Option<String>,
    #[serde(rename = "eng_name")]
    eng_name: Option<String>,
    cover: Cover,
    #[serde(default)]
    authors: Vec<NameType>,
    #[serde(default)]
    artists: Vec<NameType>,
    #[serde(default)]
    genres: Vec<NameType>,
    #[serde(default)]
    tags: Vec<NameType>,
    #[serde(default, rename = "otherNames")]
    other_names: Vec<String>,
    #[serde(default)]
    summary: Value,
    rating: Option<Rating>,
    status: Option<LabelType>,
    #[serde(rename = "scanlateStatus")]
    scanlate_status: Option<LabelType>,
    #[serde(default, rename = "is_licensed")]
    is_licensed: bool,
}
#[derive(Deserialize)]
struct NameType {
    name: String,
}
#[derive(Deserialize)]
struct LabelType {
    label: String,
}
#[derive(Deserialize)]
struct Rating {
    average: f32,
    votes: i32,
}
#[derive(Deserialize)]
struct ChapterDto {
    #[serde(default)]
    branches: Vec<ChapterBranch>,
    name: Option<String>,
    number: String,
    volume: String,
}
#[derive(Deserialize)]
struct ChapterBranch {
    #[serde(rename = "branch_id")]
    branch_id: Option<i32>,
    #[serde(rename = "created_at")]
    created_at: String,
    #[serde(default)]
    teams: Vec<NameType>,
    #[serde(rename = "restricted_view")]
    restricted_view: Option<RestrictedView>,
}
#[derive(Deserialize)]
struct RestrictedView {
    #[serde(rename = "is_open")]
    is_open: bool,
}
#[derive(Deserialize)]
struct PagesDto {
    pages: Vec<PageDto>,
}
#[derive(Deserialize)]
struct PageDto {
    slug: i32,
    url: String,
}

const LIST_FIXTURE: &str = r#"{"data":[{"name":"Sample","rus_name":"Sample","eng_name":"Sample","slug_url":"1--sample","cover":{"default":"https://v2.shlib.life/sample.jpg"}}],"meta":{"has_next_page":false}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"name":"Sample","rus_name":"Sample","eng_name":"Sample","cover":{"default":"https://v2.shlib.life/sample.jpg"},"authors":[],"artists":[],"genres":[],"tags":[],"otherNames":[],"summary":"Description","rating":{"average":5,"votes":1},"status":{"label":"Онгоинг"},"scanlateStatus":{"label":"Продолжается"},"is_licensed":false}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"branches_count":1,"branches":[{"branch_id":1,"created_at":"2024-01-01T00:00:00.000000Z","teams":[{"name":"Team"}],"restricted_view":{"is_open":true},"user":{"username":"User"}}],"name":null,"number":"1","volume":"1"}]}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"pages":[{"slug":1,"url":"/manga/page1.jpg"},{"slug":2,"url":"/manga/page2.jpg"}]}}"#;

export_manga_source!(SOURCE);
