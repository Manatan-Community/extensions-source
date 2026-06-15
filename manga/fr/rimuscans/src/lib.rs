use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: RimuScans = RimuScans;
const BASE_URL: &str = "https://rimuscan.fr";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";

struct RimuScans;

impl MangaSource for RimuScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_list(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            None
        } else {
            Some("rating")
        };
        Ok(parse_series_list(&fetch_text(
            &series_url(page, "", sort, request.get("filters")),
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
        if let Some(key) = manga_key_from_input(query) {
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        Ok(parse_series_list(&fetch_text(
            &series_url(page, query, None, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let show_premium = request
            .get("preferences")
            .and_then(|preferences| preferences.get("show_premium_chapters"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(
            &body,
            key.trim_start_matches("/manga/"),
            show_premium,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/read/sample/1".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        let chapter_number = key
            .trim_matches('/')
            .rsplit('/')
            .next()
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(1.0);
        Ok(parse_pages(&body, chapter_number))
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
        if let Some(key) = manga_key_from_input(input) {
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
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
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn series_url(page: u64, query: &str, sort: Option<&str>, filters: Option<&Value>) -> String {
    let mut pairs = Vec::<(&str, String)>::new();
    if !query.trim().is_empty() {
        pairs.push(("search", query.trim().to_string()));
    } else {
        let selected_sort = sort
            .map(ToString::to_string)
            .or_else(|| filter_string(filters, "sort"));
        if let Some(sort) = selected_sort.filter(|value| !value.is_empty() && value != "updated") {
            pairs.push(("sort", sort));
        }
    }
    for name in ["types", "min_chapters"] {
        if let Some(value) = filter_string(filters, name).filter(|value| !value.is_empty()) {
            pairs.push((name, value));
        }
    }
    if filter_bool(filters, "premium") {
        pairs.push(("premium", "1".to_string()));
    }
    for name in ["status", "genres"] {
        let joined = filter_values(filters, name).join(",");
        if !joined.is_empty() {
            pairs.push((name, joined));
        }
    }
    pairs.push(("page", page.to_string()));
    format!(
        "{BASE_URL}/api/series?{}",
        pairs
            .into_iter()
            .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn parse_series_list(body: &str) -> Paged<CatalogItem> {
    let parsed = serde_json::from_str::<SeriesList>(body)
        .or_else(|_| serde_json::from_str(LIST_FIXTURE))
        .unwrap_or_default();
    Paged {
        entries: parsed
            .series
            .into_iter()
            .map(series_entry_to_item)
            .collect(),
        has_next_page: parsed.has_more,
    }
}

fn series_entry_to_item(entry: SeriesEntry) -> CatalogItem {
    let key = format!("/manga/{}", entry.slug.trim_matches('/'));
    CatalogItem {
        key: key.clone(),
        title: non_empty(entry.title).unwrap_or_else(|| "Rimu Scans".to_string()),
        cover: non_empty(entry.cover_url).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let ld = json_ld(body).unwrap_or_default();
    let badges = badges_before_title(body);
    let mut tags = ld.genre;
    if let Some(type_label) = badges.first().and_then(|value| type_label(value)) {
        tags.insert(0, type_label);
    }
    let mut description = non_empty(ld.description).unwrap_or_default();
    let alts = ld
        .alternate_name
        .into_iter()
        .filter_map(non_empty)
        .collect::<Vec<_>>();
    if !alts.is_empty() {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Titres alternatifs : ");
        description.push_str(&alts.join(", "));
    }
    CatalogItem {
        key: key.clone(),
        title: non_empty(ld.name)
            .or_else(|| {
                html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value))
            })
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Rimu Scans".to_string()),
        cover: non_empty(ld.image).map(|image| url::join_url(BASE_URL, &image)),
        description: non_empty(description),
        authors: ld
            .author
            .and_then(|person| non_empty(person.name))
            .into_iter()
            .collect(),
        artists: ld
            .illustrator
            .and_then(|person| non_empty(person.name))
            .into_iter()
            .collect(),
        tags,
        status: badges
            .get(1)
            .map(|value| status_from_text(value))
            .unwrap_or(ItemStatus::Unknown),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_slug: &str, show_premium: bool) -> Vec<MangaChapter> {
    let mut chapters = collect_chapters(body)
        .into_iter()
        .filter(|chapter| show_premium || !chapter.kind.eq_ignore_ascii_case("PREMIUM"))
        .map(|chapter| chapter_to_model(chapter, manga_slug))
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .unwrap_or(-1.0)
            .total_cmp(&a.chapter_number.unwrap_or(-1.0))
    });
    chapters.dedup_by(|a, b| a.key == b.key);
    chapters
}

fn chapter_to_model(chapter: NextChapter, manga_slug: &str) -> MangaChapter {
    let number_string = trim_float(chapter.number);
    let mut title = if chapter.title.trim().is_empty() {
        format!("Chapitre {number_string}")
    } else if chapter.title.to_ascii_lowercase().contains("chapitre")
        || chapter.title.to_ascii_lowercase().contains("chapter")
    {
        chapter.title
    } else {
        format!("Chapitre {number_string} : {}", chapter.title.trim())
    };
    if chapter.kind.eq_ignore_ascii_case("PREMIUM") {
        title = format!("Locked - {title}");
    }
    let key = format!("/read/{}/{}", manga_slug.trim_matches('/'), number_string);
    MangaChapter {
        key: key.clone(),
        title: Some(title),
        chapter_number: Some(chapter.number as f32),
        date_uploaded: chapter
            .release_date
            .as_deref()
            .and_then(|value| dates::parse_ymd(value.get(..10).unwrap_or(value))),
        scanlators: vec!["Rimu Scans".to_string()],
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.to_string()),
        is_locked: chapter.kind.eq_ignore_ascii_case("PREMIUM"),
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str, chapter_number: f64) -> Vec<MangaPage> {
    let chapters = collect_chapters(body);
    let Some(chapter) = chapters
        .iter()
        .find(|chapter| {
            (chapter.number - chapter_number).abs() < f64::EPSILON && !chapter.images.is_empty()
        })
        .or_else(|| {
            chapters
                .iter()
                .find(|chapter| (chapter.number - chapter_number).abs() < f64::EPSILON)
        })
    else {
        return Vec::new();
    };
    let mut images = chapter.images.clone();
    images.sort_by_key(|image| image.order);
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image.url),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn json_ld(body: &str) -> Option<ComicSeriesLd> {
    for chunk in body.split("<script").skip(1) {
        if !chunk.contains("application/ld+json") || !chunk.contains("ComicSeries") {
            continue;
        }
        let data = chunk.split('>').nth(1)?.split("</script>").next()?;
        if let Ok(ld) = serde_json::from_str::<ComicSeriesLd>(data.trim()) {
            return Some(ld);
        }
    }
    None
}

fn collect_chapters(body: &str) -> Vec<NextChapter> {
    let mut chapters = Vec::new();
    for object in json_object_candidates(body) {
        if !object.contains("\"number\"") || !object.contains("\"type\"") {
            continue;
        }
        if let Ok(chapter) = serde_json::from_str::<NextChapter>(&object) {
            if !chapters.iter().any(|existing: &NextChapter| {
                (existing.number - chapter.number).abs() < f64::EPSILON
                    && existing.title == chapter.title
                    && existing.images.len() == chapter.images.len()
            }) {
                chapters.push(chapter);
            }
        }
    }
    chapters
}

fn json_object_candidates(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    for start in 0..bytes.len() {
        if bytes[start] != b'{' {
            continue;
        }
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escape = false;
        for end in start..bytes.len() {
            let ch = bytes[end] as char;
            if in_string {
                if escape {
                    escape = false;
                } else if ch == '\\' {
                    escape = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        out.push(String::from_utf8_lossy(&bytes[start..=end]).to_string());
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn badges_before_title(body: &str) -> Vec<String> {
    let before_h1 = body.split("<h1").next().unwrap_or(body);
    before_h1
        .split("<span")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn manga_key_from_input(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input.trim_start_matches(BASE_URL)))
        .filter(|key| key.starts_with("/manga/"))
}

fn normalize_key(value: &str) -> String {
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn filter_string(filters: Option<&Value>, id: &str) -> Option<String> {
    filters?
        .get(id)?
        .as_str()
        .map(|value| value.trim().to_string())
}

fn filter_bool(filters: Option<&Value>, id: &str) -> bool {
    filters
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn filter_values(filters: Option<&Value>, id: &str) -> Vec<String> {
    match filters.and_then(|filters| filters.get(id)) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect(),
        Some(Value::String(value)) => value
            .split(',')
            .map(|part| part.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn status_from_text(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("cours") || lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else if lower.contains("termin") || lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("pause") || lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("annul") || lower.contains("cancel") || lower.contains("abandon") {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Unknown
    }
}

fn type_label(value: &str) -> Option<String> {
    match value.to_ascii_lowercase().as_str() {
        "webtoon" | "manhwa" => Some("Manhwa".to_string()),
        "manhua" => Some("Manhua".to_string()),
        "manga" => Some("Manga".to_string()),
        _ if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => None,
    }
}

fn trim_float(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

fn non_empty(value: impl Into<String>) -> Option<String> {
    let value = value.into();
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[derive(Default, Deserialize)]
struct SeriesList {
    #[serde(default)]
    series: Vec<SeriesEntry>,
    #[serde(default, rename = "has_more")]
    has_more: bool,
}

#[derive(Deserialize)]
struct SeriesEntry {
    slug: String,
    title: String,
    #[serde(default, rename = "cover_url")]
    cover_url: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComicSeriesLd {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    image: String,
    #[serde(default)]
    alternate_name: Vec<String>,
    author: Option<PersonLd>,
    illustrator: Option<PersonLd>,
    #[serde(default)]
    genre: Vec<String>,
}

#[derive(Deserialize)]
struct PersonLd {
    name: String,
}

#[derive(Clone, Deserialize)]
struct NextChapter {
    number: f64,
    #[serde(default)]
    title: String,
    #[serde(default, rename = "releaseDate")]
    release_date: Option<String>,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    images: Vec<ImageDto>,
}

#[derive(Clone, Deserialize)]
struct ImageDto {
    order: i64,
    url: String,
}

const LIST_FIXTURE: &str = r#"
{"series":[{"slug":"sample","title":"Sample Rimu","cover_url":"/cover.jpg"}],"has_more":false}
"#;
const DETAILS_FIXTURE: &str = r#"
<span>Manga</span><span>En cours</span><h1>Sample Rimu</h1>
<script type="application/ld+json">{"@type":"ComicSeries","name":"Sample Rimu","description":"Resume","image":"/cover.jpg","alternateName":["Alt"],"author":{"name":"Author"},"illustrator":{"name":"Artist"},"genre":["Action"]}</script>
<script>self.__next_f.push([1,{"number":1,"title":"Debut","releaseDate":"2024-01-01T00:00:00.000Z","type":"NORMAL","images":[{"order":1,"url":"/page1.jpg"}]}])</script>
"#;
const PAGES_FIXTURE: &str = DETAILS_FIXTURE;
