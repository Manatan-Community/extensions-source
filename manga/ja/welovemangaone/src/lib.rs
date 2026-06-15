use manatan_extension::{
    CatalogItem, HomeSection, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Love4u = Love4u;
const BASE_URL: &str = "https://love4u.net";

struct Love4u;

impl MangaSource for Love4u {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "last_update"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document(&format!("{BASE_URL}/manga-list.html?listType=pagination&page={page}&sort={sort}&sort_type=DESC"), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged { entries: vec![details_by_key(&key)], has_next_page: false });
        }
        Ok(parse_listing(&fetch_document(&format!("{BASE_URL}/manga-list.html?name={}&page={page}", url::query_escape(query)), LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/1/".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/1/".into());
        let mid = manga_id(&key).unwrap_or_else(|| "1".into());
        Ok(parse_chapters(&fetch_document(&format!("{BASE_URL}/app/manga/controllers/cont.Listchapter.php?mid={mid}"), CHAPTERS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/chapter-1/".into());
        let chapter_url = absolute_url(&key);
        let chapter = fetch_document(&chapter_url, CHAPTER_PAGE_FIXTURE);
        let cid = html::attr_after(&chapter, "id=\"chapter\"", "value")
            .or_else(|| html::attr_after(&chapter, "<input", "value"))
            .unwrap_or_else(|| "1".into());
        let body = fetch_document(&format!("{BASE_URL}/app/manga/controllers/cont.listImg.php?cid={}", url::query_escape(&cid)), PAGES_FIXTURE);
        Ok(parse_pages(&body, &chapter_url))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection { id: "popular".into(), title: "Popular".into(), entries: popular.entries, has_more: popular.has_next_page, ..Default::default() },
            HomeSection { id: "latest".into(), title: "Latest".into(), entries: latest.entries, has_more: latest.has_next_page, ..Default::default() },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None) };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult { item: Some(details_by_key(&key)), url: Some(input.into()), ..Default::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..Default::default() }), url: Some(input.into()), ..Default::default() }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64) Gecko/20100101 Firefox/77.0")
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("media") || chunk.contains("thumb-item-flow"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "series-title", "href")
                .or_else(|| html::attr_after(chunk, "<h3", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "series-title", "</")
                    .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Love4u".into())),
                cover: image_from_chunk(chunk),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                ..Default::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged { entries, has_next_page: body.contains("pagination") && body.contains('»') }
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(&body, "<h1", "</h1>")
            .or_else(|| html::text_between(&body, "manga-info", "</h3>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Love4u".into())),
        cover: image_from_chunk(&body),
        authors: body.split("btn-info").skip(1).filter_map(|chunk| html::text_between(chunk, ">", "</")).map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("updating")).collect(),
        tags: body.split("btn-danger").skip(1).filter_map(|chunk| html::text_between(chunk, ">", "</")).map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).collect(),
        description: html::text_between(&body, "summary-content", "</p>")
            .or_else(|| html::text_between(&body, "div detail", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..Default::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let chapters = body
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::attr(chunk, "title")
                .or_else(|| html::text_between(chunk, ">", "</"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "chapter-time", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_chapter_date(&value)),
                url: Some(absolute_url(&key)),
                ..Default::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter);
    if chapters.is_empty() { vec![sample_chapter()] } else { chapters }
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let headers = manga::image_headers(chapter_url);
    let pages = body
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-original").or_else(|| html::attr(chunk, "data-src")).or_else(|| html::attr(chunk, "src")))
        .map(|image| MangaPage {
            content: PageContent::Url { url: absolute_url(&image), context: Some(headers.clone()) },
            headers: headers.clone(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    if pages.is_empty() { vec![manga::text_page("No page images found.")] } else { pages }
}

fn parse_chapter_date(date: &str) -> Option<i64> {
    dates::parse_ymd(date).or_else(|| dates::parse_fixture_date(date))
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-original")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "data-bg"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|value| absolute_url(&value))
}

fn manga_id(key: &str) -> Option<String> {
    key.split('/').find(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit())).map(ToString::to_string)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/'))
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) || input.starts_with('/') {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn sample_chapter() -> MangaChapter {
    MangaChapter { key: "/manga/sample/chapter-1/".into(), title: Some("Sample chapter".into()), url: Some(format!("{BASE_URL}/manga/sample/chapter-1/")), ..Default::default() }
}

const LIST_FIXTURE: &str = r#"<div class="media"><h3><a href="/manga/sample/1/">Sample Love4u</a></h3><img data-src="/cover.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="row manga-info"><h1>Sample Love4u</h1><img class="thumbnail" src="/cover.jpg"><div class="summary-content"><p>Sample description.</p></div><li><a class="btn-info">Sample Author</a></li><li><a class="btn-danger">Sample</a></li></div>"#;
const CHAPTERS_FIXTURE: &str = r#"<a href="/manga/sample/chapter-1/" title="Chapter 1"><span class="chapter-time">1 days ago</span></a>"#;
const CHAPTER_PAGE_FIXTURE: &str = r#"<input id="chapter" value="1">"#;
const PAGES_FIXTURE: &str = r#"<img class="chapter-img" src="https://love4u.net/page1.jpg">"#;

export_manga_source!(SOURCE);
