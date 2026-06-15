use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: XoManga = XoManga;
const BASE_URL: &str = "https://www.xomanga.site";

struct XoManga;

impl MangaSource for XoManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let index: IndexResponse = fetch_json("/index.json", INDEX_FIXTURE);
        let entries = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            index.slider
        } else {
            index.latest
        }
        .into_iter()
        .map(MangaDto::to_item)
        .collect();
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let index: IndexResponse = fetch_json("/index.json", INDEX_FIXTURE);
        Ok(Paged {
            entries: index
                .latest
                .into_iter()
                .filter(|item| query.is_empty() || item.title.to_ascii_lowercase().contains(&query))
                .map(MangaDto::to_item)
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let details: DetailsResponse =
            fetch_json(&format!("/manga/{key}/details.json"), DETAILS_FIXTURE);
        Ok(details.to_item(key, true))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let chapters: ChapterResponse =
            fetch_json(&format!("/manga/{key}/details.json"), CHAPTERS_FIXTURE);
        Ok(chapters
            .chapters_list
            .into_iter()
            .map(|chapter| chapter.to_chapter(&key))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample#1".into());
        let (slug, chapter) = key.split_once('#').unwrap_or((&key, "1"));
        let images: ImageResponse = fetch_json(
            &format!("/manga/{slug}/chapters/{chapter}.json"),
            PAGES_FIXTURE,
        );
        Ok(images
            .images
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
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
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

fn fetch_json<T: for<'de> Deserialize<'de>>(path: &str, fixture: &str) -> T {
    let body = client()
        .get(&url::join_url(BASE_URL, path))
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    serde_json::from_str(&body).unwrap_or_else(|_| serde_json::from_str(fixture).unwrap())
}

#[derive(Debug, Deserialize)]
struct IndexResponse {
    #[serde(default)]
    slider: Vec<MangaDto>,
    #[serde(default)]
    latest: Vec<MangaDto>,
}

#[derive(Debug, Deserialize)]
struct MangaDto {
    title: String,
    image: Option<String>,
    link: String,
}

impl MangaDto {
    fn to_item(self) -> CatalogItem {
        let key = query_value(&self.link, "id")
            .unwrap_or_else(|| self.link.trim_matches('/').to_string());
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: self.image.map(|image| url::join_url(BASE_URL, &image)),
            url: Some(format!("{BASE_URL}/details.html?id={key}")),
            language: Some("en".into()),
            content_rating: Some("adult".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct DetailsResponse {
    title: String,
    description: Option<String>,
    cover: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    status: Option<String>,
}

impl DetailsResponse {
    fn to_item(self, key: String, initialized: bool) -> CatalogItem {
        CatalogItem {
            key: key.clone(),
            title: self.title,
            description: self.description,
            cover: self.cover.map(|cover| url::join_url(BASE_URL, &cover)),
            tags: self.tags,
            status: match self.status.as_deref() {
                Some("ongoing") => ItemStatus::Ongoing,
                Some("completed") => ItemStatus::Completed,
                Some("hiatus") => ItemStatus::Hiatus,
                Some("cancelled") => ItemStatus::Cancelled,
                _ => ItemStatus::Unknown,
            },
            url: Some(format!("{BASE_URL}/details.html?id={key}")),
            language: Some("en".into()),
            content_rating: Some("adult".into()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChapterResponse {
    #[serde(rename = "chapters_list", default)]
    chapters_list: Vec<ChapterDto>,
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    chapter: f32,
    link: String,
    #[allow(dead_code)]
    date: Option<String>,
}

impl ChapterDto {
    fn to_chapter(self, fallback_slug: &str) -> MangaChapter {
        let slug = query_value(&self.link, "id").unwrap_or_else(|| fallback_slug.to_string());
        let chapter = query_value(&self.link, "ch").unwrap_or_else(|| self.chapter.to_string());
        let title_num = self.chapter.to_string().trim_end_matches(".0").to_string();
        MangaChapter {
            key: format!("{slug}#{chapter}"),
            title: Some(format!("Chapter {title_num}")),
            chapter_number: Some(self.chapter),
            url: Some(format!("{BASE_URL}/reader.html?id={slug}&ch={chapter}")),
            language: Some("en".into()),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ImageResponse {
    #[serde(default)]
    images: Vec<String>,
}

fn query_value(input: &str, key: &str) -> Option<String> {
    input
        .split('?')
        .nth(1)?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value.to_string())
}

export_manga_source!(SOURCE);

const INDEX_FIXTURE: &str = r#"{"slider":[{"title":"Sample","image":"/cover.jpg","link":"/details.html?id=sample"}],"latest":[{"title":"Sample","image":"/cover.jpg","link":"/details.html?id=sample"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"title":"Sample","description":"Summary","cover":"/cover.jpg","tags":["Drama"],"status":"ongoing"}"#;
const CHAPTERS_FIXTURE: &str =
    r#"{"chapters_list":[{"chapter":1,"link":"/reader.html?id=sample&ch=1","date":"2024-01-01"}]}"#;
const PAGES_FIXTURE: &str = r#"{"images":["/page1.jpg","/page2.jpg"]}"#;
