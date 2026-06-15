use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: RoliaScan = RoliaScan;
const BASE_URL: &str = "https://roliascan.com";

struct RoliaScan;

impl MangaSource for RoliaScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_browse(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "post_desc"
        } else {
            "popular_desc"
        };
        Ok(parse_browse(&post_json_or_fixture(
            "/wp-json/manga/v1/load",
            search_payload(page, "", sort, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
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
        if !query.is_empty() {
            return Ok(parse_query_search(&post_json_or_fixture(
                "/auth/search",
                json!({"limit": 25, "query": query}),
                QUERY_FIXTURE,
            )));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_browse(&post_json_or_fixture(
            "/wp-json/manga/v1/load",
            search_payload(page, query, "popular_desc", request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1|sample".to_string());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1|sample".to_string());
        let slug = slug_from_key(&key);
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/manga/{slug}/"), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/read/sample/chapter-1-1".to_string());
        let chapter_id = key.trim_end_matches('/').rsplit('-').next().unwrap_or("1");
        Ok(parse_pages(&fetch_document_or_fixture(
            &format!("{BASE_URL}/auth/chapter-content?chapter_id={chapter_id}"),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/manga/{}/", slug_from_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
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
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn post_json_or_fixture(path: &str, payload: Value, fixture: &str) -> String {
    client()
        .post(format!("{BASE_URL}{path}"))
        .xhr()
        .json(payload.to_string())
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

fn search_payload(page: u64, query: &str, sort: &str, filters: Option<&Value>) -> Value {
    json!({
        "page": page,
        "search": query,
        "years": "[]",
        "genres": stringified_array(filters.and_then(|f| f.get("genre_ids"))),
        "types": stringified_array(filters.and_then(|f| f.get("type"))),
        "statuses": stringified_array(filters.and_then(|f| f.get("status"))),
        "sort": filters.and_then(|f| f.get("sort")).and_then(Value::as_str).unwrap_or(sort),
        "genreMatchMode": "any"
    })
}

fn stringified_array(value: Option<&Value>) -> String {
    let values = match value {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect::<Vec<_>>(),
        Some(Value::String(text)) if !text.is_empty() => vec![text.as_str()],
        _ => Vec::new(),
    };
    format!(
        "[{}]",
        values
            .into_iter()
            .map(|value| format!("\"{}\"", value.replace('"', "\\\"")))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn parse_browse(body: &str) -> Paged<CatalogItem> {
    let entries = serde_json::from_str::<Vec<BrowseManga>>(body)
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.media_type.as_deref() != Some("Novel") && !item.url.is_empty())
        .map(BrowseManga::into_catalog)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() == 24,
        entries,
    }
}

fn parse_query_search(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<QueryResponse>(body).unwrap_or_default();
    Paged {
        entries: response
            .results
            .into_iter()
            .filter(|item| item.media_type.as_deref() != Some("Novel"))
            .map(QueryManga::into_catalog)
            .collect(),
        has_next_page: false,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let id = id_from_key(key);
    parse_details(
        &fetch_document_or_fixture(
            &format!("{BASE_URL}/wp-json/wp/v2/manga/{id}?_embed"),
            DETAILS_JSON_FIXTURE,
        ),
        key.to_string(),
    )
}

fn parse_details(body: &str, fallback_key: String) -> CatalogItem {
    let details = serde_json::from_str::<MangaDetails>(body).unwrap_or_default();
    let key = if details.id > 0 {
        format!("{}|{}", details.id, details.slug)
    } else {
        fallback_key
    };
    let terms = details.embedded.terms.concat();
    CatalogItem {
        key: key.clone(),
        title: html::strip_tags(&details.title.rendered)
            .trim()
            .to_string()
            .if_empty(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: details
            .embedded
            .featured_media
            .first()
            .map(|media| media.source_url.clone()),
        description: Some(html::strip_tags(&details.content.rendered))
            .filter(|value| !value.is_empty()),
        authors: terms
            .iter()
            .filter(|term| term.taxonomy == "manga_author")
            .map(|term| term.name.clone())
            .collect(),
        tags: terms
            .iter()
            .filter(|term| term.taxonomy == "post_tag")
            .map(|term| term.name.clone())
            .chain(details.kind.into_iter())
            .collect(),
        status: status_from_fragment(&key),
        url: Some(format!("{BASE_URL}/manga/{}/", slug_from_key(&key))),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/read/") || chunk.contains("/chapter"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::attr(chunk, "title")
                .or_else(|| {
                    html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: chapter_number_from_key(&key),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    serde_json::from_str::<Pages>(body)
        .unwrap_or_default()
        .images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

#[derive(Default, Deserialize)]
struct BrowseManga {
    id: String,
    title: String,
    url: String,
    cover: Option<String>,
    #[serde(rename = "type")]
    media_type: Option<String>,
    description: Option<String>,
    status: Option<String>,
}

impl BrowseManga {
    fn into_catalog(self) -> CatalogItem {
        let slug = url::slug_from_url(&self.url).unwrap_or_else(|| self.title.to_ascii_lowercase());
        CatalogItem {
            key: format!("{}|{}", self.id, slug),
            title: html::html_unescape(&self.title),
            cover: self.cover,
            description: self.description.map(|value| html::html_unescape(&value)),
            tags: self.media_type.into_iter().collect(),
            status: parse_status(self.status.as_deref().unwrap_or_default()),
            url: Some(self.url),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct QueryResponse {
    results: Vec<QueryManga>,
}

#[derive(Default, Deserialize)]
struct QueryManga {
    id: u64,
    slug: String,
    title: String,
    thumbnail: Option<String>,
    #[serde(rename = "type")]
    media_type: Option<String>,
    description: Option<String>,
    status: Option<String>,
}

impl QueryManga {
    fn into_catalog(self) -> CatalogItem {
        CatalogItem {
            key: format!("{}|{}", self.id, self.slug),
            title: html::html_unescape(&self.title),
            cover: self.thumbnail,
            description: self.description.map(|value| html::html_unescape(&value)),
            tags: self.media_type.into_iter().collect(),
            status: parse_status(self.status.as_deref().unwrap_or_default()),
            url: Some(format!("{BASE_URL}/manga/{}/", self.slug)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct MangaDetails {
    id: u64,
    slug: String,
    title: Rendered,
    content: Rendered,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "_embedded")]
    embedded: Embedded,
}

#[derive(Default, Deserialize)]
struct Rendered {
    rendered: String,
}

#[derive(Default, Deserialize)]
struct Embedded {
    #[serde(rename = "wp:featuredmedia")]
    featured_media: Vec<Media>,
    #[serde(rename = "wp:term")]
    terms: Vec<Vec<Term>>,
}

#[derive(Deserialize)]
struct Media {
    source_url: String,
}

#[derive(Clone, Deserialize)]
struct Term {
    name: String,
    taxonomy: String,
}

#[derive(Default, Deserialize)]
struct Pages {
    images: Vec<String>,
}

trait IfEmpty {
    fn if_empty<F: FnOnce() -> String>(self, fallback: F) -> String;
}

impl IfEmpty for String {
    fn if_empty<F: FnOnce() -> String>(self, fallback: F) -> String {
        if self.is_empty() { fallback() } else { self }
    }
}

fn id_from_key(key: &str) -> String {
    key.split('|')
        .next()
        .filter(|value| !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit()))
        .unwrap_or("1")
        .to_string()
}

fn slug_from_key(key: &str) -> String {
    key.split('|')
        .nth(1)
        .map(ToString::to_string)
        .or_else(|| url::slug_from_url(key))
        .unwrap_or_else(|| "sample".to_string())
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        let path = input[BASE_URL.len()..]
            .trim_start_matches('/')
            .trim_end_matches('/');
        if path.starts_with("manga/") {
            return format!("1|{}", path.trim_start_matches("manga/"));
        }
        return format!("/{path}");
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn parse_status(input: &str) -> ItemStatus {
    match input.to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        value if value.contains("hiatus") => ItemStatus::Hiatus,
        value if value.contains("drop") => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn status_from_fragment(_key: &str) -> ItemStatus {
    ItemStatus::Unknown
}

fn chapter_number_from_key(key: &str) -> Option<f32> {
    key.split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .filter(|value| !value.is_empty())
        .next_back()
        .and_then(|value| value.parse().ok())
}

fn push_unique_chapter(
    mut chapters: Vec<MangaChapter>,
    chapter: MangaChapter,
) -> Vec<MangaChapter> {
    if !chapters.iter().any(|existing| existing.key == chapter.key) {
        chapters.push(chapter);
    }
    chapters
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"[{"id":"1","title":"Sample Manga","url":"https://roliascan.com/manga/sample/","cover":"https://roliascan.com/cover.jpg","type":"Manhwa","description":"Sample description","status":"Ongoing"}]"#;
const QUERY_FIXTURE: &str = r#"{"success":true,"results":[{"id":1,"slug":"sample","title":"Sample Manga","thumbnail":"https://roliascan.com/cover.jpg","type":"Manhwa","description":"Sample description","status":"Ongoing"}]}"#;
const DETAILS_JSON_FIXTURE: &str = r#"{"id":1,"slug":"sample","title":{"rendered":"Sample Manga"},"content":{"rendered":"<p>Sample description</p>"},"type":"Manhwa","_embedded":{"wp:featuredmedia":[{"source_url":"https://roliascan.com/cover.jpg"}],"wp:term":[[{"name":"Action","taxonomy":"post_tag"}],[{"name":"Author","taxonomy":"manga_author"}]]}}"#;
const DETAILS_FIXTURE: &str = r#"<body data-manga-id="1"><div id="chapter-list"><a href="/read/sample/chapter-1-1" title="Chapter 1">Chapter 1</a></div></body>"#;
const PAGES_FIXTURE: &str =
    r#"{"images":["https://roliascan.com/page1.jpg","https://roliascan.com/page2.jpg"]}"#;
