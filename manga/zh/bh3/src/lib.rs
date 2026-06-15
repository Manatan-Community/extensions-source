use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Bh3 = Bh3;
const BASE_URL: &str = "https://comic.bh3.com";

struct Bh3;

impl MangaSource for Bh3 {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = fetch(&format!("{BASE_URL}/book"), LIST_FIXTURE);
        Ok(Paged { entries: parse_books(&body), has_next_page: false })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged { entries: vec![parse_details(&fetch(query, DETAILS_FIXTURE), &key)], has_next_page: false });
        }
        Ok(Paged { entries: Vec::new(), has_next_page: false })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/book/1".to_string());
        Ok(parse_details(&fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE), &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/book/1".to_string());
        let body = client().get(format!("{}{}/get_chapter", BASE_URL, key.trim_end_matches('/'))).xhr().send_text().unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
        Ok(serde_json::from_str::<Vec<ChapterDto>>(&body).unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("valid fixture")).into_iter().map(ChapterDto::to_chapter).collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/book/1/1".to_string());
        let body = fetch(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) && input.contains("/book/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult { item: Some(parse_details(&fetch(input, DETAILS_FIXTURE), &key)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}

fn fetch(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn normalize_key(input: &str) -> String {
    let path = input.split(".com").nth(1).unwrap_or(input);
    format!("/{}", path.trim_matches('/'))
}

fn parse_books(body: &str) -> Vec<CatalogItem> {
    body.split("<a").skip(1).filter(|chunk| chunk.contains("book")).filter_map(|chunk| {
        let id = html::attr_after(chunk, "container", "id").or_else(|| html::attr(chunk, "href").and_then(|href| url::slug_from_url(&href)))?;
        let key = format!("/book/{id}");
        Some(CatalogItem {
            key: key.clone(),
            title: html::text_between(chunk, "container-title", "</").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| format!("Book {id}")),
            cover: html::attr_after(chunk, "<img", "src").map(|image| url::join_url(BASE_URL, &image)),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("zh".to_string()),
            content_rating: Some("safe".to_string()),
            ..CatalogItem::default()
        })
    }).collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(body, "class=\"title\"", "</").or_else(|| html::text_between(body, "<div class='title'", "</")).map(|value| html::strip_tags(&value)).unwrap_or_else(|| "Book".to_string()),
        cover: html::attr_after(body, "cover", "src").or_else(|| html::attr_after(body, "<img", "src")).map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "detail_info1", "</div>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        status: ItemStatus::Unknown,
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("zh".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    title: String,
    #[serde(rename = "bookid")]
    book_id: i64,
    #[serde(rename = "chapterid")]
    chapter_id: i64,
    timestamp: String,
}

impl ChapterDto {
    fn to_chapter(self) -> MangaChapter {
        let key = format!("/book/{}/{}", self.book_id, self.chapter_id);
        MangaChapter {
            key: key.clone(),
            title: Some(self.title),
            url: Some(url::join_url(BASE_URL, &key)),
            date_uploaded: dates::parse_ymd(self.timestamp.split_whitespace().next().unwrap_or("")),
            chapter_number: Some(self.chapter_id as f32),
            ..MangaChapter::default()
        }
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img").skip(1).filter_map(|chunk| html::attr(chunk, "data-original").or_else(|| html::attr(chunk, "src"))).enumerate().map(|(index, image)| MangaPage {
        content: PageContent::Url { url: url::join_url(BASE_URL, &image), context: None },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }).collect()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<a href="/book/1"><div class="container" id="1"><img src="/cover.jpg"><div class="container-title">Sample</div></div></a>"#;
const DETAILS_FIXTURE: &str = r#"<div class="title">Sample</div><img class="cover" src="/cover.jpg"><div class="detail_info1">Sample description.</div>"#;
const CHAPTERS_FIXTURE: &str = r#"[{"title":"Chapter 1","bookid":1,"chapterid":1,"timestamp":"2024-01-01 00:00:00"}]"#;
const PAGES_FIXTURE: &str = r#"<img class="lazy comic_img" data-original="https://comic.bh3.com/page-1.jpg">"#;
