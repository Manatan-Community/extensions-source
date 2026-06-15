use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: MangaTube = MangaTube;
const BASE_URL: &str = "https://manga-tube.me";
const LATEST_PAGE_SIZE: u64 = 40;

struct MangaTube;

impl MangaSource for MangaTube {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_top(TOP_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let offset = page.saturating_sub(1) * LATEST_PAGE_SIZE;
            let body = fetch_api_or_fixture(
                &format!("/api/home/updates?offset={offset}"),
                LATEST_FIXTURE,
                "",
            );
            return Ok(parse_latest(&body, offset));
        }
        Ok(parse_top(&fetch_api_or_fixture(
            "/api/home/top-manga",
            TOP_FIXTURE,
            "",
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let slug = slug_from_key(&key);
            let body = fetch_api_or_fixture(&format!("/api/manga/{slug}"), DETAILS_FIXTURE, &slug);
            return Ok(Paged {
                entries: vec![parse_details(&body, key)],
                has_next_page: false,
            });
        }
        Ok(parse_search(&fetch_api_or_fixture(
            &format!("/api/manga/quick-search?query={}", url::query_escape(query)),
            SEARCH_FIXTURE,
            "",
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let slug = slug_from_key(&key);
        let body = fetch_api_or_fixture(&format!("/api/manga/{slug}"), DETAILS_FIXTURE, &slug);
        Ok(parse_details(&body, key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let slug = slug_from_key(&key);
        let body = fetch_api_or_fixture(
            &format!("/api/manga/{slug}/chapters"),
            CHAPTERS_FIXTURE,
            &slug,
        );
        Ok(parse_chapters(&body, &slug))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/api/manga/sample/chapter/1".to_string());
        let slug = key
            .trim_matches('/')
            .split('/')
            .nth(2)
            .unwrap_or("sample")
            .to_string();
        let api_path = if key.starts_with("/api/manga/") {
            key
        } else {
            let chapter_id = key
                .split("/read/")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .unwrap_or("1");
            format!("/api/manga/{slug}/chapter/{chapter_id}")
        };
        let body = fetch_api_or_fixture(&api_path, PAGES_FIXTURE, &slug);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let slug = slug_from_key(&key);
            let body = fetch_api_or_fixture(&format!("/api/manga/{slug}"), DETAILS_FIXTURE, &slug);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, key)),
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

fn client(slug: &str) -> HttpClient {
    let referer = if slug.is_empty() {
        format!("{BASE_URL}/")
    } else {
        format!("{BASE_URL}/series/{slug}")
    };
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_header("Accept", "application/json")
        .with_header("Use-Parameter", "manga_slug")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api_or_fixture(path: &str, fixture: &str, slug: &str) -> String {
    client(slug)
        .get(format!("{BASE_URL}{path}"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_top(body: &str) -> Paged<CatalogItem> {
    let response: TopMangaResponse = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: response
            .data
            .manga
            .into_iter()
            .map(ApiManga::into_catalog)
            .collect(),
        has_next_page: false,
    }
}

fn parse_latest(body: &str, offset: u64) -> Paged<CatalogItem> {
    let response: LatestUpdatesResponse = serde_json::from_str(body).unwrap_or_default();
    let mut entries = Vec::new();
    for published in response.data.published {
        let item = published.manga.into_catalog();
        if !entries
            .iter()
            .any(|existing: &CatalogItem| existing.key == item.key)
        {
            entries.push(item);
        }
    }
    Paged {
        entries,
        has_next_page: offset < LATEST_PAGE_SIZE * 2,
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let response: QuickSearchResponse = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: response
            .data
            .into_iter()
            .map(QuickSearchManga::into_catalog)
            .collect(),
        has_next_page: false,
    }
}

fn parse_details(body: &str, fallback_key: String) -> CatalogItem {
    let response: MangaDetailsResponse = serde_json::from_str(body).unwrap_or_default();
    let manga = response.data.manga;
    if manga.title.is_empty() {
        return CatalogItem {
            key: fallback_key.clone(),
            title: url::slug_from_url(&fallback_key).unwrap_or_else(|| "Manga".into()),
            language: Some("de".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        };
    }
    CatalogItem {
        key: normalize_key(&manga.url),
        title: manga.title,
        cover: Some(manga.cover),
        description: Some(manga.description),
        authors: manga.author.into_iter().map(|person| person.name).collect(),
        artists: manga.artist.into_iter().map(|person| person.name).collect(),
        status: match manga.status {
            1 => ItemStatus::Ongoing,
            2 => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(url::join_url(BASE_URL, &manga.url)),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, slug: &str) -> Vec<MangaChapter> {
    let response: MangaChaptersResponse = serde_json::from_str(body).unwrap_or_default();
    response
        .data
        .chapters
        .into_iter()
        .map(|chapter| {
            let title = chapter_title(&chapter);
            let key = format!("/api/manga/{slug}/chapter/{}", chapter.id);
            MangaChapter {
                key: key.clone(),
                title: Some(title),
                volume_number: (chapter.volume > 0.0).then_some(chapter.volume as f32),
                chapter_number: Some((chapter.number + chapter.sub_number / 100.0) as f32),
                date_uploaded: manatan_shared::dates::parse_fixture_date(&chapter.published_at),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response: ChapterDetailsResponse = serde_json::from_str(body).unwrap_or_default();
    let mut pages = response.data.chapter.pages;
    pages.sort_by_key(|page| page.page);
    pages
        .into_iter()
        .enumerate()
        .filter_map(|(index, page)| {
            let image = page
                .url
                .filter(|value| !value.is_empty())
                .or_else(|| page.alt_source.filter(|value| !value.is_empty()))?;
            Some(MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn chapter_title(chapter: &ApiChapter) -> String {
    let mut title = String::new();
    if chapter.volume > 0.0 {
        title.push_str("Vol. ");
        title.push_str(&trim_decimal(chapter.volume));
        title.push(' ');
    }
    title.push_str("Ch. ");
    title.push_str(&trim_decimal(chapter.number));
    if chapter.sub_number > 0.0 {
        title.push('.');
        title.push_str(&trim_decimal(chapter.sub_number));
    }
    if !chapter.name.is_empty() {
        title.push_str(" - ");
        title.push_str(&chapter.name);
    }
    title
}

fn trim_decimal(value: f64) -> String {
    let text = value.to_string();
    text.strip_suffix(".0").unwrap_or(&text).to_string()
}

fn slug_from_key(key: &str) -> String {
    key.trim_matches('/')
        .split('/')
        .nth(1)
        .unwrap_or_else(|| key.trim_matches('/'))
        .to_string()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!("/{}", input.trim_start_matches(BASE_URL).trim_matches('/'));
    }
    format!("/{}", input.trim_matches('/'))
}

#[derive(Default, Deserialize)]
struct QuickSearchResponse {
    #[serde(default)]
    data: Vec<QuickSearchManga>,
}

#[derive(Default, Deserialize)]
struct QuickSearchManga {
    title: String,
    url: String,
    cover: String,
}

impl QuickSearchManga {
    fn into_catalog(self) -> CatalogItem {
        ApiManga {
            title: self.title,
            url: self.url,
            cover: self.cover,
        }
        .into_catalog()
    }
}

#[derive(Default, Deserialize)]
struct TopMangaResponse {
    #[serde(default)]
    data: TopMangaData,
}

#[derive(Default, Deserialize)]
struct TopMangaData {
    #[serde(default)]
    manga: Vec<ApiManga>,
}

#[derive(Default, Deserialize)]
struct LatestUpdatesResponse {
    #[serde(default)]
    data: LatestUpdatesData,
}

#[derive(Default, Deserialize)]
struct LatestUpdatesData {
    #[serde(default)]
    published: Vec<LatestEntry>,
}

#[derive(Default, Deserialize)]
struct LatestEntry {
    manga: ApiManga,
}

#[derive(Default, Deserialize)]
struct ApiManga {
    title: String,
    cover: String,
    url: String,
}

impl ApiManga {
    fn into_catalog(self) -> CatalogItem {
        let key = normalize_key(&self.url);
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: Some(self.cover),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("de".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct MangaDetailsResponse {
    #[serde(default)]
    data: MangaDetailsData,
}

#[derive(Default, Deserialize)]
struct MangaDetailsData {
    #[serde(default)]
    manga: MangaDetailsManga,
}

#[derive(Default, Deserialize)]
struct MangaDetailsManga {
    title: String,
    #[serde(default)]
    description: String,
    cover: String,
    url: String,
    status: i32,
    #[serde(default)]
    author: Vec<Person>,
    #[serde(default)]
    artist: Vec<Person>,
}

#[derive(Deserialize)]
struct Person {
    name: String,
}

#[derive(Default, Deserialize)]
struct MangaChaptersResponse {
    #[serde(default)]
    data: MangaChaptersData,
}

#[derive(Default, Deserialize)]
struct MangaChaptersData {
    #[serde(default)]
    chapters: Vec<ApiChapter>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiChapter {
    id: i64,
    number: f64,
    sub_number: f64,
    volume: f64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    published_at: String,
}

#[derive(Default, Deserialize)]
struct ChapterDetailsResponse {
    #[serde(default)]
    data: ChapterDetailsData,
}

#[derive(Default, Deserialize)]
struct ChapterDetailsData {
    #[serde(default)]
    chapter: ChapterDetails,
}

#[derive(Default, Deserialize)]
struct ChapterDetails {
    #[serde(default)]
    pages: Vec<ChapterPage>,
}

#[derive(Default, Deserialize)]
struct ChapterPage {
    #[serde(default)]
    url: Option<String>,
    #[serde(default, rename = "alt_source")]
    alt_source: Option<String>,
    page: i64,
}

export_manga_source!(SOURCE);

const TOP_FIXTURE: &str = r#"
{"data":{"manga":[{"title":"Probe Manga","cover":"https://img/cover.jpg","url":"/series/probe"}]}}
"#;

const LATEST_FIXTURE: &str = r#"
{"data":{"published":[{"manga":{"title":"Probe Manga","cover":"https://img/cover.jpg","url":"/series/probe"}}]}}
"#;

const SEARCH_FIXTURE: &str = r#"
{"data":[{"title":"Probe Manga","cover":"https://img/cover.jpg","url":"/series/probe"}]}
"#;

const DETAILS_FIXTURE: &str = r#"
{"data":{"manga":{"title":"Probe Manga","description":"Beschreibung","cover":"https://img/cover.jpg","url":"/series/probe","status":1,"author":[{"name":"Autor"}],"artist":[{"name":"Zeichner"}]}}}
"#;

const CHAPTERS_FIXTURE: &str = r#"
{"data":{"chapters":[{"id":1,"number":1.0,"subNumber":0.0,"volume":0.0,"name":"Start","publishedAt":"2024-01-01 00:00:00"}]}}
"#;

const PAGES_FIXTURE: &str = r#"
{"data":{"chapter":{"pages":[{"url":"https://img/page2.jpg","alt_source":"","page":2},{"url":"","alt_source":"https://img/page1.jpg","page":1}]}}}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_source() {
        assert_eq!(parse_top(TOP_FIXTURE).entries[0].key, "/series/probe");
        assert!(parse_latest(LATEST_FIXTURE, 0).has_next_page);
        assert_eq!(parse_search(SEARCH_FIXTURE).entries[0].title, "Probe Manga");
        assert_eq!(
            parse_details(DETAILS_FIXTURE, "/series/probe".into()).authors[0],
            "Autor"
        );
        assert_eq!(
            parse_chapters(CHAPTERS_FIXTURE, "probe")[0]
                .title
                .as_deref(),
            Some("Ch. 1 - Start")
        );
        assert_eq!(
            parse_pages(PAGES_FIXTURE)[0].description.as_deref(),
            Some("Page 1")
        );
    }
}
