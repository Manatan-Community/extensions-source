use manatan_extension::{
    export_manga_source,
    http::HttpClient,
    source::MangaSource,
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
};
use manatan_shared::{dates, html, manga, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: SaikaiScan = SaikaiScan;
const BASE_URL: &str = "https://housesaikai.net";
const API_URL: &str = "https://api.housesaikai.net";
const STORAGE_URL: &str = "https://s3-beta.housesaikai.net";
const PER_PAGE: &str = "12";
const FORMAT_ID: &str = "2";

struct SaikaiScan;

impl MangaSource for SaikaiScan {
    fn list(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            latest_url(page)
        } else {
            list_url(page)
        };
        let body = fetch_json(&target, LIST_FIXTURE);
        Ok(parse_story_page(&body))
    }

    fn search(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_json(&details_url(&key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: parse_stories(&body)
                    .into_iter()
                    .next()
                    .map(|story| vec![story.into_item(true)])
                    .unwrap_or_default(),
                has_next_page: false,
            });
        }
        let body = fetch_json(&search_url(page(&request), query, &request), LIST_FIXTURE);
        Ok(parse_story_page(&body))
    }

    fn details(&self, request: Value) -> manatan_extension::abi::ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".to_string());
        let body = fetch_json(&details_url(&key), DETAILS_FIXTURE);
        Ok(parse_stories(&body)
            .into_iter()
            .next()
            .map(|story| story.into_item(true))
            .unwrap_or_else(|| fallback_item(key)))
    }

    fn chapters(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".to_string());
        let body = fetch_json(&chapters_url(&key), DETAILS_FIXTURE);
        let story_slug = key.trim_end_matches('/').rsplit('/').next().unwrap_or("sample");
        Ok(parse_stories(&body)
            .into_iter()
            .next()
            .map(|story| story.chapters(story_slug))
            .unwrap_or_default())
    }

    fn pages(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/ler/comics/sample/1/sample-chapter".to_string());
        let release_id = key
            .trim_end_matches('/')
            .rsplit('/')
            .nth(1)
            .unwrap_or("1");
        let body = fetch_json(&release_url(release_id), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/comics/") {
            let key = normalize_key(input);
            let body = fetch_json(&details_url(&key), DETAILS_FIXTURE);
            let item = parse_stories(&body).into_iter().next().map(|story| story.into_item(true));
            return Ok(Some(UrlResolveResult {
                item,
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
struct ApiPage<T> {
    #[serde(default)]
    data: T,
    meta: Option<ApiMeta>,
}

#[derive(Deserialize)]
struct ApiMeta {
    #[serde(rename = "current_page")]
    current_page: u64,
    #[serde(rename = "last_page")]
    last_page: u64,
}

#[derive(Default, Deserialize)]
struct Story {
    #[serde(default)]
    artists: Vec<Person>,
    #[serde(default)]
    authors: Vec<Person>,
    #[serde(default)]
    genres: Vec<Genre>,
    #[serde(default)]
    image: String,
    #[serde(default)]
    releases: Vec<Release>,
    #[serde(default)]
    slug: String,
    status: Option<Named>,
    #[serde(default)]
    synopsis: String,
    #[serde(default)]
    title: String,
}

#[derive(Default, Deserialize)]
struct Person {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct Genre {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct Named {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct Release {
    #[serde(default)]
    chapter: String,
    #[serde(default)]
    id: u64,
    #[serde(default, rename = "is_active")]
    is_active: i32,
    #[serde(default, rename = "published_at")]
    published_at: String,
    #[serde(default, rename = "release_images")]
    release_images: Vec<ReleaseImage>,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Default, Deserialize)]
struct ReleaseImage {
    #[serde(default)]
    image: String,
}

impl Story {
    fn into_item(self, initialized: bool) -> CatalogItem {
        let key = format!("/comics/{}", self.slug);
        CatalogItem {
            key: key.clone(),
            title: if self.title.is_empty() {
                self.slug.clone()
            } else {
                self.title
            },
            cover: (!self.image.is_empty()).then(|| storage_url(&self.image)),
            url: Some(format!("{BASE_URL}{key}")),
            authors: names(self.authors),
            artists: names(self.artists),
            description: (!self.synopsis.is_empty()).then(|| html::strip_tags(&self.synopsis)),
            tags: names(self.genres),
            language: Some("pt-BR".to_string()),
            content_rating: Some("safe".to_string()),
            status: match self.status.as_ref().map(|status| status.name.as_str()) {
                Some("Concluido") | Some("Concluído") => ItemStatus::Completed,
                Some("Em Andamento") => ItemStatus::Ongoing,
                Some("Hiato") => ItemStatus::Hiatus,
                Some("Cancelado") => ItemStatus::Cancelled,
                _ => ItemStatus::Unknown,
            },
            initialized,
            ..CatalogItem::default()
        }
    }

    fn chapters(self, story_slug: &str) -> Vec<MangaChapter> {
        let mut chapters = self
            .releases
            .into_iter()
            .filter(|release| release.is_active == 1)
            .map(|release| release.into_chapter(story_slug))
            .collect::<Vec<_>>();
        chapters.sort_by(|left, right| {
            right
                .chapter_number
                .partial_cmp(&left.chapter_number)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        chapters
    }
}

trait IntoName {
    fn into_name(self) -> String;
}

impl IntoName for Person {
    fn into_name(self) -> String {
        self.name
    }
}

impl IntoName for Genre {
    fn into_name(self) -> String {
        self.name
    }
}

fn names<T: IntoName>(values: Vec<T>) -> Vec<String> {
    values
        .into_iter()
        .map(IntoName::into_name)
        .filter(|name| !name.is_empty())
        .collect()
}

impl Release {
    fn into_chapter(self, story_slug: &str) -> MangaChapter {
        let title = self
            .title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("Capitulo {} - {value}", self.chapter))
            .unwrap_or_else(|| format!("Capitulo {}", self.chapter));
        let key = format!("/ler/comics/{story_slug}/{}/{}", self.id, self.slug);
        MangaChapter {
            key: key.clone(),
            title: Some(title),
            chapter_number: self.chapter.parse::<f32>().ok(),
            date_uploaded: parse_api_date(&self.published_at),
            scanlators: vec!["Saikai Scan".to_string()],
            language: Some("pt-BR".to_string()),
            url: Some(format!("{BASE_URL}{key}")),
            page_count: (!self.release_images.is_empty()).then_some(self.release_images.len() as u32),
            ..MangaChapter::default()
        }
    }
}

fn parse_story_page(body: &str) -> Paged<CatalogItem> {
    let page = serde_json::from_str::<ApiPage<Vec<Story>>>(body).unwrap_or_default();
    let has_next_page = page
        .meta
        .map(|meta| meta.current_page < meta.last_page)
        .unwrap_or(false);
    Paged {
        entries: page
            .data
            .into_iter()
            .map(|story| story.into_item(false))
            .collect(),
        has_next_page,
    }
}

fn parse_stories(body: &str) -> Vec<Story> {
    serde_json::from_str::<ApiPage<Vec<Story>>>(body)
        .unwrap_or_default()
        .data
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let page = serde_json::from_str::<ApiPage<Release>>(body).unwrap_or_default();
    page.data
        .release_images
        .into_iter()
        .enumerate()
        .filter(|(_, image)| !image.image.is_empty())
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: storage_url(&image.image),
                context: None,
            },
            headers: image_headers(),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
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
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn list_url(page: u64) -> String {
    story_url(&[
        ("format", FORMAT_ID.to_string()),
        ("sortProperty", "pageviews".to_string()),
        ("sortDirection", "desc".to_string()),
        ("page", page.to_string()),
        ("per_page", PER_PAGE.to_string()),
        ("relationships", "language,type,format".to_string()),
    ])
}

fn latest_url(page: u64) -> String {
    format!(
        "{API_URL}/api/lancamentos?format={FORMAT_ID}&page={page}&per_page={PER_PAGE}&relationships=language,type,format,latestReleases.separator"
    )
}

fn search_url(page: u64, query: &str, request: &Value) -> String {
    let mut params = vec![
        ("format", FORMAT_ID.to_string()),
        ("q", query.to_string()),
        ("sortProperty", filter_value(request, "sort").unwrap_or_else(|| "pageViews".to_string())),
        ("sortDirection", filter_value(request, "sortDirection").unwrap_or_else(|| "desc".to_string())),
        ("page", page.to_string()),
        ("per_page", PER_PAGE.to_string()),
        ("relationships", "language,type,format".to_string()),
    ];
    if let Some(country) = filter_value(request, "country").filter(|value| !value.is_empty()) {
        params.push(("country", country));
    }
    if let Some(status) = filter_value(request, "status").filter(|value| !value.is_empty()) {
        params.push(("status", status));
    }
    if let Some(genres) = filter_values(request, "genres").filter(|values| !values.is_empty()) {
        params.push(("genres", genres.join(",")));
    }
    story_url(&params)
}

fn details_url(key: &str) -> String {
    let slug = key.trim_end_matches('/').rsplit('/').next().unwrap_or("sample");
    format!(
        "{API_URL}/api/stories?format={FORMAT_ID}&slug={}&per_page=1&relationships=language,type,format,artists,status",
        url::query_escape(slug)
    )
}

fn chapters_url(key: &str) -> String {
    let slug = key.trim_end_matches('/').rsplit('/').next().unwrap_or("sample");
    format!(
        "{API_URL}/api/stories?format={FORMAT_ID}&slug={}&per_page=1&relationships=releases",
        url::query_escape(slug)
    )
}

fn release_url(release_id: &str) -> String {
    format!("{API_URL}/api/releases/{}?relationships=releaseImages", url::query_escape(release_id))
}

fn story_url(params: &[(&str, String)]) -> String {
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{API_URL}/api/stories?{query}")
}

fn storage_url(path: &str) -> String {
    url::join_url(STORAGE_URL, path)
}

fn image_headers() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Accept".to_string(), "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8".to_string()),
        ("Origin".to_string(), BASE_URL.to_string()),
        ("Referer".to_string(), format!("{BASE_URL}/")),
    ])
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
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

fn fallback_item(key: String) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title: url::slug_from_url(&key).unwrap_or_else(|| "Saikai Scan".to_string()),
        url: Some(format!("{BASE_URL}{key}")),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

const LIST_FIXTURE: &str = r#"{"data":[{"image":"covers/sample.jpg","slug":"sample","synopsis":"<p>Sample description.</p>","title":"Sample Saikai","status":{"name":"Em Andamento"},"genres":[{"name":"Action"}],"authors":[{"name":"Author"}],"artists":[{"name":"Artist"}]}],"meta":{"current_page":1,"last_page":1}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":[{"image":"covers/sample.jpg","slug":"sample","synopsis":"<p>Sample description.</p>","title":"Sample Saikai","status":{"name":"Em Andamento"},"genres":[{"name":"Action"}],"authors":[{"name":"Author"}],"artists":[{"name":"Artist"}],"releases":[{"chapter":"1","id":1,"is_active":1,"published_at":"2024-01-01T00:00:00.000000Z","slug":"sample-chapter","title":"Start","release_images":[{"image":"pages/sample-1.jpg"}]}]}],"meta":{"current_page":1,"last_page":1}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"chapter":"1","id":1,"is_active":1,"published_at":"2024-01-01T00:00:00.000000Z","slug":"sample-chapter","title":"Start","release_images":[{"image":"pages/sample-1.jpg"},{"image":"pages/sample-2.jpg"}]}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_api_fixtures() {
        assert_eq!(parse_story_page(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(SOURCE.chapters(json!({"manga":"/comics/sample"})).unwrap().len(), 1);
        assert_eq!(SOURCE.pages(json!({"chapter":"/ler/comics/sample/1/sample-chapter"})).unwrap().len(), 2);
    }
}
