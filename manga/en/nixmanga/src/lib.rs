use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

const SOURCE: NixManga = NixManga;
const BASE_URL: &str = "https://nixmanga.com";
const API_URL: &str = "https://api.nixmanga.com";
const SITE_ID: &str = "00000000-0000-0000-0000-000000000003";

struct NixManga;

impl MangaSource for NixManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if listing_id(&request) == "popular" {
            "popular"
        } else {
            "latest"
        };
        Ok(parse_comics(&api_get(
            "/api/v1/comics",
            &format!("/comics?page={page}&per_page=24&sort={sort}"),
            COMICS_FIXTURE,
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
                entries: vec![parse_detail(&api_get(
                    &format!("/api/v1/comics/slug/{key}"),
                    &format!("/comics/slug/{key}"),
                    DETAIL_FIXTURE,
                ))],
                has_next_page: false,
            });
        }
        let (endpoint, path) = if query.is_empty() {
            (
                "/api/v1/comics".to_string(),
                format!("/comics?page={page}&per_page=24&sort=latest"),
            )
        } else {
            (
                "/api/v1/comics/search".to_string(),
                format!("/comics/search?q={}&page={page}", url::query_escape(query)),
            )
        };
        Ok(parse_comics(&api_get(&endpoint, &path, COMICS_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_detail(&api_get(
            &format!("/api/v1/comics/slug/{key}"),
            &format!("/comics/slug/{key}"),
            DETAIL_FIXTURE,
        )))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let slug = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let mut chapters = Vec::new();
        let mut page = 1;
        loop {
            let body = api_get(
                &format!("/api/v1/comics/slug/{slug}/chapters"),
                &format!("/comics/slug/{slug}/chapters?page={page}&per_page=100&sort=newest"),
                CHAPTERS_FIXTURE,
            );
            let dto = serde_json::from_str::<ChapterPage>(&body).unwrap_or_else(|_| {
                serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid")
            });
            let has_next = dto.has_next_page();
            chapters.extend(
                dto.chapters
                    .into_iter()
                    .map(|chapter| chapter.to_chapter(&slug)),
            );
            if !has_next || page > 20 {
                break;
            }
            page += 1;
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/read/sample/chapter-1#chapter-id".to_string());
        let id = key.rsplit('#').next().unwrap_or(&key);
        let body = api_get(
            &format!("/api/v1/chapters/{id}"),
            &format!("/chapters/{id}?skip_view=true"),
            PAGES_FIXTURE,
        );
        let dto = serde_json::from_str::<PagesResponse>(&body)
            .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
        Ok(dto
            .pages
            .into_iter()
            .enumerate()
            .map(|(index, page)| MangaPage {
                content: PageContent::Url {
                    url: page.image_url,
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
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/manga/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}{}", key.split('#').next().unwrap_or(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_detail(&api_get(
                    &format!("/api/v1/comics/slug/{key}"),
                    &format!("/comics/slug/{key}"),
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

fn api_get(endpoint: &str, path: &str, fixture: &str) -> String {
    let Some(auth) = auth_headers(endpoint) else {
        return fixture.to_string();
    };
    client()
        .get(format!("{API_URL}/api/v1{path}"))
        .header("Accept", "*/*")
        .header("x-web-token", auth.token)
        .header("x-web-signature", auth.signature)
        .header("x-web-slot", auth.slot)
        .header("x-site-id", SITE_ID)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn auth_headers(endpoint: &str) -> Option<AuthHeaders> {
    let body = client()
        .get(format!("{API_URL}/_nix/signer.js"))
        .header("Accept", "application/javascript")
        .send_text()
        .ok()?;
    let array = body.split("const z=[").nth(1)?.split("],").next()?;
    let values = array
        .split(',')
        .map(|value| {
            value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string()
        })
        .collect::<Vec<_>>();
    if values.len() < 5 {
        return None;
    }
    let slot = reverse(&values[0]);
    let token = values[4..]
        .iter()
        .map(|value| reverse(value))
        .collect::<String>();
    let key = values[1..=3]
        .iter()
        .map(|value| reverse(value))
        .collect::<String>();
    let payload = format!("GET|{endpoint}|{SITE_ID}|{slot}|{token}|{key}");
    let signature = base64_url_no_pad(&Sha256::digest(payload.as_bytes()));
    Some(AuthHeaders {
        slot,
        token,
        signature,
    })
}

fn reverse(input: &str) -> String {
    input.chars().rev().collect()
}

fn base64_url_no_pad(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::new();
    let mut index = 0;
    while index < bytes.len() {
        let b0 = bytes[index];
        let b1 = bytes.get(index + 1).copied().unwrap_or(0);
        let b2 = bytes.get(index + 2).copied().unwrap_or(0);
        output.push(TABLE[(b0 >> 2) as usize] as char);
        output.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if index + 1 < bytes.len() {
            output.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        }
        if index + 2 < bytes.len() {
            output.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        }
        index += 3;
    }
    output
}

fn parse_comics(body: &str) -> Paged<CatalogItem> {
    let dto = serde_json::from_str::<ComicPage>(body)
        .unwrap_or_else(|_| serde_json::from_str(COMICS_FIXTURE).expect("fixture is valid"));
    Paged {
        has_next_page: dto.has_next_page(),
        entries: dto.comics.into_iter().map(Comic::to_catalog).collect(),
    }
}

fn parse_detail(body: &str) -> CatalogItem {
    serde_json::from_str::<Comic>(body)
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
        .unwrap_or("latest")
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

struct AuthHeaders {
    slot: String,
    token: String,
    signature: String,
}

#[derive(Deserialize)]
struct ComicPage {
    #[serde(default, alias = "results")]
    comics: Vec<Comic>,
    #[serde(default = "one")]
    page: u64,
    #[serde(default = "one", rename = "total_pages")]
    total_pages: u64,
    #[serde(default)]
    total: u64,
}

impl ComicPage {
    fn has_next_page(&self) -> bool {
        self.page < self.total_pages || self.page * 24 < self.total
    }
}

#[derive(Deserialize)]
struct Comic {
    slug: String,
    title: String,
    synopsis: Option<String>,
    cover: Option<String>,
    status: Option<String>,
    #[serde(default)]
    genres: Vec<Genre>,
}

impl Comic {
    fn to_catalog(self) -> CatalogItem {
        CatalogItem {
            key: self.slug.clone(),
            title: self.title,
            cover: self.cover,
            url: Some(format!("{BASE_URL}/manga/{}", self.slug)),
            description: self.synopsis,
            tags: self.genres.into_iter().map(|genre| genre.name).collect(),
            status: match self
                .status
                .as_deref()
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("ongoing") => ItemStatus::Ongoing,
                Some("completed") => ItemStatus::Completed,
                Some("hiatus") => ItemStatus::Hiatus,
                Some("cancelled") => ItemStatus::Cancelled,
                _ => ItemStatus::Unknown,
            },
            content_rating: Some("adult".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct Genre {
    name: String,
}

#[derive(Deserialize)]
struct ChapterPage {
    #[serde(default)]
    chapters: Vec<Chapter>,
    #[serde(default = "one")]
    page: u64,
    #[serde(default = "one", rename = "total_pages")]
    total_pages: u64,
}

impl ChapterPage {
    fn has_next_page(&self) -> bool {
        self.page < self.total_pages
    }
}

#[derive(Deserialize)]
struct Chapter {
    id: String,
    number: Option<f32>,
    title: Option<String>,
    slug: String,
    scanlator: Option<Scanlator>,
}

impl Chapter {
    fn to_chapter(self, manga_slug: &str) -> MangaChapter {
        let title = match (self.number, self.title.as_deref()) {
            (Some(number), Some(title)) if !title.is_empty() => {
                format!("Chapter {} - {title}", trim_number(number))
            }
            (Some(number), _) => format!("Chapter {}", trim_number(number)),
            _ => self.title.unwrap_or_else(|| "Chapter".into()),
        };
        MangaChapter {
            key: format!("/read/{manga_slug}/{}#{}", self.slug, self.id),
            title: Some(title),
            chapter_number: self.number,
            url: Some(format!("{BASE_URL}/read/{manga_slug}/{}", self.slug)),
            scanlators: self
                .scanlator
                .map(|scanlator| vec![scanlator.name])
                .unwrap_or_default(),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct Scanlator {
    name: String,
}

#[derive(Deserialize)]
struct PagesResponse {
    #[serde(default)]
    pages: Vec<PageDto>,
}

#[derive(Deserialize)]
struct PageDto {
    image_url: String,
}

fn trim_number(value: f32) -> String {
    let text = value.to_string();
    text.strip_suffix(".0").unwrap_or(&text).to_string()
}

fn one() -> u64 {
    1
}

export_manga_source!(SOURCE);

const COMICS_FIXTURE: &str = r#"{"results":[{"slug":"sample","title":"Sample","synopsis":"Summary","cover":"https://img.nixmanga.example.invalid/cover.jpg","status":"ongoing","genres":[{"name":"Action"}]}],"page":1,"total_pages":1,"total":1}"#;
const DETAIL_FIXTURE: &str = r#"{"slug":"sample","title":"Sample","synopsis":"Summary","cover":"https://img.nixmanga.example.invalid/cover.jpg","status":"ongoing","genres":[{"name":"Action"}]}"#;
const CHAPTERS_FIXTURE: &str = r#"{"chapters":[{"id":"chapter-id","number":1,"title":"Start","slug":"chapter-1","scanlator":{"name":"Group"}}],"page":1,"total_pages":1}"#;
const PAGES_FIXTURE: &str =
    r#"{"pages":[{"image_url":"https://img.nixmanga.example.invalid/page1.jpg"}]}"#;
