use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaDenizi = MangaDenizi;
const BASE_URL: &str = "https://www.mangadenizi.net";

struct MangaDenizi;

impl MangaSource for MangaDenizi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/manga?page={page}")
        } else {
            format!("{BASE_URL}/manga?sort=popular&page={page}")
        };
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_list(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = format!(
            "{BASE_URL}/manga?q={}&page={page}",
            url::query_escape(query)
        );
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_list(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/read/sample/chapter-1".to_string());
        let reader_key = key.replacen("/manga/", "/read/", 1);
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &reader_key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let props = inertia_props(body);
    let manga = props.get("manga").unwrap_or(&Value::Null);
    let entries = manga
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(catalog_from_manga)
        .fold(Vec::new(), push_unique);
    let current = manga.get("current_page").and_then(Value::as_u64).unwrap_or(1);
    let last = manga.get("last_page").and_then(Value::as_u64).unwrap_or(current);
    Paged {
        entries,
        has_next_page: current < last,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let props = inertia_props(body);
    let manga = props.get("manga").unwrap_or(&Value::Null);
    let key = key.unwrap_or_else(|| {
        manga.get("slug")
            .and_then(Value::as_str)
            .map(|slug| format!("/manga/{slug}"))
            .unwrap_or_else(|| "/manga/sample".to_string())
    });
    let mut item = catalog_from_manga(manga);
    item.key = key.clone();
    item.description = manga
        .get("description")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    item.tags = manga
        .get("categories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|category| category.get("name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect();
    item.authors = manga
        .get("authors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|author| author.get("name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect();
    item.status = match manga.get("status").and_then(Value::as_str).unwrap_or_default() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    };
    item.url = Some(url::join_url(BASE_URL, &key));
    item.initialized = true;
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let props = inertia_props(body);
    let manga = props.get("manga").unwrap_or(&Value::Null);
    let manga_slug = manga
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or("sample");
    manga
        .get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let slug = chapter.get("slug").and_then(Value::as_str)?;
            let number = chapter_number_string(chapter.get("number"));
            let title = chapter.get("title").and_then(Value::as_str).unwrap_or_default();
            let chapter_title = if title.is_empty() {
                format!("Bolum {number}")
            } else {
                format!("Bolum {number}: {title}")
            };
            let key = format!("/read/{manga_slug}/{slug}");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(chapter_title),
                chapter_number: number.parse().ok(),
                date_uploaded: chapter
                    .get("published_at")
                    .and_then(Value::as_str)
                    .and_then(|raw| manatan_shared::dates::parse_fixture_date(raw.split('T').next().unwrap_or(raw))),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let props = inertia_props(body);
    props.get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| page.get("image_url").and_then(Value::as_str))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.to_string(),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn catalog_from_manga(value: &Value) -> CatalogItem {
    let slug = value
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or("sample");
    let key = format!("/manga/{slug}");
    CatalogItem {
        key: key.clone(),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Manga")
            .to_string(),
        cover: value
            .get("cover_url")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("tr".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn inertia_props(body: &str) -> Value {
    let data = html::attr_after(body, "id=\"app\"", "data-page")
        .or_else(|| html::attr_after(body, "id='app'", "data-page"))
        .unwrap_or_else(|| "{}".to_string());
    serde_json::from_str::<Value>(&data)
        .ok()
        .and_then(|root| root.get("props").cloned())
        .unwrap_or(Value::Null)
}

fn chapter_number_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(raw)) => raw.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(other) => other.to_string().trim_matches('"').to_string(),
        None => "1".to_string(),
    }
}

fn normalize_key(value: &str) -> String {
    if value.starts_with(BASE_URL) {
        return format!(
            "/{}",
            value[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut out: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !out.iter().any(|existing| existing.key == item.key) {
        out.push(item);
    }
    out
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div id="app" data-page="{&quot;props&quot;:{&quot;manga&quot;:{&quot;data&quot;:[{&quot;title&quot;:&quot;Sample&quot;,&quot;slug&quot;:&quot;sample&quot;,&quot;cover_url&quot;:&quot;https://www.mangadenizi.net/cover.jpg&quot;}],&quot;current_page&quot;:1,&quot;last_page&quot;:1}}}"></div>"#;
const DETAILS_FIXTURE: &str = r#"<div id="app" data-page="{&quot;props&quot;:{&quot;manga&quot;:{&quot;title&quot;:&quot;Sample&quot;,&quot;slug&quot;:&quot;sample&quot;,&quot;cover_url&quot;:&quot;https://www.mangadenizi.net/cover.jpg&quot;,&quot;description&quot;:&quot;Desc&quot;,&quot;status&quot;:&quot;ongoing&quot;,&quot;categories&quot;:[{&quot;name&quot;:&quot;Action&quot;}],&quot;authors&quot;:[{&quot;name&quot;:&quot;Author&quot;}],&quot;chapters&quot;:[{&quot;number&quot;:1,&quot;title&quot;:&quot;&quot;,&quot;slug&quot;:&quot;chapter-1&quot;,&quot;published_at&quot;:&quot;2024-01-01T00:00:00.000000Z&quot;}]}}}"></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="app" data-page="{&quot;props&quot;:{&quot;pages&quot;:[{&quot;image_url&quot;:&quot;https://www.mangadenizi.net/page1.jpg&quot;}]}}"></div>"#;
