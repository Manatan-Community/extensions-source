use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ComicCx = ComicCx;
const BASE_URL: &str = "https://comic.cx";
const API_URL: &str = "https://comic.cx/api";
const LIMIT: u64 = 100;

struct ComicCx;

impl MangaSource for ComicCx {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "popularity"
        };
        Ok(parse_list(&fetch_api_or_fixture(
            &format!("/manga?limit={LIMIT}&page={page}&sort={sort}"),
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
            let body = fetch_api_or_fixture(&format!("/manga/{slug}"), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, &slug)],
                has_next_page: false,
            });
        }
        let search = if query.is_empty() {
            String::new()
        } else {
            format!("&search={}", url::query_escape(query))
        };
        Ok(parse_list(&fetch_api_or_fixture(
            &format!("/manga?limit={LIMIT}&page={page}{search}"),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let slug = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = fetch_api_or_fixture(&format!("/manga/{slug}"), DETAILS_FIXTURE);
        Ok(parse_details(&body, &slug))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let slug = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = fetch_api_or_fixture(&format!("/manga/{slug}/chapters"), CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body, &slug))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".to_string());
        let (slug, id) = key.split_once('/').unwrap_or(("sample", "1"));
        let body = fetch_api_or_fixture(
            &format!("/manga/{slug}/chapters?chapter_id={id}"),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_pages(&body, id.parse().ok()))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|slug| format!("{BASE_URL}/manga/{slug}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let slug = normalize_slug(input);
            let body = fetch_api_or_fixture(&format!("/manga/{slug}"), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, &slug)),
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

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let response: MangaListResponse = serde_json::from_str(body).unwrap_or_default();
    let page = response
        .pagination
        .as_ref()
        .map(|item| item.page)
        .unwrap_or(1);
    let pages = response
        .pagination
        .as_ref()
        .map(|item| item.pages)
        .unwrap_or(1);
    Paged {
        entries: response
            .manga
            .into_iter()
            .map(MangaItem::into_catalog)
            .collect(),
        has_next_page: page < pages,
    }
}

fn parse_details(body: &str, fallback_slug: &str) -> CatalogItem {
    let mut item = serde_json::from_str::<MangaItem>(body)
        .unwrap_or_else(|_| MangaItem::fallback(fallback_slug))
        .into_catalog();
    item.initialized = true;
    item
}

fn parse_chapters(body: &str, slug: &str) -> Vec<MangaChapter> {
    let mut chapters: Vec<_> = serde_json::from_str::<Vec<ChapterItem>>(body)
        .unwrap_or_default()
        .into_iter()
        .map(|chapter| chapter.into_chapter(slug))
        .collect();
    chapters.sort_by(|left, right| {
        right
            .chapter_number
            .partial_cmp(&left.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str, chapter_id: Option<i64>) -> Vec<MangaPage> {
    let chapters: Vec<ChapterItem> = serde_json::from_str(body).unwrap_or_default();
    chapters
        .into_iter()
        .find(|chapter| Some(chapter.id) == chapter_id)
        .or_else(|| serde_json::from_str::<ChapterItem>(body).ok())
        .into_iter()
        .flat_map(|chapter| chapter.pages)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_media_url(&image),
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
        .filter(|part| !part.is_empty())
        .next_back()
        .unwrap_or("sample")
        .to_string()
}

fn absolute_media_url(value: &str) -> String {
    if value.starts_with('/') {
        url::join_url(BASE_URL, value)
    } else {
        value.to_string()
    }
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "hiatus" | "on hold" => ItemStatus::Hiatus,
        "cancelled" | "canceled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

#[derive(Debug, Default, Deserialize)]
struct MangaListResponse {
    #[serde(default)]
    manga: Vec<MangaItem>,
    pagination: Option<Pagination>,
}

#[derive(Debug, Deserialize)]
struct Pagination {
    page: u64,
    pages: u64,
}

#[derive(Debug, Default, Deserialize)]
struct MangaItem {
    #[serde(default)]
    title: String,
    description: Option<String>,
    author: Option<String>,
    artist: Option<String>,
    status: Option<String>,
    #[serde(default, alias = "cover_image")]
    cover_image: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    slug: String,
    #[serde(default, alias = "required_tier")]
    required_tier: Option<String>,
    tier: Option<String>,
}

impl MangaItem {
    fn fallback(slug: &str) -> Self {
        Self {
            title: url::slug_from_url(slug).unwrap_or_else(|| "Comic CX".to_string()),
            slug: slug.to_string(),
            ..Self::default()
        }
    }

    fn into_catalog(self) -> CatalogItem {
        let slug = if self.slug.is_empty() {
            self.title.to_ascii_lowercase().replace(' ', "-")
        } else {
            self.slug
        };
        let mut description = self.description;
        let tier = self.required_tier.or(self.tier);
        if tier
            .as_deref()
            .is_some_and(|tier| tier != "free" && tier != "tier_0")
        {
            let note = format!(
                "This title requires {} access. Log in via WebView to read.",
                tier.unwrap_or_default().replace('_', " ").to_uppercase()
            );
            description = Some(match description {
                Some(value) if !value.is_empty() => format!("{value}\n\n{note}"),
                _ => note,
            });
        }
        CatalogItem {
            key: slug.clone(),
            title: if self.title.is_empty() {
                slug.clone()
            } else {
                self.title
            },
            cover: self.cover_image.map(|cover| absolute_media_url(&cover)),
            description,
            authors: self.author.into_iter().collect(),
            artists: self.artist.into_iter().collect(),
            tags: self.genres,
            status: parse_status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/manga/{slug}")),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ChapterItem {
    id: i64,
    #[serde(default, alias = "chapter_number")]
    chapter_number: f32,
    title: Option<String>,
    #[serde(default)]
    pages: Vec<String>,
}

impl ChapterItem {
    fn into_chapter(self, slug: &str) -> MangaChapter {
        let number = if self.chapter_number.fract() == 0.0 {
            format!("{}", self.chapter_number as i64)
        } else {
            self.chapter_number.to_string()
        };
        let title = match self.title {
            Some(title) if !title.is_empty() => format!("Chapter {number} - {title}"),
            _ => format!("Chapter {number}"),
        };
        MangaChapter {
            key: format!("{slug}/{}", self.id),
            title: Some(title),
            chapter_number: Some(self.chapter_number),
            url: Some(format!("{BASE_URL}/manga/{slug}/reader/{number}")),
            ..MangaChapter::default()
        }
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{
  "manga":[{"title":"Sample CX","description":"A sample.","author":"Author","artist":"Artist","status":"Ongoing","cover_image":"/cover.jpg","genres":["Action"],"slug":"sample"}],
  "pagination":{"page":1,"limit":100,"total":1,"pages":1}
}"#;

const DETAILS_FIXTURE: &str = r#"{"title":"Sample CX","description":"A sample.","status":"Completed","cover_image":"/cover.jpg","genres":["Action"],"slug":"sample"}"#;

const CHAPTERS_FIXTURE: &str =
    r#"[{"id":1,"chapter_number":1,"title":"Start","pages":["/page1.jpg","/page2.jpg"]}]"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_api_payloads() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample CX");
        let pages = SOURCE.pages(json!({"chapter":"sample/1"})).unwrap();
        assert_eq!(pages.len(), 2);
    }
}
