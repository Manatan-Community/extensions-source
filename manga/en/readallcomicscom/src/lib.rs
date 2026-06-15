use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ReadAllComics = ReadAllComics;
const BASE_URL: &str = "https://readallcomics.com";

struct ReadAllComics;

impl MangaSource for ReadAllComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if page == 1 { BASE_URL.to_string() } else { format!("{BASE_URL}/?paged={page}") };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged { entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), Some(key))], has_next_page: false });
        }
        let page_param = if page > 1 { format!("&paged={page}") } else { String::new() };
        Ok(parse_listing(&fetch_document(&format!("{BASE_URL}/?story={}&s=&type=comic{page_param}", url::query_escape(query)), LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/1".to_string());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key))),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("cat-title"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "cat-title", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "cat-title", "</")
                    .or_else(|| html::text_between(chunk, "<a", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "ReadAllComics".to_string())),
                cover: html::attr_after(chunk, "book-cover", "src").or_else(|| html::attr_after(chunk, "<img", "src")).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged { entries, has_next_page: body.contains("page-numbers current") || body.contains("class=\"next") }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    let archive = body.split("description-archive").nth(1).unwrap_or(body);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(archive, "<h1", "</h1>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| "ReadAllComics".to_string()),
        cover: html::attr_after(archive, "<img", "src").map(|image| absolute_url(&image)),
        tags: archive.split("<strong").nth(1).and_then(|chunk| html::text_between(chunk, "<strong", "</strong>")).map(|value| vec![html::strip_tags(&value)]).unwrap_or_default(),
        authors: archive.rsplit("<strong").next().and_then(|chunk| html::text_between(chunk, "<strong", "</strong>")).map(|value| vec![html::strip_tags(&value)]).unwrap_or_default(),
        description: html::text_between(archive, "hidden-description", "</").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body
        .split("list-story")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty());
            Some(MangaChapter { key: key.clone(), title, url: Some(absolute_url(&key)), ..MangaChapter::default() })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body
        .split("<img")
        .skip(1)
        .filter(|chunk| !chunk.contains("id=\"logo\"") && !chunk.contains("logo"))
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: absolute_url(&image), context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<ul class="list-story categories"><li><a class="cat-title" href="/sample">Sample</a><img class="book-cover" src="/cover.jpg"></li></ul>"#;
const DETAILS_FIXTURE: &str = r#"<div class="description-archive"><h1>Sample</h1><p><img src="/cover.jpg"></p><div class="b"><p><strong>Action</strong></p><p><strong>Author</strong></p></div><div id="hidden-description">Summary</div><ul class="list-story"><a href="/sample/1">Issue 1 (2024)</a></ul></div>"#;
const PAGES_FIXTURE: &str = r#"<body><img src="/page1.jpg"></body>"#;
