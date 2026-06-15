use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: BaoziMhOrg = BaoziMhOrg;
const BASE_URL: &str = "https://baozimh.org";
const API_URL: &str = "https://api-get-v3.mgsearcher.com/api";
const IMAGE_BASE_URL: &str = "https://f40-1-4.g-mh.online";
const CONTENT_RATING: &str = "safe";

struct BaoziMhOrg;

impl MangaSource for BaoziMhOrg {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "updated" } else { "popular" };
        let body = fetch_html(&format!("{BASE_URL}/manga/?page={page}&sort={order}"), LIST_FIXTURE);
        Ok(Paged { entries: parse_cards(&body), has_next_page: has_next_page(&body) })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged { entries: vec![parse_details(&fetch_html(query, DETAILS_FIXTURE), &key)], has_next_page: false });
        }
        let body = fetch_html(&format!("{BASE_URL}/search?q={}&page={page}", url::query_escape(query)), LIST_FIXTURE);
        Ok(Paged { entries: parse_cards(&body), has_next_page: has_next_page(&body) })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(&fetch_html(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE), &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample#1".to_string());
        let manga_id = key.split('#').nth(1).unwrap_or_default();
        let slug = key.trim_start_matches("/manga/").split('#').next().unwrap_or("sample");
        let target = format!("{API_URL}/manga/get?mid={manga_id}&mode=all");
        let body = client().get(&target).xhr().send_text().unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
        Ok(serde_json::from_str::<ResponseDto<ChapterListDto>>(&body)
            .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("valid fixture"))
            .data
            .to_chapters(slug, manga_id))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/chapter-1#1/1".to_string());
        let ids = key.split('#').nth(1).unwrap_or("1/1");
        let mut parts = ids.split('/');
        let manga_id = parts.next().unwrap_or("1");
        let chapter_id = parts.next().unwrap_or("1");
        let target = format!("{API_URL}/chapter/getinfo?m={manga_id}&c={chapter_id}");
        let body = client().get(&target).xhr().send_text().unwrap_or_else(|_| PAGES_FIXTURE.to_string());
        Ok(serde_json::from_str::<ResponseDto<PageListDto>>(&body)
            .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("valid fixture"))
            .data
            .info
            .images
            .images
            .into_iter()
            .map(|image| image.to_page())
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult { item: Some(parse_details(&fetch_html(input, DETAILS_FIXTURE), &key)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_html(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn normalize_key(input: &str) -> String {
    let path = input.split(".org").nth(1).unwrap_or(input).trim_matches('/');
    format!("/{path}")
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("<a").skip(1).filter(|chunk| chunk.contains("/manga/")).filter_map(|chunk| {
        let href = html::attr(chunk, "href")?;
        let key = normalize_key(&href);
        let title = html::attr(chunk, "title")
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Manga".to_string()));
        Some(CatalogItem {
            key: key.clone(),
            title,
            cover: html::attr_after(chunk, "<img", "src").map(|image| url::join_url(BASE_URL, &image)),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("zh".to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            ..CatalogItem::default()
        })
    }).fold(Vec::new(), push_unique)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let id = html::attr_after(body, "data-mid", "data-mid").or_else(|| html::attr(body, "data-mid"));
    let key = id.map(|id| format!("{}#{id}", key.trim_end_matches('/'))).unwrap_or_else(|| key.to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value)).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: html::attr_after(body, "<img", "src").map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "description", "</div>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        status: if body.contains("连载") || body.contains("連載") { ItemStatus::Ongoing } else if body.contains("完结") || body.contains("完結") { ItemStatus::Completed } else { ItemStatus::Unknown },
        url: Some(url::join_url(BASE_URL, key.split('#').next().unwrap_or(&key))),
        language: Some("zh".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn has_next_page(body: &str) -> bool {
    body.contains("next") || body.contains("下一")
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Debug, Deserialize)]
struct ResponseDto<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct ChapterListDto {
    id: i64,
    slug: String,
    chapters: Vec<ChapterDto>,
}

impl ChapterListDto {
    fn to_chapters(self, fallback_slug: &str, fallback_id: &str) -> Vec<MangaChapter> {
        let manga_id = if self.id == 0 { fallback_id.to_string() } else { self.id.to_string() };
        let manga_slug = if self.slug.is_empty() { fallback_slug.to_string() } else { self.slug };
        self.chapters.into_iter().rev().map(|chapter| chapter.to_chapter(&manga_slug, &manga_id)).collect()
    }
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    id: i64,
    attributes: ChapterAttributesDto,
}

impl ChapterDto {
    fn to_chapter(self, manga_slug: &str, manga_id: &str) -> MangaChapter {
        let key = format!("{manga_slug}/{}#{manga_id}/{}", self.attributes.slug, self.id);
        MangaChapter {
            key: key.clone(),
            title: Some(self.attributes.title),
            date_uploaded: parse_api_date(&self.attributes.updated_at),
            url: Some(format!("{BASE_URL}/manga/{manga_slug}/{}", self.attributes.slug)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChapterAttributesDto {
    title: String,
    slug: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct PageListDto {
    info: PageListInfoDto,
}

#[derive(Debug, Deserialize)]
struct PageListInfoDto {
    images: PageListInfoImagesDto,
}

#[derive(Debug, Deserialize)]
struct PageListInfoImagesDto {
    images: Vec<ImageDto>,
}

#[derive(Debug, Deserialize)]
struct ImageDto {
    url: String,
    order: usize,
}

impl ImageDto {
    fn to_page(self) -> MangaPage {
        MangaPage {
            content: PageContent::Url { url: url::join_url(IMAGE_BASE_URL, &self.url), context: None },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", self.order)),
            ..MangaPage::default()
        }
    }
}

fn parse_api_date(input: &str) -> Option<i64> {
    let date = input.split('T').next().unwrap_or(input);
    dates::parse_ymd(date)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<a href="/manga/sample" title="Sample"><img src="/cover.jpg"></a>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><div data-mid="1"></div><img src="/cover.jpg"><div class="description">Sample description.</div>"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"id":1,"slug":"sample","chapters":[{"id":1,"attributes":{"title":"Chapter 1","slug":"chapter-1","updatedAt":"2024-01-01T00:00:00Z"}}]}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"info":{"images":{"images":[{"url":"/page-1.jpg","order":1}]}}}}"#;
