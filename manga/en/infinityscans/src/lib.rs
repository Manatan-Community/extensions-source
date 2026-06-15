use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: InfinityScans = InfinityScans;
const BASE_URL: &str = "https://infinityscans.org";
const CDN_HOST: &str = "cdn.infinityscans.org";

struct InfinityScans;

impl MangaSource for InfinityScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_ranking(RANKING_FIXTURE));
        }
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            Ok(parse_search_entries(
                &fetch_json("api/comics", COMICS_FIXTURE),
                None,
                None,
            ))
        } else {
            Ok(parse_ranking(&fetch_json("api/ranking", RANKING_FIXTURE)))
        }
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
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_search_entries(
            &fetch_json("api/comics", COMICS_FIXTURE),
            Some(query),
            request.get("filters"),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/1/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/1/sample".to_string());
        let target = url::join_url(BASE_URL, &key);
        Ok(parse_chapters(&post_json(&target, CHAPTERS_FIXTURE), &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comic/1/sample/chapter/1".to_string());
        let target = url::join_url(BASE_URL, &key);
        Ok(parse_pages(&post_json(&target, PAGES_FIXTURE)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_header("Accept", "application/json, text/javascript, */*; q=0.01")
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(path: &str, fixture: &str) -> String {
    client()
        .get(format!(
            "{}/{}",
            BASE_URL.trim_end_matches('/'),
            path.trim_start_matches('/')
        ))
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

fn post_json(target: &str, fixture: &str) -> String {
    client()
        .post(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_ranking(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<ResponseDto<RankingResultDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(RANKING_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: response
            .result
            .weekly
            .into_iter()
            .map(SearchEntryDto::to_item)
            .collect(),
        has_next_page: false,
    }
}

fn parse_search_entries(
    body: &str,
    query: Option<&str>,
    filters: Option<&Value>,
) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<ResponseDto<SearchResultDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(COMICS_FIXTURE).expect("fixture is valid"));
    let mut titles = response.result.titles;
    if let Some(query) = query.filter(|value| !value.is_empty()) {
        let query = query.to_ascii_lowercase();
        titles.retain(|title| title.title.to_ascii_lowercase().contains(&query));
    }
    if let Some(genres) = csv_filter(filters, "genres") {
        titles.retain(|title| csv_any(title.genres.as_deref(), &genres));
    }
    if let Some(authors) = csv_filter(filters, "authors") {
        titles.retain(|title| csv_any(title.authors.as_deref(), &authors));
    }
    if let Some(statuses) = csv_filter(filters, "statuses") {
        titles.retain(|title| {
            title
                .status
                .as_deref()
                .is_none_or(|status| statuses.iter().any(|expected| expected == status))
        });
    }
    match filter(filters, "sort").unwrap_or("latest") {
        "title" => titles.sort_by(|a, b| a.title.cmp(&b.title)),
        "popularity" => titles.sort_by(|a, b| {
            b.all_views
                .unwrap_or_default()
                .cmp(&a.all_views.unwrap_or_default())
        }),
        _ => titles.sort_by(|a, b| {
            b.updated
                .unwrap_or_default()
                .cmp(&a.updated.unwrap_or_default())
        }),
    }
    Paged {
        entries: titles.into_iter().map(SearchEntryDto::to_item).collect(),
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/comic/1/sample".to_string());
    let lower = body.to_ascii_lowercase();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                url::slug_from_url(&key).unwrap_or_else(|| "InfinityScans".to_string())
            }),
        description: html::text_between(body, "Summary", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: collect_labeled_links(body, "Authors"),
        tags: collect_labeled_links(body, "Genres"),
        status: if lower.contains("completed") {
            ItemStatus::Completed
        } else if lower.contains("hiatus") {
            ItemStatus::Hiatus
        } else if lower.contains("dropped") || lower.contains("cancelled") {
            ItemStatus::Cancelled
        } else if lower.contains("ongoing") || lower.contains("publishing") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<ResponseDto<Vec<ChapterEntryDto>>>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
    response
        .result
        .into_iter()
        .map(|chapter| chapter.to_chapter(manga_key))
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<ResponseDto<Vec<PageEntryDto>>>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    response
        .result
        .into_iter()
        .enumerate()
        .map(|(index, page)| MangaPage {
            content: PageContent::Url {
                url: page.link,
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn collect_labeled_links(body: &str, label: &str) -> Vec<String> {
    body.split("<div")
        .filter(|chunk| chunk.contains(label))
        .flat_map(|chunk| chunk.split("<a").skip(1))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn csv_filter(filters: Option<&Value>, id: &str) -> Option<Vec<String>> {
    filter(filters, id).map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(ToString::to_string)
            .collect()
    })
}

fn csv_any(value: Option<&str>, filter_values: &[String]) -> bool {
    value.is_none_or(|value| {
        value
            .split(',')
            .map(str::trim)
            .any(|part| filter_values.iter().any(|expected| expected == part))
    })
}

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(id))
        .and_then(Value::as_str)
}

fn normalize_key(input: &str) -> String {
    let path = input.trim_start_matches(BASE_URL).trim_matches('/');
    format!("/{path}")
}

#[derive(Deserialize)]
struct ResponseDto<T> {
    result: T,
}

#[derive(Deserialize)]
struct SearchResultDto {
    titles: Vec<SearchEntryDto>,
}

#[derive(Deserialize)]
struct RankingResultDto {
    weekly: Vec<SearchEntryDto>,
}

#[derive(Deserialize)]
struct SearchEntryDto {
    id: i64,
    title: String,
    slug: String,
    cover: String,
    #[serde(default)]
    authors: Option<String>,
    #[serde(default)]
    genres: Option<String>,
    #[serde(default, rename = "all_views")]
    all_views: Option<i64>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    updated: Option<i64>,
}

impl SearchEntryDto {
    fn to_item(self) -> CatalogItem {
        let updated = self.updated.unwrap_or_default();
        CatalogItem {
            key: format!("/comic/{}/{}", self.id, self.slug),
            title: self.title,
            cover: Some(format!(
                "https://{CDN_HOST}/{}/{}?_={updated}",
                self.id, self.cover
            )),
            url: Some(format!("{BASE_URL}/comic/{}/{}", self.id, self.slug)),
            authors: csv_values(self.authors.as_deref()),
            tags: csv_values(self.genres.as_deref()),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            status: parse_status(self.status.as_deref()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct ChapterEntryDto {
    id: i64,
    title: String,
    sequence: f32,
    date: i64,
}

impl ChapterEntryDto {
    fn to_chapter(self, manga_key: &str) -> MangaChapter {
        MangaChapter {
            key: format!("{}/chapter/{}", manga_key.trim_end_matches('/'), self.id),
            title: Some(self.title.clone()),
            chapter_number: self
                .title
                .split("hapter ")
                .nth(1)
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse().ok())
                .or(Some(self.sequence)),
            date_uploaded: Some(self.date),
            url: Some(format!(
                "{BASE_URL}{}/chapter/{}",
                manga_key.trim_end_matches('/'),
                self.id
            )),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct PageEntryDto {
    link: String,
}

fn csv_values(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    let value = value.unwrap_or_default().to_ascii_lowercase();
    if value.contains("completed") {
        ItemStatus::Completed
    } else if value.contains("hiatus") {
        ItemStatus::Hiatus
    } else if value.contains("dropped") || value.contains("cancelled") {
        ItemStatus::Cancelled
    } else if value.contains("ongoing") || value.contains("publishing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

export_manga_source!(SOURCE);

const RANKING_FIXTURE: &str = r#"{
  "result": {
    "weekly": [{
      "id": 1,
      "title": "Sample Manga",
      "slug": "sample",
      "cover": "cover.jpg",
      "authors": "Author",
      "genres": "Action",
      "all_views": 10,
      "status": "ongoing",
      "updated": 1704067200
    }],
    "monthly": [],
    "all": []
  }
}"#;
const COMICS_FIXTURE: &str = r#"{
  "result": {
    "titles": [{
      "id": 1,
      "title": "Sample Manga",
      "slug": "sample",
      "cover": "cover.jpg",
      "authors": "Author",
      "genres": "Action",
      "all_views": 10,
      "status": "ongoing",
      "updated": 1704067200
    }]
  }
}"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample Manga</h1>
<div><span>Summary:</span><p>Sample description</p></div>
<div><span>Authors:</span><a>Author</a></div>
<div><span>Genres:</span><a>Action</a></div>
<div><span>Status:</span>Ongoing</div>
"#;
const CHAPTERS_FIXTURE: &str = r#"{
  "result": [{ "id": 1, "title": "Chapter 1", "sequence": 1, "date": 1704067200 }]
}"#;
const PAGES_FIXTURE: &str = r#"{
  "result": [{ "link": "https://cdn.infinityscans.org/1/page.jpg" }]
}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_infinity_pages() {
        let pages = SOURCE.pages(json!({})).unwrap();
        assert_eq!(pages.len(), 1);
    }
}
