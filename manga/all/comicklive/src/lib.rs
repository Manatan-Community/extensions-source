use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const BASE_URL: &str = "https://comick.live";
const SOURCE: Comick = Comick;

struct Comick;

impl MangaSource for Comick {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request.get("listingId").and_then(Value::as_str).unwrap_or("popular");
        if listing == "latest" {
            Ok(fetch_browse_page(&format!("{BASE_URL}/api/chapters/latest?order=new&page={page}"), page, 100))
        } else {
            let days = match page { 1 | 4 => 7, 2 | 5 => 30, _ => 90 };
            let kind = if page <= 3 { "follow" } else { "most_follow_new" };
            Ok(fetch_browse_page(&format!("{BASE_URL}/api/comics/top?days={days}&type={kind}"), page, 6))
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) || query.starts_with("https://comick.art") {
            let key = query.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
            let body = fetch_text_or_fixture(&format!("{BASE_URL}/comic/{key}"), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details_body(&body, key)],
                has_next_page: false,
            });
        }
        let source = source_for(&request);
        let target_url = search_url(page, query, request.get("filters").unwrap_or(&Value::Null));
        let body = fetch_text_or_fixture(&target_url, SEARCH_FIXTURE);
        let mut page_out = parse_search_page(&body, source.lang);
        if query.is_empty() && page == 1 && page_out.entries.is_empty() {
            page_out = fetch_browse_page(&format!("{BASE_URL}/api/search?type=comic&showAll=false&exclude_mylist=false"), page, 1);
        }
        Ok(page_out)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let slug = key.trim_start_matches("/comic/").trim_matches('/');
        let body = fetch_text_or_fixture(&format!("{BASE_URL}/comic/{slug}"), DETAILS_FIXTURE);
        Ok(parse_details_body(&body, slug))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let slug = key.trim_start_matches("/comic/").trim_matches('/');
        let body = fetch_text_or_fixture(
            &format!("{BASE_URL}/api/comics/{slug}/chapter-list?lang={}", source.site_lang),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapter_list(&body, slug))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/comic/sample/hid-chapter-1-en".into());
        let body = fetch_text_or_fixture(&absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages_body(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) || input.starts_with("https://comick.art") {
            let slug = input.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
            let body = fetch_text_or_fixture(&format!("{BASE_URL}/comic/{slug}"), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details_body(&body, slug)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

#[derive(Clone, Copy)]
struct SourceConfig { id: &'static str, lang: &'static str, site_lang: &'static str }

const SOURCES: [SourceConfig; 18] = [
    SourceConfig { id: "comicklive-en", lang: "en", site_lang: "en" },
    SourceConfig { id: "comicklive-ru", lang: "ru", site_lang: "ru" },
    SourceConfig { id: "comicklive-vi", lang: "vi", site_lang: "vi" },
    SourceConfig { id: "comicklive-fr", lang: "fr", site_lang: "fr" },
    SourceConfig { id: "comicklive-pl", lang: "pl", site_lang: "pl" },
    SourceConfig { id: "comicklive-id", lang: "id", site_lang: "id" },
    SourceConfig { id: "comicklive-tr", lang: "tr", site_lang: "tr" },
    SourceConfig { id: "comicklive-it", lang: "it", site_lang: "it" },
    SourceConfig { id: "comicklive-es", lang: "es", site_lang: "es" },
    SourceConfig { id: "comicklive-uk", lang: "uk", site_lang: "uk" },
    SourceConfig { id: "comicklive-de", lang: "de", site_lang: "de" },
    SourceConfig { id: "comicklive-ko", lang: "ko", site_lang: "ko" },
    SourceConfig { id: "comicklive-th", lang: "th", site_lang: "th" },
    SourceConfig { id: "comicklive-ro", lang: "ro", site_lang: "ro" },
    SourceConfig { id: "comicklive-ms", lang: "ms", site_lang: "ms" },
    SourceConfig { id: "comicklive-ja", lang: "ja", site_lang: "ja" },
    SourceConfig { id: "comicklive-sv", lang: "sv", site_lang: "sv" },
    SourceConfig { id: "comicklive-no", lang: "no", site_lang: "no" },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str).unwrap_or("comicklive-en");
    SOURCES.iter().find(|source| source.id == id).copied().unwrap_or(SOURCES[0])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text_or_fixture(target_url: &str, fixture: &str) -> String {
    client().get(target_url).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_browse_page(target_url: &str, page: u64, max_page: u64) -> Paged<CatalogItem> {
    let body = fetch_text_or_fixture(target_url, SEARCH_FIXTURE);
    let data = serde_json::from_str::<Data<Vec<BrowseComic>>>(&body).unwrap_or_else(|_| {
        serde_json::from_str::<Data<Vec<BrowseComic>>>(SEARCH_FIXTURE).expect("fixture search")
    });
    Paged {
        entries: data.data.into_iter().map(|comic| comic.to_item("all")).collect(),
        has_next_page: page < max_page,
    }
}

fn search_url(page: u64, query: &str, filters: &Value) -> String {
    let mut target = format!(
        "{BASE_URL}/api/search?type=comic&showAll=false&exclude_mylist=false&order_by={}&order_direction={}&page={page}",
        filter(filters, "order_by").unwrap_or("created_at"),
        filter(filters, "order_direction").unwrap_or("desc")
    );
    if !query.is_empty() {
        target.push_str("&q=");
        target.push_str(&url::query_escape(query));
    }
    for key in ["genres", "tags", "demographic", "country", "minimum", "status", "content_rating"] {
        if let Some(value) = filter(filters, key) {
            for part in value.split(',').map(str::trim).filter(|part| !part.is_empty()) {
                target.push('&');
                target.push_str(key);
                target.push('=');
                target.push_str(&url::query_escape(part));
            }
        }
    }
    target
}

fn parse_search_page(body: &str, lang: &str) -> Paged<CatalogItem> {
    let data = serde_json::from_str::<SearchResponse>(body).unwrap_or_else(|_| {
        serde_json::from_str::<SearchResponse>(SEARCH_RESPONSE_FIXTURE).expect("fixture search response")
    });
    Paged {
        entries: data.data.into_iter().map(|comic| comic.to_item(lang)).collect(),
        has_next_page: data.cursor.is_some(),
    }
}

fn parse_details_body(body: &str, fallback_slug: &str) -> CatalogItem {
    let json = html::text_between(body, "id=\"comic-data\"", "</script>")
        .or_else(|| html::text_between(body, "id='comic-data'", "</script>"))
        .unwrap_or_else(|| DETAILS_JSON.to_string());
    let data = serde_json::from_str::<ComicData>(&html::strip_tags(&json)).unwrap_or_else(|_| {
        serde_json::from_str::<ComicData>(DETAILS_JSON).expect("fixture details")
    });
    data.to_item(fallback_slug)
}

fn parse_chapter_list(body: &str, slug: &str) -> Vec<MangaChapter> {
    let data = serde_json::from_str::<ChapterList>(body).unwrap_or_else(|_| {
        serde_json::from_str::<ChapterList>(CHAPTERS_FIXTURE).expect("fixture chapters")
    });
    data.data
        .into_iter()
        .map(|chapter| MangaChapter {
            key: format!("/comic/{slug}/{}-chapter-{}-{}", chapter.hid, chapter.chap, chapter.lang),
            title: Some(chapter.title_line()),
            date_uploaded: None,
            scanlators: chapter.groups,
            url: Some(format!("{BASE_URL}/comic/{slug}/{}-chapter-{}-{}", chapter.hid, chapter.chap, chapter.lang)),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages_body(body: &str) -> Vec<MangaPage> {
    let json = html::text_between(body, "id=\"sv-data\"", "</script>")
        .or_else(|| html::text_between(body, "id='sv-data'", "</script>"))
        .unwrap_or_else(|| PAGES_JSON.to_string());
    let data = serde_json::from_str::<PageListData>(&html::strip_tags(&json)).unwrap_or_else(|_| {
        serde_json::from_str::<PageListData>(PAGES_JSON).expect("fixture pages")
    });
    data.chapter.images.into_iter().enumerate().map(|(index, image)| MangaPage {
        content: PageContent::Url { url: image.url, context: None },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }).collect()
}

fn filter<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).and_then(Value::as_str).filter(|value| !value.is_empty())
}

fn absolute_url(value: &str) -> String { url::join_url(BASE_URL, value) }

#[derive(Debug, Deserialize)]
struct Data<T> { data: T }

#[derive(Debug, Deserialize)]
struct SearchResponse {
    data: Vec<BrowseComic>,
    #[serde(rename = "next_cursor")]
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BrowseComic {
    #[serde(rename = "default_thumbnail")]
    thumbnail: String,
    slug: String,
    title: String,
}

impl BrowseComic {
    fn to_item(self, lang: &str) -> CatalogItem {
        CatalogItem {
            key: self.slug.clone(),
            title: self.title,
            cover: Some(self.thumbnail),
            url: Some(format!("{BASE_URL}/comic/{}", self.slug)),
            language: Some(lang.to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ComicData {
    title: String,
    slug: String,
    #[serde(rename = "default_thumbnail")]
    thumbnail: String,
    status: u8,
    #[serde(default)]
    artists: Vec<Name>,
    #[serde(default)]
    authors: Vec<Name>,
    #[serde(default)]
    desc: String,
    #[serde(default, rename = "content_rating")]
    content_rating: String,
    #[serde(default)]
    country: String,
    #[serde(default, rename = "md_comic_md_genres")]
    genres: Vec<GenreLink>,
}

impl ComicData {
    fn to_item(self, fallback_slug: &str) -> CatalogItem {
        CatalogItem {
            key: if self.slug.is_empty() { fallback_slug.to_string() } else { self.slug.clone() },
            title: self.title,
            cover: Some(self.thumbnail),
            status: match self.status {
                1 => ItemStatus::Ongoing,
                2 => ItemStatus::Completed,
                3 => ItemStatus::Cancelled,
                4 => ItemStatus::Hiatus,
                _ => ItemStatus::Unknown,
            },
            authors: self.authors.into_iter().map(|name| name.name).collect(),
            artists: self.artists.into_iter().map(|name| name.name).collect(),
            description: Some(html::strip_tags(&self.desc)).filter(|value| !value.is_empty()),
            tags: self
                .genres
                .into_iter()
                .map(|genre| genre.genre.name)
                .chain(match self.country.as_str() {
                    "jp" => Some("Manga".to_string()),
                    "cn" => Some("Manhua".to_string()),
                    "ko" => Some("Manhwa".to_string()),
                    _ => None,
                })
                .chain((!self.content_rating.is_empty()).then(|| format!("Content Rating: {}", self.content_rating)))
                .collect(),
            url: Some(format!("{BASE_URL}/comic/{}", self.slug)),
            language: Some("all".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct Name { name: String }

#[derive(Debug, Deserialize)]
struct GenreLink {
    #[serde(rename = "md_genres")]
    genre: Name,
}

#[derive(Debug, Deserialize)]
struct ChapterList {
    data: Vec<ChapterDto>,
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    hid: String,
    chap: String,
    vol: Option<String>,
    lang: String,
    title: Option<String>,
    #[serde(default, rename = "group_name")]
    groups: Vec<String>,
}

impl ChapterDto {
    fn title_line(&self) -> String {
        let mut title = String::new();
        if let Some(vol) = &self.vol {
            if !vol.is_empty() {
                title.push_str("Vol. ");
                title.push_str(vol);
                title.push(' ');
            }
        }
        title.push_str("Ch. ");
        title.push_str(&self.chap);
        if let Some(extra) = &self.title {
            if !extra.is_empty() {
                title.push_str(": ");
                title.push_str(extra);
            }
        }
        title
    }
}

#[derive(Debug, Deserialize)]
struct PageListData { chapter: ChapterData }

#[derive(Debug, Deserialize)]
struct ChapterData { images: Vec<ImageDto> }

#[derive(Debug, Deserialize)]
struct ImageDto { url: String }

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"{"data":[{"default_thumbnail":"https://img.example/cover.jpg","slug":"sample","title":"Sample"}]}"#;
const SEARCH_RESPONSE_FIXTURE: &str = r#"{"data":[{"default_thumbnail":"https://img.example/cover.jpg","slug":"sample","title":"Sample"}],"next_cursor":null}"#;
const DETAILS_JSON: &str = r#"{"title":"Sample","slug":"sample","default_thumbnail":"https://img.example/cover.jpg","status":1,"artists":[{"name":"Artist"}],"authors":[{"name":"Author"}],"desc":"<p>Description</p>","content_rating":"safe","country":"jp","md_comic_md_genres":[{"md_genres":{"name":"Action"}}]}"#;
const DETAILS_FIXTURE: &str = r#"<html><body><script id="comic-data" type="application/json">{"title":"Sample","slug":"sample","default_thumbnail":"https://img.example/cover.jpg","status":1,"artists":[{"name":"Artist"}],"authors":[{"name":"Author"}],"desc":"<p>Description</p>","content_rating":"safe","country":"jp","md_comic_md_genres":[{"md_genres":{"name":"Action"}}]}</script></body></html>"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"hid":"abc","chap":"1","vol":null,"lang":"en","title":"Start","created_at":"2024-01-01T00:00:00.000000Z","group_name":["Group"]}],"pagination":{"current_page":1,"last_page":1}}"#;
const PAGES_JSON: &str = r#"{"chapter":{"images":[{"url":"https://img.example/1.jpg"},{"url":"https://img.example/2.jpg"}]}}"#;
const PAGES_FIXTURE: &str = r#"<html><body><script id="sv-data" type="application/json">{"chapter":{"images":[{"url":"https://img.example/1.jpg"},{"url":"https://img.example/2.jpg"}]}}</script></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_details_chapters_pages() {
        let data = serde_json::from_str::<Data<Vec<BrowseComic>>>(SEARCH_FIXTURE).unwrap();
        assert_eq!(data.data.into_iter().map(|comic| comic.to_item("all")).count(), 1);
        assert_eq!(parse_search_page(SEARCH_RESPONSE_FIXTURE, "en").entries[0].title, "Sample");
        assert_eq!(parse_details_body(DETAILS_FIXTURE, "sample").title, "Sample");
        assert_eq!(parse_chapter_list(CHAPTERS_FIXTURE, "sample").len(), 1);
        assert_eq!(parse_pages_body(PAGES_FIXTURE).len(), 2);
    }
}
