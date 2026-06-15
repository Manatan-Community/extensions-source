use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: SimplyHentai = SimplyHentai;
const BASE_URL: &str = "https://www.simply-hentai.com";
const API_URL: &str = "https://api.simply-hentai.com/v3";

struct SimplyHentai;

impl MangaSource for SimplyHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let mut target = format!(
            "{API_URL}/tag/{}?type=language&page={}",
            source.lang_name,
            page_for(&request)
        );
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            target.push_str("&sort=newest");
        }
        Ok(parse_album_list(
            &fetch_json_or_fixture(&target, LIST_FIXTURE),
            source,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = normalize_key(query) {
            return Ok(Paged {
                entries: vec![catalog_from_key(&key, source)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let blacklist = request
            .get("preferences")
            .and_then(|prefs| prefs.get("blacklist"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut target = format!(
            "{API_URL}/search/complex?query={}&page={}&blacklist={}&filter[language][0]={}",
            url::query_escape(query),
            page_for(&request),
            url::query_escape(blacklist),
            url::query_escape(source.lang_title)
        );
        append_value(filters, "sort", "sort", &mut target);
        append_value(filters, "series", "filter[series_title][0]", &mut target);
        append_list(filters, "tags", "filter[tags]", &mut target);
        append_list(filters, "artists", "filter[artists]", &mut target);
        append_list(filters, "translators", "filter[translators]", &mut target);
        append_list(filters, "characters", "filter[characters]", &mut target);
        Ok(parse_search_list(
            &fetch_json_or_fixture(&target, SEARCH_FIXTURE),
            source,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let target = album_url(&key);
        Ok(parse_details(
            &fetch_json_or_fixture(&target, DETAILS_FIXTURE),
            source,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let target = album_url(&key);
        Ok(vec![parse_chapter(
            &fetch_json_or_fixture(&target, DETAILS_FIXTURE),
            &key,
            source,
        )])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/series/sample".into());
        let target = format!("{}/pages", album_url(&key));
        Ok(parse_pages(&fetch_json_or_fixture(&target, PAGES_FIXTURE)))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(serde_json::json!({
            "sourceId": source_for(&request).id,
            "listingId": "popular",
            "page": 1
        }))?;
        let latest = self.list(serde_json::json!({
            "sourceId": source_for(&request).id,
            "listingId": "latest",
            "page": 1
        }))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let source = source_for(&request);
        if let Some(key) = normalize_key(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(catalog_from_key(&key, source)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..UrlResolveResult::default()
            }));
        }
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

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    lang_name: &'static str,
    lang_title: &'static str,
}

const SOURCES: &[SourceConfig] = &[
    source("simplyhentai-en", "en", "english", "English"),
    source("simplyhentai-ja", "ja", "japanese", "Japanese"),
    source("simplyhentai-zh", "zh", "chinese", "Chinese"),
    source("simplyhentai-ko", "ko", "korean", "Korean"),
    source("simplyhentai-es", "es", "spanish", "Spanish"),
    source("simplyhentai-ru", "ru", "russian", "Russian"),
    source("simplyhentai-fr", "fr", "french", "French"),
    source("simplyhentai-de", "de", "german", "German"),
    source("simplyhentai-it", "it", "italian", "Italian"),
    source("simplyhentai-pl", "pl", "polish", "Polish"),
];

const fn source(
    id: &'static str,
    lang: &'static str,
    lang_name: &'static str,
    lang_title: &'static str,
) -> SourceConfig {
    SourceConfig {
        id,
        lang,
        lang_name,
        lang_title,
    }
}

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("simplyhentai-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn album_url(key: &str) -> String {
    format!("{API_URL}/manga/{}", slug_from_key(key))
}

fn slug_from_key(key: &str) -> &str {
    key.trim_matches('/').split('/').nth(1).unwrap_or("sample")
}

fn normalize_key(input: &str) -> Option<String> {
    let value = input.trim();
    let path = if value.starts_with(BASE_URL) {
        value.strip_prefix(BASE_URL)?.split(['?', '#']).next()?
    } else if value.starts_with('/') {
        value
    } else {
        return None;
    };
    let segments = path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    (segments.len() >= 2).then(|| format!("/{}/{}", segments[0], segments[1]))
}

fn parse_album_list(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<AlbumList>(body).unwrap_or_default();
    Paged {
        entries: response
            .data
            .albums
            .into_iter()
            .map(|album| album.into_catalog(source))
            .collect(),
        has_next_page: response.pagination.next.is_some(),
    }
}

fn parse_search_list(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<SearchList>(body).unwrap_or_default();
    Paged {
        entries: response
            .data
            .into_iter()
            .map(|wrapper| wrapper.object.into_catalog(source))
            .collect(),
        has_next_page: response.pagination.next.is_some(),
    }
}

fn parse_details(body: &str, source: SourceConfig) -> CatalogItem {
    serde_json::from_str::<Album>(body)
        .map(|album| album.data.into_catalog(source))
        .unwrap_or_else(|_| catalog_from_key("/series/sample", source))
}

fn parse_chapter(body: &str, key: &str, source: SourceConfig) -> MangaChapter {
    let album = serde_json::from_str::<Album>(body)
        .ok()
        .map(|album| album.data);
    MangaChapter {
        key: format!("{}/all-pages", key.trim_end_matches('/')),
        title: Some("Chapter".to_string()),
        chapter_number: Some(1.0),
        language: Some(source.lang.to_string()),
        scanlators: album
            .as_ref()
            .map(|album| {
                album
                    .translators
                    .iter()
                    .map(|tag| tag.title.clone())
                    .collect()
            })
            .unwrap_or_default(),
        url: Some(url::join_url(
            BASE_URL,
            &format!("{}/all-pages", key.trim_end_matches('/')),
        )),
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    serde_json::from_str::<AlbumPages>(body)
        .map(|pages| {
            pages
                .data
                .pages
                .into_iter()
                .map(|page| MangaPage {
                    content: PageContent::Url {
                        url: page.sizes.full,
                        context: Some(manga::image_headers(BASE_URL)),
                    },
                    thumbnail: Some(page.sizes.thumb),
                    description: Some(page.page_num.to_string()),
                    ..MangaPage::default()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn catalog_from_key(key: &str, source: SourceConfig) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: url::slug_from_url(key).unwrap_or_else(|| "Manga".to_string()),
        url: Some(url::join_url(BASE_URL, key)),
        language: Some(source.lang.to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: false,
        ..CatalogItem::default()
    }
}

fn append_value(filters: &Value, id: &str, parameter: &str, target: &mut String) {
    if let Some(value) = filters
        .get(id)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        target.push('&');
        target.push_str(parameter);
        target.push('=');
        target.push_str(&url::query_escape(value.trim()));
    }
}

fn append_list(filters: &Value, id: &str, parameter: &str, target: &mut String) {
    let Some(value) = filters
        .get(id)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return;
    };
    for (index, entry) in value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .enumerate()
    {
        target.push_str(&format!(
            "&{parameter}[{index}]={}",
            url::query_escape(entry)
        ));
    }
}

fn page_for(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

#[derive(Debug, Default, Deserialize)]
struct Pagination {
    next: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct AlbumData {
    #[serde(default)]
    albums: Vec<SummaryObject>,
}

#[derive(Debug, Default, Deserialize)]
struct AlbumList {
    #[serde(default)]
    pagination: Pagination,
    #[serde(default)]
    data: AlbumData,
}

#[derive(Debug, Default, Deserialize)]
struct SearchList {
    #[serde(default)]
    pagination: Pagination,
    #[serde(default)]
    data: Vec<SearchWrapper>,
}

#[derive(Debug, Deserialize)]
struct SearchWrapper {
    #[serde(rename = "object")]
    object: SummaryObject,
}

#[derive(Debug, Deserialize)]
struct SummaryObject {
    preview: Image,
    series: Tag,
    slug: String,
    title: String,
}

impl SummaryObject {
    fn into_catalog(self, source: SourceConfig) -> CatalogItem {
        let key = format!("/{}/{}", self.series.slug, self.slug);
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: Some(self.preview.sizes.thumb),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some(source.lang.to_string()),
            content_rating: Some("adult".to_string()),
            status: ItemStatus::Completed,
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct Album {
    data: DetailsData,
}

#[derive(Debug, Deserialize)]
struct DetailsData {
    #[serde(default)]
    artists: Vec<Tag>,
    #[serde(default)]
    characters: Vec<Tag>,
    #[allow(dead_code)]
    created_at: String,
    description: Option<String>,
    preview: Image,
    series: Tag,
    slug: String,
    #[serde(default)]
    tags: Vec<Tag>,
    title: String,
    #[serde(default)]
    translators: Vec<Tag>,
}

impl DetailsData {
    fn into_catalog(self, source: SourceConfig) -> CatalogItem {
        let key = format!("/{}/{}", self.series.slug, self.slug);
        let mut description = String::new();
        if let Some(value) = self.description.filter(|value| !value.trim().is_empty()) {
            description.push_str(&value);
            description.push_str("\n\n");
        }
        description.push_str("Series: ");
        description.push_str(&self.series.title);
        if !self.characters.is_empty() {
            description.push_str("\nCharacters: ");
            description.push_str(
                &self
                    .characters
                    .iter()
                    .map(|tag| tag.title.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        let artists = self
            .artists
            .iter()
            .map(|tag| tag.title.clone())
            .collect::<Vec<_>>();
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: Some(self.preview.sizes.thumb),
            url: Some(url::join_url(BASE_URL, &key)),
            authors: artists.clone(),
            artists,
            description: Some(description),
            tags: self.tags.into_iter().map(|tag| tag.title).collect(),
            language: Some(source.lang.to_string()),
            content_rating: Some("adult".to_string()),
            status: ItemStatus::Completed,
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct AlbumPages {
    data: PagesData,
}

#[derive(Debug, Deserialize)]
struct PagesData {
    pages: Vec<Image>,
}

#[derive(Debug, Deserialize)]
struct Image {
    page_num: u32,
    sizes: Sizes,
}

#[derive(Debug, Deserialize)]
struct Sizes {
    full: String,
    thumb: String,
}

#[derive(Debug, Deserialize)]
struct Tag {
    slug: String,
    title: String,
}

const LIST_FIXTURE: &str = r#"{
  "pagination": { "next": null },
  "data": { "albums": [
    { "preview": { "page_num": 1, "sizes": { "full": "https://cdn.example/full.jpg", "thumb": "https://cdn.example/thumb.jpg" } }, "series": { "slug": "sample-series", "title": "Sample Series" }, "slug": "sample-album", "title": "Sample Album" }
  ] }
}"#;

const SEARCH_FIXTURE: &str = r#"{
  "pagination": { "next": null },
  "data": [
    { "object": { "preview": { "page_num": 1, "sizes": { "full": "https://cdn.example/full.jpg", "thumb": "https://cdn.example/thumb.jpg" } }, "series": { "slug": "sample-series", "title": "Sample Series" }, "slug": "sample-album", "title": "Sample Album" } }
  ]
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "data": {
    "artists": [{ "slug": "artist", "title": "Sample Artist" }],
    "characters": [{ "slug": "character", "title": "Sample Character" }],
    "created_at": "2024-01-01T00:00:00.000",
    "description": "Sample description",
    "images": [],
    "preview": { "page_num": 1, "sizes": { "full": "https://cdn.example/full.jpg", "thumb": "https://cdn.example/thumb.jpg" } },
    "series": { "slug": "sample-series", "title": "Sample Series" },
    "slug": "sample-album",
    "tags": [{ "slug": "tag", "title": "Sample Tag" }],
    "title": "Sample Album",
    "translators": [{ "slug": "translator", "title": "Sample Translator" }]
  }
}"#;

const PAGES_FIXTURE: &str = r#"{
  "data": { "pages": [
    { "page_num": 1, "sizes": { "full": "https://cdn.example/page-1.jpg", "thumb": "https://cdn.example/thumb-1.jpg" } },
    { "page_num": 2, "sizes": { "full": "https://cdn.example/page-2.jpg", "thumb": "https://cdn.example/thumb-2.jpg" } }
  ] }
}"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_language_source() {
        assert_eq!(
            source_for(&serde_json::json!({"sourceId": "simplyhentai-ja"})).lang_name,
            "japanese"
        );
    }

    #[test]
    fn parses_album_list() {
        let page = parse_album_list(LIST_FIXTURE, SOURCES[0]);
        assert_eq!(page.entries[0].key, "/sample-series/sample-album");
        assert_eq!(page.entries[0].language.as_deref(), Some("en"));
    }

    #[test]
    fn parses_details() {
        let item = parse_details(DETAILS_FIXTURE, SOURCES[0]);
        assert_eq!(item.title, "Sample Album");
        assert_eq!(item.artists, vec!["Sample Artist"]);
        assert!(item.initialized);
    }

    #[test]
    fn parses_pages() {
        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
        match &pages[0].content {
            PageContent::Url { url, .. } => assert_eq!(url, "https://cdn.example/page-1.jpg"),
            _ => panic!("expected image URL page"),
        }
    }
}
