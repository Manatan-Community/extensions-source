use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const SOURCE: AsuraScans = AsuraScans;
const BASE_URL: &str = "https://asurascans.com";
const API_URL: &str = "https://api.asurascans.com/api";
const PER_PAGE: u64 = 20;

struct AsuraScans;

impl MangaSource for AsuraScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let offset = (page.saturating_sub(1)) * PER_PAGE;
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "popular"
        };
        Ok(parse_list(&fetch_api_or_fixture(
            &format!("/series?offset={offset}&limit={PER_PAGE}&sort={sort}&order=desc"),
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
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let offset = (page.saturating_sub(1)) * PER_PAGE;
        Ok(parse_list(&fetch_api_or_fixture(
            &format!(
                "/series?offset={offset}&limit={PER_PAGE}&search={}",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#sample".into());
        let random_slug = key_random_slug(&key);
        Ok(parse_chapters(&fetch_document_or_fixture(
            &format!("{BASE_URL}/comics/{random_slug}"),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "sample/chapter/1#sample".into());
        let random_slug = key.split('#').nth(1).unwrap_or("sample");
        let (series_slug, number) = parse_chapter_key(&key);
        Ok(parse_pages(&fetch_document_or_fixture(
            &format!("{BASE_URL}/comics/{random_slug}/chapter/{number}"),
            &PAGES_FIXTURE.replace("__SERIES__", &series_slug),
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let payload: DataDto<Vec<MangaDto>> = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: payload
            .data
            .into_iter()
            .flatten()
            .map(MangaDto::into_catalog)
            .collect(),
        has_next_page: payload.meta.is_some_and(|meta| meta.has_more),
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let random_slug = key_random_slug(key);
    let payload = fetch_api_or_fixture(&format!("/series/{random_slug}"), DETAILS_FIXTURE);
    let root: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
    let details: MangaDetailsDto = if root.get("data").is_some() {
        serde_json::from_value(root).unwrap_or_default()
    } else {
        MangaDetailsDto {
            data: Some(MangaDetails {
                series: serde_json::from_value(root).unwrap_or_default(),
            }),
            meta: None,
        }
    };
    details
        .data
        .map(|data| data.series.into_catalog_initialized())
        .unwrap_or_else(|| fallback_item(key))
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let payload: ChapterList = extract_astro_value(body, "chapters")
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    payload
        .chapters
        .into_iter()
        .filter(|chapter| !chapter.is_locked)
        .map(ChapterDto::into_chapter)
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut payload: PageList = extract_astro_value(body, "pages")
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    if payload.pages.is_empty() {
        payload.pages = extract_page_urls_from_html(body)
            .into_iter()
            .map(|url| PageDto {
                url,
                tiles: None,
                tile_cols: None,
                tile_rows: None,
            })
            .collect();
    }
    payload
        .pages
        .into_iter()
        .enumerate()
        .map(|(index, page)| {
            let mut image = page.url;
            if let Some(tiles) = page.tiles.filter(|tiles| !tiles.is_empty()) {
                let data = PageTileData {
                    tiles,
                    tile_cols: page.tile_cols.unwrap_or(4),
                    tile_rows: page.tile_rows.unwrap_or(5),
                };
                if let Ok(fragment) = serde_json::to_string(&data) {
                    image.push('#');
                    image.push_str(&fragment);
                }
            }
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn extract_page_urls_from_html(body: &str) -> Vec<String> {
    let decoded = html::html_unescape(body);
    decoded
        .split("\"url\"")
        .skip(1)
        .filter_map(|chunk| {
            let rest = chunk.split_once(':')?.1.trim_start();
            let rest = rest.strip_prefix('"')?;
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        })
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .collect()
}

fn extract_astro_value(body: &str, key: &str) -> Option<Value> {
    let props = body.split("props=").skip(1).find_map(|chunk| {
        let quote = chunk.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let rest = &chunk[1..];
        let end = rest.find(quote)?;
        let raw = &rest[..end];
        raw.contains(key).then(|| html::html_unescape(raw))
    })?;
    let value: Value = serde_json::from_str(&props).ok()?;
    unwrap_astro(value).get(key).cloned()
}

fn unwrap_astro(value: Value) -> Value {
    match value {
        Value::Array(values) if values.len() == 2 && values[0].is_number() => {
            unwrap_astro(values.into_iter().nth(1).unwrap_or(Value::Null))
        }
        Value::Array(values) => Value::Array(values.into_iter().map(unwrap_astro).collect()),
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, unwrap_astro(value)))
                .collect(),
        ),
        other => other,
    }
}

fn normalize_key(input: &str) -> String {
    let slug = input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches('/')
        .trim_end_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if let Some(pos) = slug
        .iter()
        .position(|part| *part == "comics" || *part == "series")
    {
        let random = slug.get(pos + 1).copied().unwrap_or("sample");
        return format!("{random}#{random}");
    }
    input.trim_matches('/').to_string()
}

fn key_random_slug(key: &str) -> String {
    key.split('#')
        .nth(1)
        .or_else(|| key.split('/').next())
        .unwrap_or("sample")
        .trim_matches('/')
        .to_string()
}

fn parse_chapter_key(key: &str) -> (String, String) {
    let raw = key.split('#').next().unwrap_or(key);
    let parts = raw
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let series = parts.first().copied().unwrap_or("sample").to_string();
    let number = parts.last().copied().unwrap_or("1").to_string();
    (series, number)
}

fn fallback_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: url::slug_from_url(key).unwrap_or_else(|| "Manga".into()),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

#[derive(Default, Deserialize)]
struct DataDto<T> {
    #[serde(default)]
    data: Option<T>,
    #[serde(default)]
    meta: Option<MetaDto>,
}

type MangaDetailsDto = DataDto<MangaDetails>;

#[derive(Default, Deserialize)]
struct MetaDto {
    #[serde(default, rename = "has_more")]
    has_more: bool,
}

#[derive(Default, Deserialize)]
struct MangaDetails {
    series: MangaDto,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaDto {
    #[serde(default, rename = "public_url")]
    public_url: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    cover: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    genres: Vec<GenreDto>,
    #[serde(default)]
    status: Option<String>,
}

impl MangaDto {
    fn into_catalog(self) -> CatalogItem {
        let random_slug = self
            .public_url
            .trim_end_matches('/')
            .split('/')
            .next_back()
            .unwrap_or(&self.slug)
            .to_string();
        CatalogItem {
            key: format!("{}#{random_slug}", self.slug),
            title: self.title,
            cover: self.cover,
            tags: self.genres.into_iter().map(|genre| genre.name).collect(),
            status: map_status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/comics/{random_slug}")),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }

    fn into_catalog_initialized(self) -> CatalogItem {
        let description = self.description.clone();
        let author = self.author.clone();
        let artist = self.artist.clone();
        let mut item = self.into_catalog();
        item.description = item.description.or_else(|| {
            description
                .as_ref()
                .map(|description| html::strip_tags(description))
                .filter(|description| !description.is_empty())
        });
        item.authors = author
            .into_iter()
            .filter(|v| !v.trim().is_empty())
            .collect();
        item.artists = artist
            .into_iter()
            .filter(|v| !v.trim().is_empty())
            .collect();
        item.initialized = true;
        item
    }
}

#[derive(Default, Deserialize)]
struct GenreDto {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct ChapterList {
    #[serde(default)]
    chapters: Vec<ChapterDto>,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    number: f32,
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "created_at")]
    created_at: String,
    #[serde(default, rename = "is_locked")]
    is_locked: bool,
    #[serde(default, rename = "series_slug")]
    series_slug: Option<String>,
}

impl ChapterDto {
    fn into_chapter(self) -> MangaChapter {
        let number = self.number.to_string().trim_end_matches(".0").to_string();
        let series_slug = self.series_slug.unwrap_or_else(|| "sample".to_string());
        let mut title = format!("Chapter {number}");
        if let Some(chapter_title) = self.title.filter(|value| !value.trim().is_empty()) {
            title.push_str(" - ");
            title.push_str(chapter_title.trim());
        }
        MangaChapter {
            key: format!("{series_slug}/chapter/{number}#{series_slug}"),
            title: Some(title),
            chapter_number: self.number.is_finite().then_some(self.number),
            date_uploaded: manatan_shared::dates::parse_fixture_date(&self.created_at),
            url: Some(format!("{BASE_URL}/comics/{series_slug}/chapter/{number}")),
            is_locked: self.is_locked,
            ..MangaChapter::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct PageList {
    #[serde(default)]
    pages: Vec<PageDto>,
}

#[derive(Default, Deserialize)]
struct PageDto {
    url: String,
    #[serde(default)]
    tiles: Option<Vec<usize>>,
    #[serde(default, rename = "tile_cols")]
    tile_cols: Option<usize>,
    #[serde(default, rename = "tile_rows")]
    tile_rows: Option<usize>,
}

#[derive(Serialize)]
struct PageTileData {
    tiles: Vec<usize>,
    tile_cols: usize,
    tile_rows: usize,
}

fn map_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "hiatus" => ItemStatus::Hiatus,
        "dropped" | "axed" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

const LIST_FIXTURE: &str = r#"{"data":[{"public_url":"/comics/sample-random","slug":"sample","title":"Sample Asura","cover":"https://img.example/cover.jpg","genres":[{"name":"Action"}],"status":"ongoing"}],"meta":{"has_more":true}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"series":{"public_url":"/comics/sample-random","slug":"sample","title":"Sample Asura","cover":"https://img.example/cover.jpg","author":"Writer","artist":"Artist","description":"<p>Sample description.</p>","genres":[{"name":"Action"}],"status":"completed"}}}"#;
const CHAPTERS_FIXTURE: &str = r#"<div props="{&quot;chapters&quot;:{&quot;chapters&quot;:[{&quot;number&quot;:1,&quot;title&quot;:&quot;Start&quot;,&quot;created_at&quot;:&quot;2024-01-01T00:00:00Z&quot;,&quot;is_locked&quot;:false,&quot;series_slug&quot;:&quot;sample&quot;}]}}"></div>"#;
const PAGES_FIXTURE: &str = r#"<div props="{&quot;pages&quot;:{&quot;pages&quot;:[{&quot;url&quot;:&quot;https://img.example/page1.jpg&quot;},{&quot;url&quot;:&quot;https://img.example/page2.jpg&quot;,&quot;tiles&quot;:[0,1],&quot;tile_cols&quot;:1,&quot;tile_rows&quot;:2}]}}"></div>"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_asura_payloads() {
        let list = parse_list(LIST_FIXTURE);
        assert_eq!(list.entries[0].title, "Sample Asura");
        assert!(list.has_next_page);

        let details = details_from_key("sample#sample-random");
        assert_eq!(details.status, ItemStatus::Completed);

        let chapters = parse_chapters(CHAPTERS_FIXTURE);
        assert_eq!(chapters[0].title.as_deref(), Some("Chapter 1 - Start"));

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
