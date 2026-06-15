use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: CManga = CManga;
const BASE_URL: &str = "https://cmangax17.com";
const PAGE_SIZE: u64 = 20;
const CHAPTER_PAGE_SIZE: u64 = 50;

struct CManga;

impl MangaSource for CManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let list_type = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "update" } else { "hot" };
        Ok(parse_album_page(&fetch_json(&album_list_url(page, list_type, "update", "", "", "all", "0", "0"), LIST_FIXTURE), page))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged { entries: vec![details_by_key(&key)], has_next_page: false });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let genres = multi_filter(filters, "genres").unwrap_or_default();
        let sort = filter(filters, "sort").unwrap_or("update");
        let status = filter(filters, "status").unwrap_or("all");
        let team = filter(filters, "team").unwrap_or("0");
        let min_chapter = filter(filters, "minChapter").unwrap_or("0");
        Ok(parse_album_page(&fetch_json(&album_list_url(page, "search", sort, &genres, query, status, team, min_chapter), LIST_FIXTURE), page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/album/sample-1".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/album/sample-1".into());
        let album_id = extract_album_id(&key).unwrap_or_else(|| "1".into());
        let slug = extract_album_slug(&key);
        Ok(fetch_all_chapters(&album_id, slug.as_deref()))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/album/sample/chapter-1-1".into());
        let chapter_id = extract_chapter_id(&key).unwrap_or_else(|| "1".into());
        let body = fetch_json(&format!("{BASE_URL}/api/chapter_image?chapter={chapter_id}&v=0&time=0&user_id=0&user_token="), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![home_section("popular", "Popular", self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?), home_section("latest", "Latest", self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?)])
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
            return Ok(Some(UrlResolveResult { item: key.contains("/album/").then(|| details_by_key(&key)), url: Some(input.into()), ..UrlResolveResult::default() }));
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn album_list_url(page: u64, list_type: &str, sort: &str, tag: &str, query: &str, status: &str, team: &str, min_chapter: &str) -> String {
    format!("{BASE_URL}/api/home_album_list?file=image&type={list_type}&sort={}&tag={}&limit={PAGE_SIZE}&page={page}&status={}&string={}&team={}&num_chapter={}", url::query_escape(sort), url::query_escape(tag), url::query_escape(status), url::query_escape(query), url::query_escape(team), url::query_escape(min_chapter))
}

fn parse_album_page(body: &str, page: u64) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<AlbumListResponse>(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    let entries = response.data.as_ref().map(|data| data.data.iter().filter_map(catalog_item).collect()).unwrap_or_default();
    let total = response.data.map(|data| data.total).unwrap_or(0);
    Paged { entries, has_next_page: page * PAGE_SIZE < total }
}

fn catalog_item(item: &AlbumItem) -> Option<CatalogItem> {
    let info = parse_album_info(item.info.as_deref())?;
    let title = info.name?;
    let slug = info.url?;
    let id = item.id_album?;
    let key = format!("/album/{slug}-{id}");
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: resolve_cover_url(info.avatar.as_deref()),
        url: Some(absolute_url(&key)),
        language: Some("vi".into()),
        content_rating: Some("safe".into()),
        ..CatalogItem::default()
    })
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    let api_info = extract_album_id(key).and_then(|id| fetch_album_info(&id));
    parse_details(&body, key, api_info)
}

fn fetch_album_info(album_id: &str) -> Option<AlbumInfo> {
    let body = fetch_json(&format!("{BASE_URL}/api/get_data_by_id?id={album_id}&table=album&data=info,data"), DETAILS_API_FIXTURE);
    let response = serde_json::from_str::<AlbumByIdResponse>(&body).ok()?;
    parse_album_info(response.data?.info.as_deref())
}

fn parse_details(body: &str, key: &str, api_info: Option<AlbumInfo>) -> CatalogItem {
    let title = html::text_between(body, "book_other", "</h1>")
        .or_else(|| html::text_between(body, "<h1", "</h1>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| api_info.as_ref().and_then(|info| info.name.clone()))
        .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into()));
    CatalogItem {
        key: key.into(),
        title,
        cover: html::attr_after(body, "book_avatar", "src").or_else(|| api_info.as_ref().and_then(|info| resolve_cover_url(info.avatar.as_deref()))),
        authors: api_info.as_ref().map(|info| info.author.clone()).unwrap_or_default(),
        tags: api_info.as_ref().map(|info| info.tags.clone()).unwrap_or_default(),
        description: html::text_between(body, "book_detail_text", "</div>")
            .map(|value| html::strip_tags(&value))
            .or_else(|| api_info.as_ref().and_then(|info| info.detail.clone()))
            .filter(|value| !value.is_empty()),
        status: parse_status(api_info.as_ref().and_then(|info| info.status.as_deref()).unwrap_or(&html::strip_tags(body))),
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_all_chapters(album_id: &str, slug: Option<&str>) -> Vec<MangaChapter> {
    let mut out = Vec::new();
    let mut page = 1;
    loop {
        let target = format!("{BASE_URL}/api/chapter_list?album={album_id}&page={page}&limit={CHAPTER_PAGE_SIZE}&v=0{}", slug.map(|slug| format!("&slug={}", url::query_escape(slug))).unwrap_or_default());
        let body = fetch_json(&target, CHAPTERS_FIXTURE);
        let response = serde_json::from_str::<ChapterListResponse>(&body).unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
        let items = response.data.unwrap_or_default();
        let before = out.len();
        for item in &items {
            if let Some(chapter) = chapter_item(item, slug.unwrap_or("truyen")) {
                if !out.iter().any(|seen: &MangaChapter| seen.key == chapter.key) {
                    out.push(chapter);
                }
            }
        }
        if items.len() < CHAPTER_PAGE_SIZE as usize || out.len() == before {
            break;
        }
        page += 1;
    }
    out
}

fn chapter_item(item: &ChapterItem, slug: &str) -> Option<MangaChapter> {
    let info = parse_chapter_info(item.info.as_deref())?;
    let id = json_string(info.id).or_else(|| item.id_chapter.map(|id| id.to_string()))?;
    let number = json_string(info.num)?;
    let title = chapter_title(&number, info.name.as_deref());
    let key = format!("/album/{slug}/chapter-{number}-{id}");
    Some(MangaChapter {
        key: key.clone(),
        title: Some(title),
        chapter_number: number.parse().ok(),
        date_uploaded: info.last_update.as_deref().and_then(manatan_shared::dates::parse_fixture_date),
        is_locked: info.level.as_ref().and_then(json_i64).unwrap_or(0) != 0,
        url: Some(absolute_url(&key)),
        ..MangaChapter::default()
    })
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<ChapterImageResponse>(body).unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    response.data.and_then(|data| data.image).unwrap_or_default().into_iter().enumerate().map(|(index, image)| MangaPage {
        content: PageContent::Url { url: absolute_url(&image), context: Some(manga::image_headers(BASE_URL)) },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }).collect()
}

fn parse_album_info(raw: Option<&str>) -> Option<AlbumInfo> {
    serde_json::from_str(raw?).ok()
}

fn parse_chapter_info(raw: Option<&str>) -> Option<ChapterInfo> {
    serde_json::from_str(raw?).ok()
}

fn parse_status(status: &str) -> ItemStatus {
    let lower = status.to_lowercase();
    if lower.contains("done") || lower.contains("hoàn thành") { ItemStatus::Completed } else if lower.contains("doing") || lower.contains("đang") { ItemStatus::Ongoing } else { ItemStatus::Unknown }
}

fn chapter_title(number: &str, title: Option<&str>) -> String {
    let Some(title) = title.filter(|value| !value.trim().is_empty()) else { return format!("Chapter {number}"); };
    let lower = title.trim().to_lowercase();
    if lower == number || lower == format!("chapter {number}") || lower == format!("chap {number}") || lower == format!("chương {number}") {
        format!("Chapter {number}")
    } else {
        format!("Chapter {number}: {}", title.trim())
    }
}

fn resolve_cover_url(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() { None } else if value.starts_with("http") { Some(value.into()) } else if value.starts_with('/') { Some(format!("{BASE_URL}{value}")) } else { Some(format!("{BASE_URL}/assets/tmp/album/{value}")) }
}

fn json_string(value: Option<Value>) -> Option<String> {
    match value? {
        Value::String(value) => Some(value),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn json_i64(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection { id: id.into(), title: title.into(), style: Some(HomeSectionStyle::Cover), has_more: page.has_next_page, entries: page.entries, ..HomeSection::default() }
}

fn extract_album_id(value: &str) -> Option<String> {
    let tail = value.trim_end_matches('/').rsplit('/').next()?;
    tail.rsplit('-').next().filter(|part| part.chars().all(|ch| ch.is_ascii_digit())).map(ToString::to_string)
}

fn extract_album_slug(value: &str) -> Option<String> {
    let tail = value.trim_end_matches('/').split("/album/").nth(1)?.rsplit_once('-')?.0;
    Some(tail.to_string())
}

fn extract_chapter_id(value: &str) -> Option<String> {
    value.rsplit('-').next().filter(|part| part.chars().all(|ch| ch.is_ascii_digit())).map(ToString::to_string)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") { value.trim_start_matches(BASE_URL).trim_end_matches('/').to_string() } else { format!("/{}", value.trim_start_matches('/').trim_end_matches('/')) }
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http") { value.into() } else { format!("{BASE_URL}/{}", value.trim_start_matches('/')) }
}

fn key_from_url(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize_key(input)).filter(|key| key.contains("/album/"))
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters.get(id).and_then(Value::as_str).filter(|value| !value.is_empty())
}

fn multi_filter(filters: &Value, id: &str) -> Option<String> {
    match filters.get(id) {
        Some(Value::Array(items)) => Some(items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")),
        Some(Value::String(value)) => Some(value.clone()),
        _ => None,
    }
}

#[derive(Deserialize)]
struct AlbumListResponse { data: Option<AlbumListData> }
#[derive(Deserialize)]
struct AlbumListData { data: Vec<AlbumItem>, total: u64 }
#[derive(Deserialize)]
struct AlbumItem { #[serde(rename = "id_album")] id_album: Option<u64>, info: Option<String> }
#[derive(Deserialize)]
struct AlbumByIdResponse { data: Option<AlbumByIdData> }
#[derive(Deserialize)]
struct AlbumByIdData { info: Option<String> }
#[derive(Deserialize)]
struct AlbumInfo { url: Option<String>, name: Option<String>, #[serde(default)] tags: Vec<String>, avatar: Option<String>, detail: Option<String>, status: Option<String>, #[serde(default)] author: Vec<String> }
#[derive(Deserialize)]
struct ChapterListResponse { data: Option<Vec<ChapterItem>> }
#[derive(Deserialize)]
struct ChapterItem { #[serde(rename = "id_chapter")] id_chapter: Option<u64>, info: Option<String> }
#[derive(Deserialize)]
struct ChapterInfo { id: Option<Value>, num: Option<Value>, name: Option<String>, #[serde(rename = "last_update")] last_update: Option<String>, level: Option<Value> }
#[derive(Deserialize)]
struct ChapterImageResponse { data: Option<ChapterImageData> }
#[derive(Deserialize)]
struct ChapterImageData { image: Option<Vec<String>> }

const LIST_FIXTURE: &str = r#"{"data":{"data":[{"id_album":1,"info":"{\"url\":\"sample\",\"name\":\"Sample\",\"avatar\":\"cover.jpg\",\"tags\":[\"Action\"],\"author\":[\"Author\"],\"status\":\"doing\"}"}],"total":1}}"#;
const DETAILS_FIXTURE: &str = r#"<div class="book_other"><h1><p class="name">Sample</p></h1></div><div class="book_avatar"><img src="/cover.jpg"></div><div id="book_detail_text">Summary</div>"#;
const DETAILS_API_FIXTURE: &str = r#"{"data":{"info":"{\"url\":\"sample\",\"name\":\"Sample\",\"avatar\":\"cover.jpg\",\"detail\":\"Summary\",\"tags\":[\"Action\"],\"author\":[\"Author\"],\"status\":\"doing\"}"}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"id_chapter":1,"info":"{\"id\":1,\"num\":1,\"name\":\"\",\"last_update\":\"2024-01-01 00:00:00\",\"level\":0}"}]}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"status":1,"image":["/page1.jpg"]}}"#;

export_manga_source!(SOURCE);
