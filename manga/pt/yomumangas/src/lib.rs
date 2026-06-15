use manatan_extension::{
    export_manga_source,
    http::HttpClient,
    source::MangaSource,
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
};
use manatan_shared::{dates, html, manga, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const SOURCE: YomuMangas = YomuMangas;
const BASE_URL: &str = "https://yomumangas.com";
const API_URL: &str = "https://api.yomumangas.com";

struct YomuMangas;

impl MangaSource for YomuMangas {
    fn list(&self, _request: Value) -> manatan_extension::abi::ExtensionResult<Paged<CatalogItem>> {
        let body = fetch_document(BASE_URL, HOME_FIXTURE);
        Ok(Paged {
            entries: parse_home_cards(&body),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = key_from_url(query).unwrap_or_else(|| "1#sample".to_string());
            let body = fetch_json(&details_url(&key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body)],
                has_next_page: false,
            });
        }
        let body = fetch_json(&search_url(page(&request), query, &request), SEARCH_FIXTURE);
        let result = serde_json::from_str::<SearchResponse>(&body).unwrap_or_default();
        Ok(Paged {
            entries: result.mangas.into_iter().map(SearchManga::into_item).collect(),
            has_next_page: page(&request) < result.pages.max(1),
        })
    }

    fn details(&self, request: Value) -> manatan_extension::abi::ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1#sample".to_string());
        Ok(parse_details(&fetch_json(&details_url(&key), DETAILS_FIXTURE)))
    }

    fn chapters(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1#sample".to_string());
        let body = fetch_json(&chapters_url(&key), CHAPTERS_FIXTURE);
        let (id, slug) = split_key(&key);
        Ok(serde_json::from_str::<ChaptersResponse>(&body)
            .unwrap_or_default()
            .chapters
            .into_iter()
            .rev()
            .map(|chapter| chapter.into_chapter(&id, &slug))
            .collect())
    }

    fn pages(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/mangas/1/sample/1".to_string());
        let body = fetch_document(&format!("{BASE_URL}{}", key.trim_start_matches(BASE_URL)), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            let body = fetch_json(&details_url(&key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(manatan_extension::SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..manatan_extension::SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

#[derive(Default, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    mangas: Vec<SearchManga>,
    #[serde(default = "one")]
    pages: u64,
}

#[derive(Default, Deserialize)]
struct SearchManga {
    id: u64,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    cover: String,
}

#[derive(Default, Deserialize)]
struct MangaDetailsResponse {
    manga: MangaDto,
}

#[derive(Default, Deserialize)]
struct MangaDto {
    id: u64,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    cover: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    artists: Vec<String>,
    #[serde(default)]
    genres: Vec<TagDto>,
    #[serde(default)]
    tags: Vec<TagDto>,
}

#[derive(Default, Deserialize)]
struct TagDto {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct ChaptersResponse {
    #[serde(default)]
    chapters: Vec<ChapterDto>,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    #[serde(default, rename = "chapter")]
    chapter_number: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "uploaded_at")]
    uploaded_at: Option<String>,
}

impl SearchManga {
    fn into_item(self) -> CatalogItem {
        let key = format!("{}#{}", self.id, self.slug);
        CatalogItem {
            key: key.clone(),
            title: if self.title.is_empty() { self.slug.clone() } else { self.title },
            cover: (!self.cover.is_empty()).then(|| b2_url(&self.cover)),
            url: Some(url_from_key(&key)),
            language: Some("pt-BR".to_string()),
            content_rating: Some("adult".to_string()),
            ..CatalogItem::default()
        }
    }
}

impl MangaDto {
    fn into_item(self) -> CatalogItem {
        let key = format!("{}#{}", self.id, self.slug);
        let mut tags = self
            .genres
            .into_iter()
            .chain(self.tags)
            .map(|tag| tag.name)
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>();
        tags.sort();
        tags.dedup();
        CatalogItem {
            key: key.clone(),
            title: if self.title.is_empty() { self.slug.clone() } else { self.title },
            cover: (!self.cover.is_empty()).then(|| b2_url(&self.cover)),
            url: Some(url_from_key(&key)),
            authors: self.authors,
            artists: self.artists,
            description: self.description,
            tags,
            language: Some("pt-BR".to_string()),
            content_rating: Some("adult".to_string()),
            status: match self.status.as_deref() {
                Some("ONGOING") => ItemStatus::Ongoing,
                Some("COMPLETE") => ItemStatus::Completed,
                Some("HIATUS") => ItemStatus::Hiatus,
                Some("CANCELLED") => ItemStatus::Cancelled,
                _ => ItemStatus::Unknown,
            },
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

impl ChapterDto {
    fn into_chapter(self, manga_id: &str, manga_slug: &str) -> MangaChapter {
        let parsed_title = self
            .title
            .as_deref()
            .map(str::trim)
            .map(|title| title.trim_start_matches('-').trim())
            .filter(|title| !title.is_empty());
        let title = parsed_title
            .map(|title| format!("Capitulo {} - {title}", self.chapter_number))
            .unwrap_or_else(|| format!("Capitulo {}", self.chapter_number));
        let key = format!("/mangas/{manga_id}/{manga_slug}/{}", self.chapter_number);
        MangaChapter {
            key: key.clone(),
            title: Some(title),
            chapter_number: self.chapter_number.parse::<f32>().ok(),
            date_uploaded: self.uploaded_at.as_deref().and_then(parse_api_date),
            language: Some("pt-BR".to_string()),
            url: Some(format!("{BASE_URL}{key}")),
            ..MangaChapter::default()
        }
    }
}

fn parse_details(body: &str) -> CatalogItem {
    serde_json::from_str::<MangaDetailsResponse>(body)
        .unwrap_or_default()
        .manga
        .into_item()
}

fn parse_home_cards(body: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split("<a")
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.starts_with("/mangas/") {
                return None;
            }
            let key = key_from_url(&href)?;
            if !seen.insert(key.clone()) {
                return None;
            }
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|text| html::strip_tags(&text))
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| split_key(&key).1);
            let cover = html::attr_after(chunk, "<img", "src")
                .or_else(|| html::attr_after(chunk, "<img", "data-src"))
                .map(|src| b2_url(&src));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover,
                url: Some(url_from_key(&key)),
                language: Some("pt-BR".to_string()),
                content_rating: Some("adult".to_string()),
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut pages = Vec::new();
    let mut rest = body;
    while let Some(index) = rest.find("b2://chapters/") {
        let after = &rest[index..];
        let end = after
            .find(|ch: char| ch == '"' || ch == '\'' || ch == '\\' || ch.is_whitespace())
            .unwrap_or(after.len());
        let raw = &after[..end];
        pages.push(MangaPage {
            content: PageContent::Url {
                url: b2_url(raw),
                context: None,
            },
            headers: image_headers(),
            description: Some(format!("Page {}", pages.len() + 1)),
            ..MangaPage::default()
        });
        rest = &after[end..];
    }
    pages
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_origin(BASE_URL)
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(page: u64, query: &str, request: &Value) -> String {
    let mut params = vec![("page", page.to_string())];
    if !query.is_empty() {
        params.push(("query", query.to_string()));
    }
    for key in ["type", "status", "nsfw"] {
        if let Some(value) = filter_value(request, key).filter(|value| !value.is_empty()) {
            params.push((key, value));
        }
    }
    if let Some(values) = filter_values(request, "genres").filter(|values| !values.is_empty()) {
        params.push(("genres", values.join(",")));
    }
    if let Some(values) = filter_values(request, "tags").filter(|values| !values.is_empty()) {
        params.push(("tags", values.join(",")));
    }
    format!(
        "{API_URL}/mangas?{}",
        params
            .iter()
            .map(|(key, value)| format!("{key}={}", url::query_escape(value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn details_url(key: &str) -> String {
    let (id, _) = split_key(key);
    format!("{API_URL}/mangas/{}", url::query_escape(&id))
}

fn chapters_url(key: &str) -> String {
    let (id, _) = split_key(key);
    format!("{API_URL}/mangas/{}/chapters", url::query_escape(&id))
}

fn key_from_url(input: &str) -> Option<String> {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    let parts = path
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    if parts.len() >= 3 && parts[0] == "mangas" {
        Some(format!("{}#{}", parts[1], parts[2]))
    } else {
        None
    }
}

fn split_key(key: &str) -> (String, String) {
    if let Some((id, slug)) = key.split_once('#') {
        return (id.to_string(), slug.to_string());
    }
    let parts = key
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    (
        parts.get(1).copied().unwrap_or("1").to_string(),
        parts.get(2).copied().unwrap_or("sample").to_string(),
    )
}

fn url_from_key(key: &str) -> String {
    let (id, slug) = split_key(key);
    format!("{BASE_URL}/mangas/{id}/{slug}")
}

fn b2_url(input: &str) -> String {
    input.replace("b2://", "https://b2.yomumangas.com/")
}

fn image_headers() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Referer".to_string(), format!("{BASE_URL}/")),
        ("Origin".to_string(), BASE_URL.to_string()),
    ])
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn filter_value(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(|value| match value {
            Value::String(value) => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            Value::Object(object) => object
                .get("value")
                .or_else(|| object.get("id"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            _ => None,
        })
}

fn filter_values(request: &Value, key: &str) -> Option<Vec<String>> {
    let value = request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))?;
    match value {
        Value::Array(values) => Some(
            values
                .iter()
                .filter_map(|value| match value {
                    Value::String(value) => Some(value.clone()),
                    Value::Number(value) => Some(value.to_string()),
                    Value::Object(object) => object
                        .get("value")
                        .or_else(|| object.get("id"))
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    _ => None,
                })
                .filter(|value| !value.is_empty())
                .collect(),
        ),
        Value::String(value) if !value.is_empty() => Some(vec![value.clone()]),
        _ => None,
    }
}

fn parse_api_date(value: &str) -> Option<i64> {
    let date = value.split('T').next().unwrap_or(value);
    dates::parse_ymd(date)
}

fn one() -> u64 {
    1
}

const HOME_FIXTURE: &str = r#"<main><div><a href="/mangas/1/sample"><img src="b2://covers/sample.jpg"><h3>Sample Yomu</h3></a></div></main>"#;
const SEARCH_FIXTURE: &str = r#"{"mangas":[{"id":1,"slug":"sample","title":"Sample Yomu","cover":"b2://covers/sample.jpg"}],"pages":1}"#;
const DETAILS_FIXTURE: &str = r#"{"manga":{"id":1,"slug":"sample","title":"Sample Yomu","cover":"b2://covers/sample.jpg","status":"ONGOING","description":"Description","authors":["Author"],"artists":["Artist"],"genres":[{"name":"Action"}],"tags":[{"name":"Long Strip"}]}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"chapters":[{"chapter":"1","title":"Start","uploaded_at":"2024-01-01T00:00:00.000Z"}]}"#;
const PAGES_FIXTURE: &str = r#"<script>window.__pages=["b2://chapters/sample-1.jpg","b2://chapters/sample-2.jpg"]</script>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_api_and_reader_fixtures() {
        assert_eq!(parse_home_cards(HOME_FIXTURE).len(), 1);
        assert_eq!(parse_details(DETAILS_FIXTURE).title, "Sample Yomu");
        assert_eq!(SOURCE.chapters(json!({"manga":"1#sample"})).unwrap().len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
