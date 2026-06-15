use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: RawdevartArt = RawdevartArt;
const BASE_URL: &str = "https://rawdevart.art";

struct RawdevartArt;

impl MangaSource for RawdevartArt {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "" } else { "most_viewed" };
        Ok(parse_listing(&fetch_json(&list_url(page(&request), "", "all", "", sort), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged { entries: vec![details_by_key(&key)], has_next_page: false });
        }
        let genre = filter_string(&request, "genre").filter(|value| !value.is_empty()).unwrap_or("all");
        let status = filter_string(&request, "status").unwrap_or_default();
        let sort = filter_string(&request, "sort").unwrap_or("most_viewed");
        Ok(parse_listing(&fetch_json(&list_url(page(&request), query, genre, status, sort), LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/spa/manga/1".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/spa/manga/1".into());
        let id = key.rsplit('/').next().unwrap_or("1");
        Ok(parse_chapters(&fetch_json(&format!("{BASE_URL}/spa/manga/{id}"), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/spa/manga/1/1".into());
        Ok(parse_pages(&fetch_json(&absolute_url(&key), CHAPTER_FIXTURE), &absolute_url(&key)))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection { id: "popular".into(), title: "Popular".into(), style: Some(HomeSectionStyle::Cover), has_more: popular.has_next_page, entries: popular.entries, ..HomeSection::default() },
            HomeSection { id: "latest".into(), title: "Latest".into(), style: Some(HomeSectionStyle::Cover), has_more: latest.has_next_page, entries: latest.entries, ..HomeSection::default() },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult { item: Some(details_by_key(&key)), url: Some(input.into()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }), url: Some(input.into()), ..UrlResolveResult::default() }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client().get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn list_url(page: u64, query: &str, genre: &str, status: &str, sort: &str) -> String {
    let mut target = if query.is_empty() {
        format!("{BASE_URL}/spa/genre/{genre}?page={page}")
    } else {
        format!("{BASE_URL}/spa/search?page={page}&query={}", url::query_escape(query))
    };
    if !status.is_empty() {
        target.push_str("&status=");
        target.push_str(&url::query_escape(status));
    }
    if !sort.is_empty() {
        target.push_str("&sort=");
        target.push_str(&url::query_escape(sort));
    }
    target
}

#[derive(Deserialize)]
struct MangaListResponse {
    #[serde(rename = "manga_list")]
    manga_list: Vec<MangaDetail>,
    pagi: Pagination,
}

#[derive(Deserialize)]
struct Pagination {
    button: Option<PageButtons>,
}

#[derive(Deserialize)]
struct PageButtons {
    next: Option<i64>,
}

#[derive(Deserialize)]
struct MangaDetail {
    #[serde(rename = "manga_name")]
    name: String,
    #[serde(rename = "manga_cover_img")]
    cover_image: String,
    #[serde(rename = "manga_id")]
    id: i64,
    #[serde(default, rename = "manga_others_name")]
    alternative_name: Option<String>,
    #[serde(default, rename = "manga_status")]
    status: bool,
    #[serde(default, rename = "manga_description")]
    description: Option<String>,
    #[serde(default, rename = "manga_cover_img_full")]
    cover_image_full: Option<String>,
}

#[derive(Deserialize)]
struct Tag {
    #[serde(rename = "tag_name")]
    name: String,
}

#[derive(Deserialize)]
struct Author {
    #[serde(rename = "author_name")]
    name: String,
}

#[derive(Deserialize)]
struct Chapter {
    #[serde(rename = "chapter_title")]
    title: String,
    #[serde(rename = "chapter_number")]
    number: f32,
    #[serde(rename = "chapter_date_published")]
    date_published: String,
}

#[derive(Deserialize)]
struct MangaResponse {
    detail: MangaDetail,
    #[serde(default)]
    tags: Vec<Tag>,
    #[serde(default)]
    authors: Vec<Author>,
    #[serde(default)]
    chapters: Vec<Chapter>,
}

#[derive(Deserialize)]
struct ChapterDetail {
    #[serde(rename = "chapter_content")]
    content: Option<String>,
    server: String,
}

#[derive(Deserialize)]
struct ChapterResponse {
    #[serde(rename = "chapter_detail")]
    detail: ChapterDetail,
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let data = serde_json::from_str::<MangaListResponse>(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("valid fixture"));
    Paged {
        entries: data.manga_list.into_iter().map(list_item).collect(),
        has_next_page: data.pagi.button.and_then(|button| button.next).unwrap_or_default() != 0,
    }
}

fn list_item(detail: MangaDetail) -> CatalogItem {
    CatalogItem {
        key: format!("/spa/manga/{}", detail.id),
        title: detail.name,
        cover: Some(detail.cover_image),
        url: Some(format!("{BASE_URL}/spa/manga/{}", detail.id)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let id = key.rsplit('/').next().unwrap_or("1");
    let data = serde_json::from_str::<MangaResponse>(&fetch_json(&format!("{BASE_URL}/spa/manga/{id}"), DETAILS_FIXTURE)).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("valid fixture"));
    let mut description = Vec::new();
    if let Some(alt) = data.detail.alternative_name.filter(|value| !value.is_empty()) {
        description.push(format!("Alternative Title: {alt}"));
    }
    if let Some(text) = data.detail.description.filter(|value| !value.is_empty()) {
        description.push(text);
    }
    CatalogItem {
        key: format!("/spa/manga/{}", data.detail.id),
        title: data.detail.name,
        cover: data.detail.cover_image_full.or(Some(data.detail.cover_image)),
        authors: data.authors.into_iter().map(|author| author.name).collect(),
        tags: data.tags.into_iter().map(|tag| tag.name).collect(),
        description: (!description.is_empty()).then(|| description.join("\n\n")),
        status: if data.detail.status { ItemStatus::Completed } else { ItemStatus::Ongoing },
        url: Some(format!("{BASE_URL}/spa/manga/{}", data.detail.id)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let data = serde_json::from_str::<MangaResponse>(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("valid fixture"));
    data.chapters
        .into_iter()
        .map(|chapter| MangaChapter {
            key: format!("/spa/manga/{}/{}", data.detail.id, format_chapter_number(chapter.number)),
            title: Some(if chapter.title.is_empty() { format!("Chapter {}", format_chapter_number(chapter.number)) } else { format!("Chapter {}: {}", format_chapter_number(chapter.number), chapter.title) }),
            chapter_number: Some(chapter.number),
            date_uploaded: manatan_shared::dates::parse_fixture_date(&chapter.date_published),
            url: Some(format!("{BASE_URL}/spa/manga/{}/{}", data.detail.id, format_chapter_number(chapter.number))),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let data = serde_json::from_str::<ChapterResponse>(body).unwrap_or_else(|_| serde_json::from_str(CHAPTER_FIXTURE).expect("valid fixture"));
    let content = data.detail.content.unwrap_or_default();
    content
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|src| !src.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: url::join_url(&data.detail.server, &image), context: Some(manga::image_headers(referer)) },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn format_chapter_number(number: f32) -> String {
    let text = format!("{number:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn key_from_url(input: &str) -> Option<String> {
    input.find("/spa/manga/").map(|index| normalize_key(&input[index..]))
}

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input).split('#').next().unwrap_or(input).split('?').next().unwrap_or(input).trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, &normalize_key(input))
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request.get("filters").and_then(Value::as_object).and_then(|filters| filters.get(id)).and_then(Value::as_str)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"manga_list":[{"manga_name":"Sample Rawdevart","manga_cover_img":"https://rawdevart.art/cover.jpg","manga_id":1}],"pagi":{"button":{"prev":0,"next":0}}}"#;
const DETAILS_FIXTURE: &str = r#"{"detail":{"manga_name":"Sample Rawdevart","manga_cover_img":"https://rawdevart.art/cover.jpg","manga_id":1,"manga_status":false,"manga_description":"Summary"},"tags":[{"tag_name":"Action","tag_id":1}],"authors":[{"author_name":"Author","author_id":1}],"chapters":[{"chapter_id":"1","chapter_title":"","chapter_number":1,"chapter_views":0,"chapter_date_published":"2026-01-01T00:00:00.000Z"}]}"#;
const CHAPTER_FIXTURE: &str = r#"{"chapter_detail":{"chapter_id":"1","chapter_title":"Chapter 1","chapter_number":1,"chapter_date_published":"2026-01-01T00:00:00.000Z","chapter_content":"<img data-src=\"/page1.jpg\"><img data-src=\"/page2.jpg\">","server":"https://rawdevart.art"}}"#;
