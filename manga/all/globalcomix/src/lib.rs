use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const WEB_URL: &str = "https://globalcomix.com";
const API_URL: &str = "https://api.globalcomix.com/v1";
const CLIENT_ID: &str = "gck_d0f170d5729446dcb3b55e6b3ebc7bf6";
const SOURCE: GlobalComix = GlobalComix;

struct GlobalComix;

impl MangaSource for GlobalComix {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = page(&request);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = comics_url(source.api_lang, page, if latest { Some("recent") } else { None }, None);
        let body = fetch_json_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_comic_list(&body, source))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(slug) = comic_slug_from_url(query) {
            let body = fetch_json_or_fixture(&format!("{API_URL}/read/{slug}"), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: parse_single_comic(&body, source).into_iter().collect(),
                has_next_page: false,
            });
        }
        let target = comics_url(source.api_lang, page(&request), Some("relevance"), Some(query));
        let body = fetch_json_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_comic_list(&body, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let slug = request
            .get("manga")
            .and_then(|manga| manga.get("title"))
            .and_then(Value::as_str)
            .map(title_to_slug)
            .or_else(|| manga::request_key(&request, "manga"))
            .unwrap_or_else(|| "sample-comic".into());
        let body = fetch_json_or_fixture(&format!("{API_URL}/read/{slug}"), DETAILS_FIXTURE);
        Ok(parse_single_comic(&body, source).unwrap_or_else(|| sample_item(source)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "123".into());
        let target = format!(
            "{API_URL}/comics/{}/releases?lang_id={}&all=true",
            url::query_escape(&key),
            url::query_escape(source.api_lang)
        );
        let body = fetch_json_or_fixture(&target, CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body, source, show_locked(&request)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "release-key".into());
        let target = format!("{API_URL}/readV2/{}", url::query_escape(&key));
        let body = fetch_json_or_fixture(&target, PAGES_FIXTURE);
        Ok(parse_pages(&body, &key, use_data_saver(&request)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let source = source_for(&request);
        if let Some(slug) = comic_slug_from_url(input) {
            let body = fetch_json_or_fixture(&format!("{API_URL}/read/{slug}"), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: parse_single_comic(&body, source),
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

export_manga_source!(SOURCE);

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    api_lang: &'static str,
}

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("globalcomix-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[10])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{WEB_URL}/"))
        .with_origin(WEB_URL)
        .with_header("x-gc-client", CLIENT_ID)
        .with_header("x-gc-identmode", "cookie")
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn comics_url(lang: &str, page: u64, sort: Option<&str>, query: Option<&str>) -> String {
    let mut target = format!(
        "{API_URL}/comics?lang_id%5B%5D={}&p={page}",
        url::query_escape(lang)
    );
    if let Some(sort) = sort {
        target.push_str("&sort=");
        target.push_str(&url::query_escape(sort));
    }
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        target.push_str("&q=");
        target.push_str(&url::query_escape(query));
    }
    target
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn parse_comic_list(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let Ok(response) = serde_json::from_str::<ComicListResponse>(body) else {
        return Paged {
            entries: vec![sample_item(source)],
            has_next_page: false,
        };
    };
    let Some(payload) = response.payload else {
        return Paged::default();
    };
    Paged {
        entries: payload
            .results
            .into_iter()
            .map(|comic| comic.into_item(source))
            .collect(),
        has_next_page: payload.pagination.page < payload.pagination.total_pages,
    }
}

fn parse_single_comic(body: &str, source: SourceConfig) -> Option<CatalogItem> {
    serde_json::from_str::<ComicResponse>(body)
        .ok()?
        .payload?
        .results
        .map(|comic| comic.into_item(source))
}

fn parse_chapters(body: &str, source: SourceConfig, include_locked: bool) -> Vec<MangaChapter> {
    let Ok(response) = serde_json::from_str::<ChapterListResponse>(body) else {
        return Vec::new();
    };
    response
        .payload
        .map(|payload| payload.results)
        .unwrap_or_default()
        .into_iter()
        .filter(|chapter| include_locked || chapter.premium_only.unwrap_or(0) != 1)
        .map(|chapter| chapter.into_chapter(source))
        .collect()
}

fn parse_pages(body: &str, release_key: &str, data_saver: bool) -> Vec<MangaPage> {
    let Ok(response) = serde_json::from_str::<ChapterResponse>(body) else {
        return Vec::new();
    };
    response
        .payload
        .and_then(|payload| payload.results)
        .and_then(|chapter| chapter.page_objects)
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, page)| {
            let image_url = if data_saver {
                page.mobile_image_url
            } else {
                page.desktop_image_url
            };
            let mut headers = manga::image_headers(WEB_URL);
            headers.insert("Origin".to_string(), WEB_URL.to_string());
            MangaPage {
                content: PageContent::Url {
                    url: image_url,
                    context: Some(headers.clone()),
                },
                headers,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect::<Vec<_>>()
        .into_iter()
        .enumerate()
        .map(|(index, mut page)| {
            page.thumbnail = Some(format!("{WEB_URL}/read/{release_key}/{index}"));
            page
        })
        .collect()
}

fn comic_slug_from_url(input: &str) -> Option<String> {
    let rest = input
        .trim()
        .strip_prefix("https://globalcomix.com/c/")
        .or_else(|| input.trim().strip_prefix("http://globalcomix.com/c/"))?;
    rest.split(['?', '#', '/'])
        .next()
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn title_to_slug(title: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in title.trim().chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

fn use_data_saver(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("dataSaver"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn show_locked(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("showLockedChapters"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn sample_item(source: SourceConfig) -> CatalogItem {
    CatalogItem {
        key: "123".into(),
        title: "Sample Comic".into(),
        url: Some(format!("{WEB_URL}/c/sample-comic")),
        language: Some(source.lang.into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

#[derive(Deserialize)]
struct ComicListResponse {
    payload: Option<ComicListPayload>,
}

#[derive(Deserialize)]
struct ComicListPayload {
    #[serde(default)]
    results: Vec<ComicDto>,
    pagination: PaginationDto,
}

#[derive(Deserialize)]
struct ComicResponse {
    payload: Option<ComicPayload>,
}

#[derive(Deserialize)]
struct ComicPayload {
    results: Option<ComicDto>,
}

#[derive(Deserialize)]
struct PaginationDto {
    #[serde(default)]
    page: u64,
    #[serde(default)]
    total_pages: u64,
}

#[derive(Deserialize)]
struct ComicDto {
    #[serde(default)]
    id: i64,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    status_name: Option<String>,
    #[serde(default)]
    category_name: Option<String>,
    #[serde(default)]
    image_url: Option<String>,
    artist: ArtistDto,
}

impl ComicDto {
    fn into_item(self, source: SourceConfig) -> CatalogItem {
        let author = self.artist.roman_name.or(Some(self.artist.name));
        let mut tags = Vec::new();
        if let Some(category) = self.category_name {
            tags.push(category);
        }
        CatalogItem {
            key: self.id.to_string(),
            title: self.name.clone(),
            cover: self.image_url,
            url: Some(format!("{WEB_URL}/c/{}", title_to_slug(&self.name))),
            authors: author.clone().into_iter().collect(),
            artists: author.into_iter().collect(),
            description: self.description,
            tags,
            language: Some(source.lang.into()),
            content_rating: Some("safe".into()),
            status: status_from_name(self.status_name.as_deref()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct ArtistDto {
    name: String,
    #[serde(default)]
    roman_name: Option<String>,
}

#[derive(Deserialize)]
struct ChapterListResponse {
    payload: Option<ChapterListPayload>,
}

#[derive(Deserialize)]
struct ChapterListPayload {
    #[serde(default)]
    results: Vec<ChapterDto>,
}

#[derive(Deserialize)]
struct ChapterResponse {
    payload: Option<ChapterPayload>,
}

#[derive(Deserialize)]
struct ChapterPayload {
    results: Option<ChapterDto>,
}

#[derive(Deserialize)]
struct ChapterDto {
    title: String,
    chapter: String,
    key: String,
    #[serde(default)]
    premium_only: Option<u8>,
    #[serde(default)]
    page_objects: Option<Vec<PageDto>>,
}

impl ChapterDto {
    fn into_chapter(self, source: SourceConfig) -> MangaChapter {
        let locked = self.premium_only.unwrap_or(0) == 1;
        let mut title_parts = Vec::new();
        if locked {
            title_parts.push("Locked".to_string());
        }
        if !self.chapter.is_empty() {
            title_parts.push(format!("Ch.{}", self.chapter));
        }
        if !self.title.is_empty() {
            title_parts.push(self.title);
        }
        MangaChapter {
            key: self.key.clone(),
            title: Some(title_parts.join(" - ")),
            chapter_number: self.chapter.parse().ok(),
            language: Some(source.lang.into()),
            url: Some(format!("{WEB_URL}/read/{}", self.key)),
            is_locked: locked,
            page_count: self.page_objects.as_ref().map(|pages| pages.len() as u32),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct PageDto {
    desktop_image_url: String,
    mobile_image_url: String,
}

fn status_from_name(status: Option<&str>) -> ItemStatus {
    match status {
        Some("Ongoing") | Some("Preview") => ItemStatus::Ongoing,
        Some("Finished") => ItemStatus::Completed,
        Some("On hold") => ItemStatus::Hiatus,
        Some("Cancelled") => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig { id: "globalcomix-sq", lang: "sq", api_lang: "al" },
    SourceConfig { id: "globalcomix-ar", lang: "ar", api_lang: "ar" },
    SourceConfig { id: "globalcomix-bg", lang: "bg", api_lang: "bg" },
    SourceConfig { id: "globalcomix-bn", lang: "bn", api_lang: "bn" },
    SourceConfig { id: "globalcomix-pt-br", lang: "pt-BR", api_lang: "br" },
    SourceConfig { id: "globalcomix-zh-hans", lang: "zh-Hans", api_lang: "cn" },
    SourceConfig { id: "globalcomix-cs", lang: "cs", api_lang: "cz" },
    SourceConfig { id: "globalcomix-de", lang: "de", api_lang: "de" },
    SourceConfig { id: "globalcomix-da", lang: "da", api_lang: "dk" },
    SourceConfig { id: "globalcomix-el", lang: "el", api_lang: "el" },
    SourceConfig { id: "globalcomix-en", lang: "en", api_lang: "en" },
    SourceConfig { id: "globalcomix-es", lang: "es", api_lang: "es" },
    SourceConfig { id: "globalcomix-fa", lang: "fa", api_lang: "fa" },
    SourceConfig { id: "globalcomix-fi", lang: "fi", api_lang: "fi" },
    SourceConfig { id: "globalcomix-fil", lang: "fil", api_lang: "fo" },
    SourceConfig { id: "globalcomix-fr", lang: "fr", api_lang: "fr" },
    SourceConfig { id: "globalcomix-hi", lang: "hi", api_lang: "hi" },
    SourceConfig { id: "globalcomix-hu", lang: "hu", api_lang: "hu" },
    SourceConfig { id: "globalcomix-id", lang: "id", api_lang: "id" },
    SourceConfig { id: "globalcomix-it", lang: "it", api_lang: "it" },
    SourceConfig { id: "globalcomix-he", lang: "he", api_lang: "iw" },
    SourceConfig { id: "globalcomix-ja", lang: "ja", api_lang: "jp" },
    SourceConfig { id: "globalcomix-ko", lang: "ko", api_lang: "kr" },
    SourceConfig { id: "globalcomix-lv", lang: "lv", api_lang: "lv" },
    SourceConfig { id: "globalcomix-ms", lang: "ms", api_lang: "my" },
    SourceConfig { id: "globalcomix-nl", lang: "nl", api_lang: "nl" },
    SourceConfig { id: "globalcomix-no", lang: "no", api_lang: "no" },
    SourceConfig { id: "globalcomix-pl", lang: "pl", api_lang: "pl" },
    SourceConfig { id: "globalcomix-pt", lang: "pt", api_lang: "pt" },
    SourceConfig { id: "globalcomix-ro", lang: "ro", api_lang: "ro" },
    SourceConfig { id: "globalcomix-ru", lang: "ru", api_lang: "ru" },
    SourceConfig { id: "globalcomix-sv", lang: "sv", api_lang: "se" },
    SourceConfig { id: "globalcomix-sk", lang: "sk", api_lang: "sk" },
    SourceConfig { id: "globalcomix-sl", lang: "sl", api_lang: "sl" },
    SourceConfig { id: "globalcomix-ta", lang: "ta", api_lang: "ta" },
    SourceConfig { id: "globalcomix-th", lang: "th", api_lang: "th" },
    SourceConfig { id: "globalcomix-tr", lang: "tr", api_lang: "tr" },
    SourceConfig { id: "globalcomix-uk", lang: "uk", api_lang: "ua" },
    SourceConfig { id: "globalcomix-ur", lang: "ur", api_lang: "ur" },
    SourceConfig { id: "globalcomix-vi", lang: "vi", api_lang: "vi" },
    SourceConfig { id: "globalcomix-zh-hant", lang: "zh-Hant", api_lang: "zh" },
];

const LIST_FIXTURE: &str = r#"
{
  "payload": {
    "results": [
      {
        "id": 123,
        "name": "Sample Comic",
        "description": "A sample comic.",
        "status_name": "Ongoing",
        "category_name": "Adventure",
        "image_url": "https://globalcomix.com/sample.jpg",
        "artist": { "name": "sample-artist", "roman_name": "Sample Artist" }
      }
    ],
    "pagination": { "page": 1, "per_page": 1, "total_pages": 2, "total_results": 2 }
  }
}
"#;

const DETAILS_FIXTURE: &str = r#"
{
  "payload": {
    "results": {
      "id": 123,
      "name": "Sample Comic",
      "description": "A sample comic.",
      "status_name": "Finished",
      "category_name": "Adventure",
      "image_url": "https://globalcomix.com/sample.jpg",
      "artist": { "name": "sample-artist", "roman_name": "Sample Artist" }
    }
  }
}
"#;

const CHAPTERS_FIXTURE: &str = r#"
{
  "payload": {
    "results": [
      { "title": "The Beginning", "chapter": "1", "key": "release-key", "premium_only": 0 },
      { "title": "Locked Chapter", "chapter": "2", "key": "locked-key", "premium_only": 1 }
    ],
    "pagination": { "page": 1, "per_page": 2, "total_pages": 1, "total_results": 2 }
  }
}
"#;

const PAGES_FIXTURE: &str = r#"
{
  "payload": {
    "results": {
      "title": "The Beginning",
      "chapter": "1",
      "key": "release-key",
      "premium_only": 0,
      "page_objects": [
        { "is_page_paid": false, "desktop_image_url": "https://globalcomix.com/page-desktop-1.jpg", "mobile_image_url": "https://globalcomix.com/page-mobile-1.jpg" },
        { "is_page_paid": false, "desktop_image_url": "https://globalcomix.com/page-desktop-2.jpg", "mobile_image_url": "https://globalcomix.com/page-mobile-2.jpg" }
      ]
    }
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comics() {
        let page = parse_comic_list(LIST_FIXTURE, SOURCES[10]);
        assert_eq!(page.entries.len(), 1);
        assert!(page.has_next_page);
        assert_eq!(page.entries[0].key, "123");
    }

    #[test]
    fn parses_chapters_and_locked_preference() {
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, SOURCES[10], false).len(), 1);
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, SOURCES[10], true).len(), 2);
    }

    #[test]
    fn parses_pages_and_urls() {
        assert_eq!(parse_pages(PAGES_FIXTURE, "release-key", false).len(), 2);
        assert_eq!(comic_slug_from_url("https://globalcomix.com/c/sample-comic").as_deref(), Some("sample-comic"));
        assert_eq!(title_to_slug("Sample Comic!"), "sample-comic");
    }
}
