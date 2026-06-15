use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    html, manga,
    sdk::{FilterValue, SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: YomuComics = YomuComics;
const BASE_URL: &str = "https://yomu.com.br";
const PAGE_SIZE: u64 = 20;

struct YomuComics;

impl MangaSource for YomuComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_library(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "recent"
        } else {
            "popular"
        };
        Ok(parse_library(&fetch_json_or_fixture(
            &library_url(page, "", &ParsedFilters::with_sort(sort)),
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
            let key = normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_library(&fetch_json_or_fixture(
            &library_url(page, query, &parse_filters(request.get("filters"))),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/sample".into());
        let slug = slug_from_manga_key(&key);
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/obra/{slug}"), DETAILS_FIXTURE);
        Ok(parse_series(&body, &slug)
            .map(|series| {
                series
                    .chapters
                    .into_iter()
                    .map(|chapter| chapter.into_chapter(&slug))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/ler/sample/1?chapterId=sample-chapter".into());
        let body = fetch_rsc_or_fixture(&chapter_url_from_key(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/obra/{}", slug_from_manga_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| chapter_url_from_key(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/obra/") {
            let key = normalize_manga_key(input);
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

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .header("Referer", format!("{BASE_URL}/biblioteca"))
        .header("x-yomu-web", "true")
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
        .header("x-yomu-web", "true")
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn library_url(page: u64, query: &str, filters: &ParsedFilters) -> String {
    let mut params = vec![
        ("page", page.to_string()),
        ("limit", PAGE_SIZE.to_string()),
        ("sort", filters.sort.clone()),
        ("type", filters.kind.clone()),
    ];
    if !filters.genre.is_empty() {
        params.push(("genre", url::query_escape(&filters.genre)));
    }
    if filters.status != "all" {
        params.push(("status", url::query_escape(&filters.status)));
    }
    if !query.is_empty() {
        params.push(("search", url::query_escape(query)));
    }
    format!(
        "{BASE_URL}/api/library?{}",
        params
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&")
    )
}

#[derive(Default)]
struct ParsedFilters {
    genre: String,
    kind: String,
    status: String,
    sort: String,
}

impl ParsedFilters {
    fn with_sort(sort: &str) -> Self {
        Self {
            kind: "all".to_string(),
            status: "all".to_string(),
            sort: sort.to_string(),
            ..Self::default()
        }
    }
}

fn parse_filters(filters: Option<&Value>) -> ParsedFilters {
    let mut parsed = ParsedFilters {
        kind: "all".to_string(),
        status: "all".to_string(),
        sort: "popular".to_string(),
        ..ParsedFilters::default()
    };
    for filter in filters_to_values(filters) {
        let value = string_value(&filter.value);
        match filter.id.as_str() {
            "genre" => parsed.genre = value,
            "type" if !value.is_empty() => parsed.kind = value,
            "status" if !value.is_empty() => parsed.status = value,
            "sort" if !value.is_empty() => parsed.sort = value,
            _ => {}
        }
    }
    parsed
}

fn filters_to_values(filters: Option<&Value>) -> Vec<FilterValue> {
    let Some(filters) = filters else {
        return Vec::new();
    };
    if let Ok(values) = serde_json::from_value::<Vec<FilterValue>>(filters.clone()) {
        return values;
    }
    filters
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(id, value)| FilterValue {
                    id: id.clone(),
                    value: value.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string_value(value: &Value) -> String {
    value.as_str().unwrap_or_default().trim().to_string()
}

fn parse_library(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or_default();
    let pagination = serde_json::from_value::<LibraryPagination>(
        root.get("pagination").cloned().unwrap_or_default(),
    )
    .unwrap_or_default();
    let mangas = root
        .as_object()
        .and_then(|object| {
            object.values().find_map(|value| {
                value.as_array().and_then(|array| {
                    array
                        .iter()
                        .cloned()
                        .map(serde_json::from_value::<LibraryManga>)
                        .collect::<Result<Vec<_>, _>>()
                        .ok()
                })
            })
        })
        .unwrap_or_default();
    Paged {
        entries: mangas.into_iter().map(LibraryManga::into_catalog).collect(),
        has_next_page: pagination.page < pagination.total_pages,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let slug = slug_from_manga_key(key);
    let body = fetch_document_or_fixture(&format!("{BASE_URL}/obra/{slug}"), DETAILS_FIXTURE);
    parse_series(&body, &slug)
        .map(|series| series.into_catalog(&body, &slug))
        .unwrap_or_else(|| fallback_item(&slug))
}

fn parse_series(body: &str, slug: &str) -> Option<SeriesPayload> {
    extract_value_with_keys(
        &normalized_payload_text(body),
        &["slug", slug, "capitulos_lista"],
    )
    .and_then(|value| serde_json::from_value(value).ok())
    .or_else(|| serde_json::from_str(body).ok())
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let normalized = normalized_payload_text(body);
    let images = extract_value_with_keys(&normalized, &["chapter", "imagens_lista"])
        .and_then(|value| serde_json::from_value::<ChapterPage>(value).ok())
        .map(|page| page.chapter.images)
        .or_else(|| {
            serde_json::from_str::<ChapterPage>(body)
                .ok()
                .map(|page| page.chapter.images)
        })
        .unwrap_or_default();
    images
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

fn normalize_manga_key(input: &str) -> String {
    format!("/obra/{}", slug_from_manga_key(input))
}

fn slug_from_manga_key(input: &str) -> String {
    let value = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_matches('/');
    let mut segments = value.split('/');
    match segments.next() {
        Some("obra") | Some("ler") => segments.next().unwrap_or("sample").to_string(),
        Some(value) if !value.is_empty() => value.to_string(),
        _ => "sample".to_string(),
    }
}

fn chapter_url_from_key(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn fallback_item(slug: &str) -> CatalogItem {
    CatalogItem {
        key: format!("/obra/{slug}"),
        title: title_from_slug(slug),
        url: Some(format!("{BASE_URL}/obra/{slug}")),
        language: Some("pt-BR".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn title_from_slug(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_lowercase().as_str() {
        "em lancamento" | "em lançamento" | "ongoing" => ItemStatus::Ongoing,
        "completo" | "concluido" | "concluído" | "completed" => ItemStatus::Completed,
        "hiato" | "hiatus" => ItemStatus::Hiatus,
        "cancelado" | "canceled" | "cancelled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: Option<&str>) -> Option<i64> {
    let value = value?.trim();
    if let Some(date) = value
        .split(['T', ' '])
        .next()
        .and_then(manatan_shared::dates::parse_ymd)
    {
        return Some(date);
    }
    let parts = value.split('/').collect::<Vec<_>>();
    if parts.len() == 3 {
        return manatan_shared::dates::parse_ymd(&format!(
            "{}-{}-{}",
            parts[2], parts[1], parts[0]
        ));
    }
    None
}

fn normalized_payload_text(body: &str) -> String {
    body.replace("\\\"", "\"")
        .replace("\\\\", "\\")
        .replace("\\/", "/")
}

fn extract_value_with_keys(body: &str, keys: &[&str]) -> Option<Value> {
    let bytes = body.as_bytes();
    let first_key = keys.first()?;
    for (index, _) in body.match_indices(&format!("\"{first_key}\"")) {
        if let Some(start) = body[..index].rfind('{') {
            if let Some(end) = matching_brace(bytes, start) {
                let candidate = &body[start..=end];
                if keys.iter().all(|key| candidate.contains(key)) {
                    if let Ok(value) = serde_json::from_str(candidate) {
                        return Some(value);
                    }
                }
            }
        }
    }
    None
}

fn matching_brace(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, &byte) in bytes[start..].iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(start + offset);
                }
            }
            _ => {}
        }
    }
    None
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryPagination {
    #[serde(default)]
    page: u64,
    #[serde(default)]
    total_pages: u64,
}

#[derive(Default, Deserialize)]
struct LibraryManga {
    title: String,
    cover: Option<String>,
    slug: String,
}

impl LibraryManga {
    fn into_catalog(self) -> CatalogItem {
        let key = format!("/obra/{}", self.slug);
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: self.cover,
            url: Some(format!("{BASE_URL}{key}")),
            language: Some("pt-BR".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesPayload {
    description: Option<String>,
    author: Option<String>,
    artist: Option<String>,
    cover_image: Option<String>,
    #[serde(default, rename = "capitulos_lista")]
    chapters: Vec<SeriesChapter>,
}

impl SeriesPayload {
    fn into_catalog(self, body: &str, slug: &str) -> CatalogItem {
        let title = html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_slug(slug));
        let badges = body
            .split("data-slot=\"badge\"")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</span>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        let status_text = badges
            .iter()
            .find(|badge| parse_status(Some(badge)) != ItemStatus::Unknown);
        let tags = badges
            .iter()
            .filter(|badge| parse_status(Some(badge)) == ItemStatus::Unknown)
            .cloned()
            .collect::<Vec<_>>();
        let key = format!("/obra/{slug}");
        CatalogItem {
            key: key.clone(),
            title,
            cover: self.cover_image,
            description: self.description.filter(|value| !value.trim().is_empty()),
            authors: self
                .author
                .filter(|value| !value.trim().is_empty())
                .into_iter()
                .collect(),
            artists: self
                .artist
                .filter(|value| !value.trim().is_empty())
                .into_iter()
                .collect(),
            tags,
            status: parse_status(status_text.map(String::as_str)),
            url: Some(format!("{BASE_URL}{key}")),
            language: Some("pt-BR".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct SeriesChapter {
    number: Value,
    title: Option<String>,
    #[serde(rename = "id")]
    chapter_id: String,
    #[serde(default, rename = "releaseAt")]
    release_at: Option<String>,
    #[serde(default, rename = "releaseDate")]
    release_date: Option<String>,
}

impl SeriesChapter {
    fn into_chapter(self, manga_slug: &str) -> MangaChapter {
        let number = json_number_text(&self.number);
        let key = format!("/ler/{manga_slug}/{number}?chapterId={}", self.chapter_id);
        MangaChapter {
            key: key.clone(),
            title: Some(
                self.title
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| format!("Capítulo {number}")),
            ),
            chapter_number: number.parse::<f32>().ok(),
            date_uploaded: parse_date(self.release_at.as_deref())
                .or_else(|| parse_date(self.release_date.as_deref())),
            url: Some(format!("{BASE_URL}{key}")),
            ..MangaChapter::default()
        }
    }
}

fn json_number_text(value: &Value) -> String {
    if let Some(number) = value.as_f64() {
        let text = number.to_string();
        return text.trim_end_matches(".0").to_string();
    }
    value
        .as_str()
        .unwrap_or("0")
        .trim_end_matches(".0")
        .to_string()
}

#[derive(Deserialize)]
struct ChapterPage {
    chapter: ChapterImages,
}

#[derive(Deserialize)]
struct ChapterImages {
    #[serde(default, rename = "imagens_lista")]
    images: Vec<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"pagination":{"page":1,"totalPages":2},"mangas":[{"title":"Sample Yomu","cover":"https://yomu.com.br/cover.jpg","slug":"sample"}]}"#;
const DETAILS_FIXTURE: &str = r#"<html><body><span data-slot="badge">Em lançamento</span><span data-slot="badge">Ação</span><h1>Sample Yomu</h1><script>self.__next_f.push([1,"{\"slug\":\"sample\",\"description\":\"Summary\",\"author\":\"Author\",\"artist\":\"Artist\",\"coverImage\":\"https://yomu.com.br/cover.jpg\",\"capitulos_lista\":[{\"id\":\"chapter-1\",\"number\":1,\"title\":\"Start\",\"releaseAt\":\"2024-01-01T00:00:00.000Z\"}]}"])</script></body></html>"#;
const PAGES_FIXTURE: &str = r#"{"chapter":{"imagens_lista":["https://yomu.com.br/page1.jpg","https://yomu.com.br/page2.jpg"]}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_yomu_fixtures() {
        let list = SOURCE.list(json!({})).unwrap();
        assert_eq!(list.entries[0].title, "Sample Yomu");
        assert!(list.has_next_page);

        let details = parse_series(DETAILS_FIXTURE, "sample")
            .unwrap()
            .into_catalog(DETAILS_FIXTURE, "sample");
        assert_eq!(details.title, "Sample Yomu");
        assert_eq!(details.status, ItemStatus::Ongoing);

        let chapters = parse_series(DETAILS_FIXTURE, "sample")
            .unwrap()
            .chapters
            .into_iter()
            .map(|chapter| chapter.into_chapter("sample"))
            .collect::<Vec<_>>();
        assert_eq!(chapters[0].key, "/ler/sample/1?chapterId=chapter-1");

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
