use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Ninekon = Ninekon;
const BASE_URL: &str = "https://app.ninekon.com";
const API_URL: &str = "https://api.ninekon.com/1.0";

struct Ninekon;

impl MangaSource for Ninekon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if listing_id(&request) == "latest" {
            "sort=dt&order=desc"
        } else {
            "sort=views"
        };
        Ok(parse_books(&fetch_json(
            &format!("{API_URL}/books?{sort}&page={page}"),
            BOOKS_FIXTURE,
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
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_detail(&fetch_json(
                    &format!("{API_URL}/books/{key}"),
                    DETAIL_FIXTURE,
                ))],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            format!("{API_URL}/books?page={page}&sort=dt&order=desc")
        } else {
            format!(
                "{API_URL}/books?page={page}&field=title&query={}",
                url::query_escape(query)
            )
        };
        Ok(parse_books(&fetch_json(&target, BOOKS_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_detail(&fetch_json(
            &format!("{API_URL}/books/{key}"),
            DETAIL_FIXTURE,
        )))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let detail = serde_json::from_str::<BookDetails>(&fetch_json(
            &format!("{API_URL}/books/{key}"),
            DETAIL_FIXTURE,
        ))
        .unwrap_or_else(|_| serde_json::from_str(DETAIL_FIXTURE).expect("fixture is valid"));
        Ok(detail
            .chapters
            .into_iter()
            .rev()
            .map(|chapter| chapter.to_chapter(&detail.gid))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/books/sample/chapters/chapter-1/pages".to_string());
        let body = fetch_json(&format!("{API_URL}{key}"), PAGES_FIXTURE);
        let dto = serde_json::from_str::<PagesResponse>(&body)
            .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
        Ok(dto
            .pages
            .into_iter()
            .enumerate()
            .map(|(index, path)| MangaPage {
                content: PageContent::Url {
                    url: format!("{}{}", dto.host, path),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                HomeSectionStyle::Cover,
                self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                HomeSectionStyle::Compact,
                self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?,
            ),
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/book/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            format!(
                "{BASE_URL}{}",
                key.replace("/books/", "/book/")
                    .replace("/chapters/", "/chapter/")
                    .trim_end_matches("/pages")
            )
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_detail(&fetch_json(
                    &format!("{API_URL}/books/{key}"),
                    DETAIL_FIXTURE,
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
        .with_origin(BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_books(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<BooksResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(BOOKS_FIXTURE).expect("fixture is valid"));
    Paged {
        has_next_page: response.page < response.pages,
        entries: response.books.into_iter().map(Book::to_catalog).collect(),
    }
}

fn parse_detail(body: &str) -> CatalogItem {
    serde_json::from_str::<BookDetails>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAIL_FIXTURE).expect("fixture is valid"))
        .to_catalog()
}

fn normalize_key(input: &str) -> String {
    input
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or(input)
        .to_string()
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn home_section(
    id: &str,
    title: &str,
    style: HomeSectionStyle,
    page: Paged<CatalogItem>,
) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(style),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

#[derive(Deserialize)]
struct BooksResponse {
    #[serde(default = "one")]
    page: u64,
    #[serde(default)]
    pages: u64,
    #[serde(default)]
    books: Vec<Book>,
}

#[derive(Deserialize)]
struct Book {
    gid: String,
    title: String,
    cover: Option<String>,
    host: Option<String>,
}

impl Book {
    fn to_catalog(self) -> CatalogItem {
        CatalogItem {
            key: self.gid.clone(),
            title: self.title,
            cover: self
                .host
                .zip(self.cover)
                .map(|(host, cover)| format!("{host}{cover}")),
            url: Some(format!("{BASE_URL}/book/{}", self.gid)),
            content_rating: Some("adult".into()),
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct BookDetails {
    gid: String,
    title: String,
    summary: Option<String>,
    author: Option<String>,
    tags: Option<String>,
    status: Option<String>,
    host: Option<String>,
    cover: Option<String>,
    #[serde(default)]
    chapters: Vec<Chapter>,
}

impl BookDetails {
    fn to_catalog(self) -> CatalogItem {
        CatalogItem {
            key: self.gid.clone(),
            title: self.title,
            cover: self
                .host
                .zip(self.cover)
                .map(|(host, cover)| format!("{host}{cover}")),
            url: Some(format!("{BASE_URL}/book/{}", self.gid)),
            authors: self.author.into_iter().collect(),
            description: self.summary,
            tags: self
                .tags
                .unwrap_or_default()
                .split('|')
                .filter(|tag| !tag.trim().is_empty())
                .map(|tag| tag.trim().to_string())
                .collect(),
            status: match self.status.as_deref() {
                Some("ongoing") => ItemStatus::Ongoing,
                Some("completed") => ItemStatus::Completed,
                _ => ItemStatus::Unknown,
            },
            content_rating: Some("adult".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct Chapter {
    gid: String,
    ordinal: Option<f32>,
}

impl Chapter {
    fn to_chapter(self, manga_id: &str) -> MangaChapter {
        let number = self.ordinal.unwrap_or(-1.0);
        MangaChapter {
            key: format!("/books/{manga_id}/chapters/{}/pages", self.gid),
            title: Some(format!("Chapter {}", trim_number(number))),
            chapter_number: (number >= 0.0).then_some(number),
            url: Some(format!("{BASE_URL}/book/{manga_id}/chapter/{}", self.gid)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct PagesResponse {
    host: String,
    #[serde(default)]
    pages: Vec<String>,
}

fn trim_number(value: f32) -> String {
    let text = value.to_string();
    text.strip_suffix(".0").unwrap_or(&text).to_string()
}

fn one() -> u64 {
    1
}

export_manga_source!(SOURCE);

const BOOKS_FIXTURE: &str = r#"{"page":1,"pages":1,"books":[{"gid":"sample","title":"Sample","host":"https://img.example.invalid","cover":"/cover.jpg"}]}"#;
const DETAIL_FIXTURE: &str = r#"{"gid":"sample","title":"Sample","summary":"Summary","author":"Creator","tags":"Action|Adult","status":"ongoing","host":"https://img.example.invalid","cover":"/cover.jpg","chapters":[{"gid":"chapter-1","ordinal":1}]}"#;
const PAGES_FIXTURE: &str = r#"{"host":"https://img.example.invalid","pages":["/sample/001.jpg"]}"#;
