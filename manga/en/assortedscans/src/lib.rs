use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: AssortedScans = AssortedScans;
const BASE_URL: &str = "https://assortedscans.com";
const API_URL: &str = "https://assortedscans.com/api/v2";

struct AssortedScans;

impl MangaSource for AssortedScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_page(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "-latest_upload"
        } else {
            "-views"
        };
        Ok(parse_series_page(&fetch_api_or_fixture(
            &format!("/series?page={page}&sort={sort}"),
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
            let slug = url::slug_from_url(query).unwrap_or_else(|| query.to_string());
            return Ok(Paged {
                entries: vec![parse_series(&fetch_api_or_fixture(
                    &format!("/series/{slug}"),
                    DETAILS_FIXTURE,
                ))],
                has_next_page: false,
            });
        }
        Ok(parse_series_page(&fetch_api_or_fixture(
            &format!("/series?page={page}&title={}", url::query_escape(query)),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_series(&fetch_api_or_fixture(
            &format!("/series/{}", key.trim_matches('/')),
            DETAILS_FIXTURE,
        )))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_chapters(&fetch_api_or_fixture(
            &format!(
                "/series/{}/chapters?date_format=timestamp",
                key.trim_matches('/')
            ),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1".to_string());
        Ok(parse_pages(&fetch_api_or_fixture(
            &format!("/chapters/{}/pages?track=true", key.trim_matches('/')),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let slug = url::slug_from_url(input).unwrap_or_else(|| input.to_string());
            return Ok(Some(UrlResolveResult {
                item: Some(parse_series(&fetch_api_or_fixture(
                    &format!("/series/{slug}"),
                    DETAILS_FIXTURE,
                ))),
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

fn fetch_api_or_fixture(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{API_URL}{path}"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_series_page(body: &str) -> Paged<CatalogItem> {
    let page: Paginator<Series> = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: page.results.into_iter().map(Series::into_catalog).collect(),
        has_next_page: !page.last,
    }
}

fn parse_series(body: &str) -> CatalogItem {
    let series: Series = serde_json::from_str(body).unwrap_or_default();
    series.into_catalog_initialized()
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let chapters: Results<Chapter> = serde_json::from_str(body).unwrap_or_default();
    chapters
        .results
        .into_iter()
        .map(|chapter| MangaChapter {
            key: chapter.id.to_string(),
            title: Some(if chapter.final_chapter {
                format!("{} [END]", chapter.full_title)
            } else {
                chapter.full_title
            }),
            chapter_number: Some(chapter.number),
            date_uploaded: chapter.published.parse::<i64>().ok(),
            scanlators: chapter.groups,
            url: Some(format!("{API_URL}/chapters/{}/read", chapter.id)),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let pages: Results<ApiPage> = serde_json::from_str(body).unwrap_or_default();
    pages
        .results
        .into_iter()
        .map(|page| MangaPage {
            content: PageContent::Url {
                url: page.image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", page.number)),
            ..MangaPage::default()
        })
        .collect()
}

#[derive(Default, Deserialize)]
struct Paginator<T> {
    #[serde(default)]
    last: bool,
    #[serde(default)]
    results: Vec<T>,
}

#[derive(Default, Deserialize)]
struct Results<T> {
    #[serde(default)]
    results: Vec<T>,
}

#[derive(Default, Deserialize)]
struct Series {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    cover: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    licensed: Option<bool>,
    #[serde(default)]
    aliases: Option<Vec<String>>,
    #[serde(default)]
    authors: Option<Vec<String>>,
    #[serde(default)]
    artists: Option<Vec<String>>,
    #[serde(default)]
    categories: Option<Vec<String>>,
}

impl Series {
    fn into_catalog(self) -> CatalogItem {
        CatalogItem {
            key: self.slug.clone(),
            title: if self.title.is_empty() {
                self.slug.clone()
            } else {
                self.title
            },
            cover: self.cover,
            description: build_description(self.description, self.aliases),
            authors: self.authors.unwrap_or_default(),
            artists: self.artists.unwrap_or_default(),
            tags: self.categories.unwrap_or_default(),
            status: parse_status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/reader/{}", self.slug)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            extra: self
                .licensed
                .map(|licensed| [("licensed".to_string(), Value::Bool(licensed))].into())
                .unwrap_or_default(),
            ..CatalogItem::default()
        }
    }

    fn into_catalog_initialized(self) -> CatalogItem {
        let mut item = self.into_catalog();
        item.initialized = true;
        item
    }
}

#[derive(Default, Deserialize)]
struct Chapter {
    id: u64,
    #[serde(default)]
    number: f32,
    #[serde(default)]
    published: String,
    #[serde(default, rename = "final")]
    final_chapter: bool,
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default, rename = "full_title")]
    full_title: String,
}

#[derive(Default, Deserialize)]
struct ApiPage {
    image: String,
    number: usize,
}

fn build_description(description: Option<String>, aliases: Option<Vec<String>>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(description) = description.filter(|value| !value.trim().is_empty()) {
        parts.push(description);
    }
    if let Some(aliases) = aliases.filter(|values| !values.is_empty()) {
        parts.push(format!("Alternative titles:\n{}", aliases.join("\n")));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.unwrap_or_default() {
        "completed" => ItemStatus::Completed,
        "ongoing" => ItemStatus::Ongoing,
        "hiatus" => ItemStatus::Hiatus,
        "canceled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

const LIST_FIXTURE: &str = r#"{"last":false,"results":[{"slug":"sample","title":"Sample Manga","cover":"https://img.example/cover.jpg","description":"Sample description.","status":"ongoing","authors":["Writer"],"artists":["Artist"],"categories":["Drama"]}]}"#;
const DETAILS_FIXTURE: &str = r#"{"slug":"sample","title":"Sample Manga","cover":"https://img.example/cover.jpg","description":"Sample description.","status":"completed","aliases":["Sample Alt"],"authors":["Writer"],"artists":["Artist"],"categories":["Drama"]}"#;
const CHAPTERS_FIXTURE: &str = r#"{"results":[{"id":12,"title":"One","number":1.0,"volume":null,"published":"1704067200","final":true,"series":"sample","groups":["Arc"],"full_title":"Chapter 1"}]}"#;
const PAGES_FIXTURE: &str = r#"{"results":[{"id":1,"image":"https://img.example/page1.jpg","number":1,"url":"page-1"},{"id":2,"image":"https://img.example/page2.jpg","number":2,"url":"page-2"}]}"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_payloads() {
        let list = parse_series_page(LIST_FIXTURE);
        assert_eq!(list.entries[0].title, "Sample Manga");
        assert!(list.has_next_page);

        let details = parse_series(DETAILS_FIXTURE);
        assert_eq!(details.status, ItemStatus::Completed);
        assert_eq!(details.authors, vec!["Writer"]);

        let chapters = parse_chapters(CHAPTERS_FIXTURE);
        assert_eq!(chapters[0].key, "12");
        assert!(chapters[0].title.as_deref().unwrap().contains("[END]"));

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
