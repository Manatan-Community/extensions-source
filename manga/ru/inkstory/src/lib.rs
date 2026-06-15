use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: InkStory = InkStory;
const BASE_URL: &str = "https://inkstory.net";
const API_URL: &str = "https://api.inkstory.net";
const PAGE_SIZE: u64 = 30;

struct InkStory;

impl MangaSource for InkStory {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_books(LIST_FIXTURE, 1, None));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let body = fetch_json(&format!("{API_URL}/v2/chapter-update-feed?page={}&size={PAGE_SIZE}", page.saturating_sub(1)), LATEST_FIXTURE);
            return Ok(parse_updates(&body));
        }
        Ok(parse_books(&fetch_json(&books_url(page, None, &Value::Null), LIST_FIXTURE), page, None))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) || query.starts_with("slug:") {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_book_detail(&fetch_json(&book_url_from_key(&key), DETAILS_FIXTURE), Some(key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        Ok(parse_books(&fetch_json(&books_url(page, (!query.is_empty()).then_some(query), filters), LIST_FIXTURE), page, None))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/content/sample#id=1".into());
        Ok(parse_book_detail(&fetch_json(&book_url_from_key(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/content/sample#id=1".into());
        let book_id = manga_id_from_key(&key).unwrap_or_else(|| "1".into());
        let chapters = parse_chapters(
            &fetch_json(&format!("{API_URL}/v2/chapters?bookId={book_id}"), CHAPTERS_FIXTURE),
            &fetch_json(&format!("{API_URL}/v2/branches?bookId={book_id}"), BRANCHES_FIXTURE),
            &request,
        );
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter/1".into());
        let chapter_id = key.trim_end_matches('/').rsplit('/').next().unwrap_or("1");
        let body = fetch_json(&format!("{API_URL}/v2/chapters/{chapter_id}"), PAGES_FIXTURE);
        let secret = fetch_secret(chapter_id);
        Ok(parse_pages(&body, chapter_id, &secret, &request))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}{}", key.split("#id=").next().unwrap_or(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_book_detail(&fetch_json(&book_url_from_key(&key), DETAILS_FIXTURE), Some(key))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
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
        .with_cookies_for(API_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client().get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_secret(chapter_id: &str) -> String {
    client().get(format!("{BASE_URL}/chapter/{chapter_id}")).browser_document().send_text()
        .ok()
        .and_then(|body| body.split("secretKey").nth(1).and_then(|v| v.split('"').nth(1)).map(ToString::to_string))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "UySkp0BzPhwlvP2V".into())
}

fn books_url(page: u64, query: Option<&str>, filters: &Value) -> String {
    let mut params = vec![format!("size={PAGE_SIZE}"), format!("page={page}")];
    let sort_field = filter_id(filters, "sortField").unwrap_or("viewsCount");
    let sort_order = filter_id(filters, "sortOrder").unwrap_or("desc");
    params.push(format!("sort={sort_field},{sort_order}"));
    if let Some(query) = query { params.push(format!("search={}", url::query_escape(query))); }
    for (filter, param) in [
        ("status", "status"),
        ("country", "country"),
        ("contentStatus", "contentStatus"),
        ("formats", "formats"),
        ("labelsInclude", "labelsInclude"),
        ("labelsExclude", "labelsExclude"),
    ] {
        for value in selected_values(filters.get(filter)) {
            params.push(format!("{param}={}", url::query_escape(&value)));
        }
    }
    if filters.get("strictLabelEqual").and_then(Value::as_bool) == Some(true) {
        params.push("strictLabelEqual=true".into());
    }
    format!("{API_URL}/v2/books?{}", params.join("&"))
}

fn parse_books(body: &str, page: u64, total: Option<u64>) -> Paged<CatalogItem> {
    let books: Vec<BookDto> = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap_or_default());
    let has_next_page = total.map(|total| page * PAGE_SIZE < total).unwrap_or(books.len() as u64 >= PAGE_SIZE);
    Paged {
        entries: books.into_iter().map(book_to_item).collect(),
        has_next_page,
    }
}

fn parse_updates(body: &str) -> Paged<CatalogItem> {
    let updates: Vec<ChapterUpdateFeedDto> = serde_json::from_str(body).unwrap_or_default();
    let len = updates.len();
    Paged {
        entries: updates.into_iter().map(|update| book_to_item(update.book)).collect(),
        has_next_page: len as u64 >= PAGE_SIZE,
    }
}

fn parse_book_detail(body: &str, key: Option<String>) -> CatalogItem {
    let book: BookDto = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let mut item = book_to_item(book.clone());
    item.key = key.unwrap_or_else(|| manga_key(&book.slug, &book.id));
    item.description = book.description.filter(|v| !v.trim().is_empty()).map(|mut text| {
        if !book.external_links.is_empty() {
            text.push_str("\n\nExternal links:\n");
            text.push_str(&book.external_links.join("\n"));
        }
        text
    });
    item.authors = book.relations.iter().filter(|rel| rel.kind.as_deref() == Some("AUTHOR")).filter_map(|rel| rel.publisher.as_ref()?.name.clone()).collect();
    item.artists = book.relations.iter().filter(|rel| rel.kind.as_deref() == Some("ARTIST")).filter_map(|rel| rel.publisher.as_ref()?.name.clone()).collect();
    item.tags = book.labels.iter().filter_map(|label| label.name.clone()).chain(book.formats.iter().map(|v| v.to_lowercase().replace('_', " "))).collect();
    item.status = parse_status(book.status.as_deref());
    item.initialized = true;
    item
}

fn book_to_item(book: BookDto) -> CatalogItem {
    let key = manga_key(&book.slug, &book.id);
    CatalogItem {
        key: key.clone(),
        title: title(&book.name, &book.slug),
        cover: book.poster,
        url: Some(format!("{BASE_URL}/content/{}", book.slug)),
        language: Some("ru".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, branches_body: &str, request: &Value) -> Vec<MangaChapter> {
    let mut chapters: Vec<ChapterDto> = serde_json::from_str(body).unwrap_or_default();
    let branches: Vec<BranchDto> = serde_json::from_str(branches_body).unwrap_or_default();
    let mode = request.get("preferences").and_then(|p| p.get("branchMode")).and_then(Value::as_str).unwrap_or("all");
    if mode != "all" {
        chapters = dedupe_chapters(chapters, mode, request, &branches);
    }
    chapters.sort_by(|a, b| b.volume.partial_cmp(&a.volume).unwrap_or(std::cmp::Ordering::Equal).then_with(|| b.number.partial_cmp(&a.number).unwrap_or(std::cmp::Ordering::Equal)));
    chapters.into_iter().map(|chapter| {
        let base = match (chapter.volume, chapter.number) {
            (Some(vol), Some(num)) => format!("Том {} Глава {}", format_decimal(vol), format_decimal(num)),
            (_, Some(num)) => format!("Глава {}", format_decimal(num)),
            (Some(vol), _) => format!("Том {}", format_decimal(vol)),
            _ => "Глава".into(),
        };
        let subtitle = chapter.name.or(chapter.title).unwrap_or_default();
        let mut title = if subtitle.is_empty() || subtitle.eq_ignore_ascii_case(&base) { base } else { format!("{base} - {subtitle}") };
        if chapter.donut == Some(true) { title = format!("🔒 {title}"); }
        MangaChapter {
            key: format!("/chapter/{}", chapter.id),
            title: Some(title),
            chapter_number: chapter.number.map(|v| v as f32),
            scanlators: branch_name(chapter.branch_id.as_deref(), &branches).into_iter().collect(),
            date_uploaded: chapter.created_at.as_deref().and_then(parse_iso_date),
            is_locked: chapter.donut.unwrap_or(false),
            url: Some(format!("{BASE_URL}/chapter/{}", chapter.id)),
            ..MangaChapter::default()
        }
    }).collect()
}

fn dedupe_chapters(chapters: Vec<ChapterDto>, mode: &str, request: &Value, branches: &[BranchDto]) -> Vec<ChapterDto> {
    let preferred = request.get("preferences").and_then(|p| p.get("preferredBranch")).and_then(Value::as_str).unwrap_or("").to_lowercase();
    let source = if mode == "preferred" && !preferred.is_empty() {
        let matching = chapters.iter().filter(|chapter| branch_name(chapter.branch_id.as_deref(), branches).unwrap_or_default().to_lowercase().contains(&preferred)).cloned().collect::<Vec<_>>();
        if matching.is_empty() { chapters } else { matching }
    } else {
        chapters
    };
    let mut out: Vec<ChapterDto> = Vec::new();
    for chapter in source {
        let pos = out.iter().position(|old| old.volume == chapter.volume && old.number == chapter.number);
        if let Some(pos) = pos {
            if chapter.created_at > out[pos].created_at { out[pos] = chapter; }
        } else {
            out.push(chapter);
        }
    }
    out
}

fn parse_pages(body: &str, chapter_id: &str, secret: &str, request: &Value) -> Vec<MangaPage> {
    let chapter: ChapterDto = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).unwrap());
    let mut pages = chapter.pages;
    pages.sort_by_key(|page| page.index.unwrap_or(i32::MAX));
    pages.into_iter().filter_map(|page| page.image).enumerate().map(|(index, raw)| {
        let (image, encrypted) = normalize_image(&raw, request);
        let mut headers = manga::image_headers(BASE_URL);
        if encrypted {
            headers.insert("X-InkStory-Xor-Key".into(), secret.to_string());
            headers.insert("X-InkStory-Chapter-Id".into(), chapter_id.to_string());
        }
        MangaPage {
            content: PageContent::Url { url: image, context: Some(headers.clone()) },
            headers,
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        }
    }).collect()
}

fn normalize_image(raw: &str, request: &Value) -> (String, bool) {
    let mut image = raw.to_string();
    let encrypted = raw.rsplit('/').next().map(|name| name.split('?').next().unwrap_or(name)).and_then(|name| name.split('.').next()).is_some_and(|base| base.len() == 36 && matches!(base.chars().nth(14), Some('s' | 'x')));
    if image.rsplit('/').next().and_then(|name| name.chars().nth(14)) == Some('s') {
        let file = image.rsplit('/').next().unwrap_or_default();
        if file.len() > 14 {
            let mut updated = file.to_string();
            updated.replace_range(14..15, "x");
            image = image.replace(file, &updated);
        }
    }
    if encrypted && !image.contains("width=") {
        let prefs = request.get("preferences").unwrap_or(&Value::Null);
        let width = prefs.get("imageWidth").and_then(Value::as_str).unwrap_or("1600");
        let kind = prefs.get("imageType").and_then(Value::as_str).unwrap_or("webp");
        let quality = prefs.get("imageQuality").and_then(Value::as_str).unwrap_or("75");
        let sep = if image.contains('?') { "&" } else { "?" };
        image = format!("{image}{sep}width={width}&type={kind}&quality={quality}");
    }
    (image, encrypted)
}

fn normalize_key(value: &str) -> String {
    if let Some(slug) = value.strip_prefix("slug:") {
        return format!("/content/{slug}");
    }
    let path = value.strip_prefix(BASE_URL).unwrap_or(value).split('?').next().unwrap_or(value).split('#').next().unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn manga_key(slug: &str, id: &str) -> String { format!("/content/{slug}#id={id}") }
fn manga_id_from_key(key: &str) -> Option<String> { key.split("#id=").nth(1).filter(|v| !v.is_empty()).map(ToString::to_string) }
fn book_url_from_key(key: &str) -> String { format!("{API_URL}/v2/books/{}", key.split("#id=").next().unwrap_or(key).trim_end_matches('/').rsplit('/').next().unwrap_or("sample")) }
fn title(name: &NameDto, fallback: &str) -> String { name.ru.clone().or_else(|| name.en.clone()).or_else(|| name.original.clone()).filter(|v| !v.is_empty()).unwrap_or_else(|| fallback.to_string()) }
fn parse_status(value: Option<&str>) -> ItemStatus { match value { Some("ONGOING") => ItemStatus::Ongoing, Some("DONE") => ItemStatus::Completed, Some("FROZEN") => ItemStatus::Hiatus, _ => ItemStatus::Unknown } }
fn format_decimal(value: f64) -> String { if value.fract() == 0.0 { format!("{}", value as i64) } else { value.to_string().trim_end_matches('0').trim_end_matches('.').to_string() } }
fn parse_iso_date(value: &str) -> Option<i64> { manatan_shared::dates::parse_ymd(value.get(0..10).unwrap_or(value)) }
fn branch_name(id: Option<&str>, branches: &[BranchDto]) -> Option<String> { let id = id?; branches.iter().find(|b| b.id == id).and_then(|b| b.publishers.iter().filter_map(|p| p.name.clone()).next()).or_else(|| Some(format!("Ветка {id}"))) }

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).filter_map(option_id).collect(),
        Some(Value::String(value)) => value.split(',').filter_map(option_id).collect(),
        _ => Vec::new(),
    }
}
fn filter_id<'a>(filters: &'a Value, key: &str) -> Option<&'a str> { filters.get(key).and_then(Value::as_str).and_then(|value| value.split_once(':').map(|(id, _)| id).or(Some(value))).filter(|value| !value.is_empty()) }
fn option_id(value: &str) -> Option<String> { let id = value.trim().split_once(':').map(|(id, _)| id).unwrap_or_else(|| value.trim()); (!id.is_empty()).then(|| id.to_string()) }

#[derive(Clone, Default, Deserialize)]
struct BookDto {
    id: String,
    slug: String,
    #[serde(default)]
    name: NameDto,
    poster: Option<String>,
    description: Option<String>,
    status: Option<String>,
    #[serde(default)]
    labels: Vec<LabelDto>,
    #[serde(default)]
    formats: Vec<String>,
    #[serde(default)]
    relations: Vec<RelationDto>,
    #[serde(default, rename = "externalLinks")]
    external_links: Vec<String>,
}
#[derive(Clone, Default, Deserialize)]
struct NameDto { en: Option<String>, ru: Option<String>, original: Option<String> }
#[derive(Clone, Deserialize)]
struct LabelDto { name: Option<String> }
#[derive(Clone, Deserialize)]
struct RelationDto { #[serde(rename = "type")] kind: Option<String>, publisher: Option<PublisherDto> }
#[derive(Clone, Deserialize)]
struct PublisherDto { name: Option<String> }
#[derive(Default, Deserialize)]
struct ChapterUpdateFeedDto { book: BookDto }
#[derive(Clone, Default, Deserialize)]
struct ChapterDto {
    id: String,
    name: Option<String>,
    title: Option<String>,
    number: Option<f64>,
    volume: Option<f64>,
    #[serde(rename = "branchId")]
    branch_id: Option<String>,
    #[serde(rename = "createdAt")]
    created_at: Option<String>,
    donut: Option<bool>,
    #[serde(default)]
    pages: Vec<ChapterPageDto>,
}
#[derive(Clone, Deserialize)]
struct ChapterPageDto { index: Option<i32>, image: Option<String> }
#[derive(Deserialize)]
struct BranchDto { id: String, #[serde(default)] publishers: Vec<PublisherDto> }

const LIST_FIXTURE: &str = r#"[{"id":"1","slug":"sample","name":{"ru":"Sample"},"poster":"https://inkstory.net/sample.jpg"}]"#;
const LATEST_FIXTURE: &str = r#"[{"book":{"id":"1","slug":"sample","name":{"ru":"Sample"},"poster":"https://inkstory.net/sample.jpg"}}]"#;
const DETAILS_FIXTURE: &str = r#"{"id":"1","slug":"sample","name":{"ru":"Sample"},"poster":"https://inkstory.net/sample.jpg","description":"Description","status":"ONGOING","labels":[{"name":"Драма"}],"formats":["WEBTOON"],"relations":[],"externalLinks":[]}"#;
const CHAPTERS_FIXTURE: &str = r#"[{"id":"1","number":1,"volume":1,"createdAt":"2024-01-01T00:00:00.000Z","pages":[]}]"#;
const BRANCHES_FIXTURE: &str = r#"[]"#;
const PAGES_FIXTURE: &str = r#"{"id":"1","pages":[{"index":1,"image":"https://img.inkstory.net/page1.jpg"},{"index":2,"image":"https://img.inkstory.net/page2.jpg"}]}"#;

export_manga_source!(SOURCE);
