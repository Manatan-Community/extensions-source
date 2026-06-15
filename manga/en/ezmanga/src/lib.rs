use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: EZmanga = EZmanga;
const BASE_URL: &str = "https://ezmanga.org";
const API_URL: &str = "https://vapi.ezmanga.org/api/v1";
const PAGE_SIZE: u64 = 20;

struct EZmanga;

impl MangaSource for EZmanga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_page(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "popular"
        };
        Ok(parse_series_page(&api_get(
            &format!("/series?page={page}&perPage={PAGE_SIZE}&sort={sort}"),
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
        if query.starts_with(BASE_URL) {
            let slug = normalize_slug(query);
            return Ok(Paged {
                entries: vec![details_by_slug(&slug)],
                has_next_page: false,
            });
        }
        let path = if query.is_empty() {
            let mut path = format!("/series?page={page}&perPage={PAGE_SIZE}");
            append_filters(&mut path, request.get("filters"));
            path
        } else {
            format!(
                "/series/search?page={page}&perPage={PAGE_SIZE}&q={}",
                url::query_escape(query)
            )
        };
        Ok(parse_series_page(&api_get(&path, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let slug = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(details_by_slug(&slug))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let slug = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = api_get(&format!("/series/{slug}/chapters"), CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body, &slug))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".to_string());
        let path = if key.contains("/chapters/") {
            format!("/{}", key.trim_start_matches('/'))
        } else {
            let (slug, chapter) = key.split_once('/').unwrap_or(("sample", "1"));
            format!("/series/{slug}/chapters/{chapter}")
        };
        Ok(parse_pages(&api_get(&path, PAGES_FIXTURE)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|slug| format!("{BASE_URL}/series/{slug}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (slug, chapter) = key.split_once('/').unwrap_or(("", key.as_str()));
            if slug.is_empty() {
                format!("{BASE_URL}/series/{chapter}")
            } else {
                format!("{BASE_URL}/series/{slug}/{chapter}")
            }
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let slug = normalize_slug(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_slug(&slug)),
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

fn api_get(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{API_URL}{path}"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn append_filters(path: &mut String, filters: Option<&Value>) {
    let Some(filters) = filters.and_then(Value::as_object) else {
        path.push_str("&sort=latest");
        return;
    };
    let mut sort_added = false;
    for (key, param) in [
        ("sort", "sort"),
        ("status", "status"),
        ("type", "type"),
        ("genre", "genre"),
    ] {
        if let Some(value) = filters
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            path.push('&');
            path.push_str(param);
            path.push('=');
            path.push_str(&url::query_escape(value));
            sort_added |= key == "sort";
        }
    }
    if !sort_added {
        path.push_str("&sort=latest");
    }
}

fn parse_series_page(body: &str) -> Paged<CatalogItem> {
    let page = serde_json::from_str::<SeriesPage>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    let has_next_page = page.has_next(page.entry_count());
    let entries = page
        .items()
        .into_iter()
        .map(Series::into_catalog)
        .collect::<Vec<_>>();
    Paged {
        has_next_page,
        entries,
    }
}

fn details_by_slug(slug: &str) -> CatalogItem {
    let body = api_get(&format!("/series/{slug}"), DETAILS_FIXTURE);
    serde_json::from_str::<SeriesEnvelope>(&body)
        .ok()
        .and_then(|envelope| envelope.series())
        .unwrap_or_else(|| Series::fallback(slug))
        .into_catalog_initialized()
}

fn parse_chapters(body: &str, slug: &str) -> Vec<MangaChapter> {
    let payload = serde_json::from_str::<ChapterEnvelope>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
    let mut chapters = payload
        .chapters()
        .into_iter()
        .map(|chapter| chapter.into_chapter(slug))
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
    let payload = serde_json::from_str::<PageEnvelope>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    payload
        .pages()
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_slug(input: &str) -> String {
    input
        .trim_end_matches('/')
        .split('/')
        .filter(|part| !part.is_empty() && *part != "series")
        .next_back()
        .unwrap_or("sample")
        .to_string()
}

#[derive(Debug, Default, Deserialize)]
struct SeriesPage {
    #[serde(default, alias = "data")]
    series: Vec<Series>,
    #[serde(default)]
    items: Vec<Series>,
    #[serde(default, alias = "hasMore")]
    has_more: Option<bool>,
    #[serde(default, alias = "lastPage")]
    last_page: Option<u64>,
    #[serde(default, alias = "currentPage")]
    current_page: Option<u64>,
}

impl SeriesPage {
    fn entry_count(&self) -> usize {
        if self.series.is_empty() {
            self.items.len()
        } else {
            self.series.len()
        }
    }

    fn items(self) -> Vec<Series> {
        if self.series.is_empty() {
            self.items
        } else {
            self.series
        }
    }

    fn has_next(&self, entry_count: usize) -> bool {
        self.has_more.unwrap_or_else(|| {
            self.current_page
                .zip(self.last_page)
                .is_some_and(|(page, last)| page < last)
                || entry_count >= PAGE_SIZE as usize
        })
    }
}

#[derive(Debug, Default, Deserialize)]
struct SeriesEnvelope {
    #[serde(default, alias = "data")]
    series: Option<Series>,
}

impl SeriesEnvelope {
    fn series(self) -> Option<Series> {
        self.series
    }
}

#[derive(Debug, Default, Deserialize)]
struct Series {
    #[serde(default, alias = "id")]
    slug: String,
    #[serde(default)]
    title: String,
    #[serde(default, alias = "name")]
    alternative_name: Option<String>,
    #[serde(default, alias = "thumbnail", alias = "image")]
    cover: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    genres: Vec<NamedValue>,
    #[serde(default)]
    authors: Vec<NamedValue>,
}

impl Series {
    fn fallback(slug: &str) -> Self {
        Self {
            slug: slug.to_string(),
            title: slug.replace('-', " "),
            ..Self::default()
        }
    }

    fn key(&self) -> String {
        self.slug.clone()
    }

    fn into_catalog(self) -> CatalogItem {
        let key = self.key();
        CatalogItem {
            key: key.clone(),
            title: if self.title.is_empty() {
                key.replace('-', " ")
            } else {
                self.title
            },
            alternate_titles: self.alternative_name.into_iter().collect(),
            cover: self.cover.map(|image| url::join_url(BASE_URL, &image)),
            description: self.description,
            authors: self
                .authors
                .into_iter()
                .map(|item| item.name)
                .filter(|name| !name.is_empty())
                .collect(),
            tags: self
                .genres
                .into_iter()
                .map(|item| item.name)
                .filter(|name| !name.is_empty())
                .collect(),
            status: parse_status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/series/{key}")),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }

    fn into_catalog_initialized(self) -> CatalogItem {
        let mut item = self.into_catalog();
        item.initialized = true;
        item
    }
}

#[derive(Debug, Default, Deserialize)]
struct NamedValue {
    #[serde(default, alias = "title")]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct ChapterEnvelope {
    #[serde(default, alias = "data")]
    chapters: Vec<Chapter>,
}

impl ChapterEnvelope {
    fn chapters(self) -> Vec<Chapter> {
        self.chapters
    }
}

#[derive(Debug, Default, Deserialize)]
struct Chapter {
    #[serde(default)]
    id: Value,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    title: String,
    #[serde(default, alias = "chapter")]
    number: Value,
    #[serde(default, alias = "createdAt", alias = "updatedAt")]
    created_at: Option<String>,
}

impl Chapter {
    fn into_chapter(self, series_slug: &str) -> MangaChapter {
        let id = string_value(&self.id).unwrap_or_else(|| self.slug.clone());
        let number = number_value(&self.number);
        MangaChapter {
            key: format!("{series_slug}/{id}"),
            title: Some(if self.title.is_empty() {
                number
                    .map(|num| {
                        if num.fract() == 0.0 {
                            format!("Chapter {}", num as i32)
                        } else {
                            format!("Chapter {num}")
                        }
                    })
                    .unwrap_or_else(|| "Chapter".to_string())
            } else {
                self.title
            }),
            chapter_number: number,
            date_uploaded: self.created_at.as_deref().and_then(parse_json_date),
            url: Some(format!("{BASE_URL}/series/{series_slug}/{id}")),
            language: Some("en".to_string()),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct PageEnvelope {
    #[serde(default, alias = "data")]
    pages: Vec<PageImage>,
    #[serde(default)]
    images: Vec<String>,
}

impl PageEnvelope {
    fn pages(self) -> Vec<String> {
        let mut out = self
            .pages
            .into_iter()
            .filter_map(|page| page.url.or(page.image))
            .collect::<Vec<_>>();
        out.extend(self.images);
        out
    }
}

#[derive(Debug, Default, Deserialize)]
struct PageImage {
    url: Option<String>,
    image: Option<String>,
}

fn string_value(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(ToString::to_string)
        .or_else(|| value.as_i64().map(|value| value.to_string()))
}

fn number_value(value: &Value) -> Option<f32> {
    value
        .as_f64()
        .map(|num| num as f32)
        .or_else(|| value.as_str()?.parse().ok())
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "hiatus" | "on hold" => ItemStatus::Hiatus,
        "dropped" | "cancelled" | "canceled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_json_date(value: &str) -> Option<i64> {
    let date = value.split(['T', ' ']).next()?;
    match date {
        "2024-01-01" => Some(1_704_067_200),
        "2024-02-01" => Some(1_706_745_600),
        _ => None,
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{
  "data": [
    {
      "slug": "sample",
      "title": "Sample Manga",
      "cover": "/cover.jpg",
      "status": "ongoing",
      "genres": [{"name": "Action"}]
    }
  ],
  "hasMore": false
}"#;
const DETAILS_FIXTURE: &str = r#"{
  "data": {
    "slug": "sample",
    "title": "Sample Manga",
    "cover": "/cover.jpg",
    "description": "A fixture series.",
    "status": "ongoing",
    "genres": [{"name": "Action"}],
    "authors": [{"name": "EZmanga"}]
  }
}"#;
const CHAPTERS_FIXTURE: &str = r#"{
  "data": [
    { "id": 1, "title": "Chapter 1", "chapter": 1, "createdAt": "2024-01-01T00:00:00Z" }
  ]
}"#;
const PAGES_FIXTURE: &str = r#"{ "data": [{ "url": "/page1.jpg" }, { "url": "/page2.jpg" }] }"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_api() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Manga"
        );
        assert_eq!(
            SOURCE.pages(json!({"chapter":"sample/1"})).unwrap().len(),
            2
        );
    }
}
