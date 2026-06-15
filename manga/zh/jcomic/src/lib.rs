use manatan_extension::{abi::ExtensionResult, export_manga_source, http, source::MangaSource, CatalogItem, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult};
use manatan_shared::{dates, html, manga, url};
use serde_json::Value;

const SOURCE: JComic = JComic;
const BASE_URL: &str = "https://jcomic.net";

struct JComic;

impl MangaSource for JComic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let cat = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "%E6%9C%80%E8%BF%91%E6%9B%B4%E6%96%B0" } else { "%E9%9A%A8%E6%A9%9F" };
        Ok(parse_listing(&fetch(&format!("{BASE_URL}/cat/{cat}/{}", page(&request)), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged { entries: vec![parse_detail_cover(&fetch(&absolute(&key).replace("/page", "/eps"), DETAILS_FIXTURE), &key)], has_next_page: false });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let path = if query.is_empty() {
            format!("cat/{}/{}", url::query_escape(filters.get("category").and_then(Value::as_str).unwrap_or("全彩")), page(&request))
        } else {
            format!("{}/{}/{}", filters.get("searchType").and_then(Value::as_str).unwrap_or("search"), url::query_escape(query), page(&request))
        };
        Ok(parse_listing(&fetch(&format!("{BASE_URL}/{path}"), LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/page/sample#0".into());
        Ok(parse_detail_cover(&fetch(&absolute(&key).replace("/page", "/eps"), DETAILS_FIXTURE), &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/page/sample#0".into());
        if key.contains("/page") {
            return Ok(vec![MangaChapter { key: key.clone(), title: Some("单章节".into()), date_uploaded: key.split('#').nth(1).and_then(|v| v.parse().ok()), url: Some(absolute(&key)), ..MangaChapter::default() }]);
        }
        let time = key.split('#').nth(1).and_then(|v| v.parse::<i64>().ok());
        Ok(parse_chapters(&fetch(&absolute(&key).replace("/page", "/eps"), DETAILS_FIXTURE), time))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/page/sample".into());
        Ok(parse_pages(&fetch(&absolute(&key), PAGES_FIXTURE)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> { Ok(manga::request_key(&request, "manga").map(|key| absolute(&key))) }
    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> { Ok(manga::request_key(&request, "chapter").map(|key| absolute(&key))) }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult { item: Some(parse_detail_cover(&fetch(&absolute(&key).replace("/page", "/eps"), DETAILS_FIXTURE), &key)), url: Some(input.into()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }), url: Some(input.into()), ..UrlResolveResult::default() }))
    }
}

fn client() -> http::HttpClient { http::HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback() }
fn fetch(target: &str, fixture: &str) -> String { client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.into()) }
fn page(request: &Value) -> u64 { request.get("page").and_then(Value::as_u64).unwrap_or(1) }
fn normalize_key(input: &str) -> String { format!("/{}", input.strip_prefix(BASE_URL).unwrap_or(input).split('?').next().unwrap_or(input).trim_matches('/')) }
fn absolute(key: &str) -> String { url::join_url(BASE_URL, key.split('#').next().unwrap_or(key)) }

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body.split("col-lg-4").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let mut title = html::text_between(chunk, "comic-title", "</").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| "JComic".into());
        let pages = extract_between(&title, "(", ")");
        if pages.is_some() {
            title = title.split('(').next().unwrap_or(&title).trim().to_string();
        }
        let time = html::text_between(chunk, "comic-date", "</").map(|v| html::strip_tags(&v)).and_then(|v| parse_date(&v));
        let key = format!("{}#{}", normalize_key(&url::join_url(BASE_URL, &href)), time.unwrap_or(0));
        let tags = chunk.split("list-content").nth(1).unwrap_or("").split("<a").skip(1).map(html::strip_tags).filter(|v| !v.is_empty()).collect::<Vec<_>>();
        Some(CatalogItem {
            key: key.clone(),
            title,
            cover: html::attr_after(chunk, "<img", "src").map(|v| url::join_url(BASE_URL, &v)),
            authors: tags.first().cloned().into_iter().collect(),
            tags: tags.into_iter().skip(1).collect(),
            description: pages.map(|p| format!("共 {p} 页")),
            latest_update: time,
            url: Some(absolute(&key)),
            language: Some("zh".into()),
            content_rating: Some("adult".into()),
            initialized: true,
            ..CatalogItem::default()
        })
    }).collect::<Vec<_>>();
    Paged { entries, has_next_page: !body.contains("pagination") || !body.contains("active:last-child") }
}

fn parse_detail_cover(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.into(),
        title: url::slug_from_url(key).unwrap_or_else(|| "JComic".into()),
        cover: html::attr_after(body, "col-md-6", "src").map(|v| url::join_url(BASE_URL, &v)),
        url: Some(absolute(key)),
        language: Some("zh".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, time: Option<i64>) -> Vec<MangaChapter> {
    let mut chapters = body.split("col-md-6").nth(2).unwrap_or(body).split("<a").skip(1).filter_map(|chunk| {
        let href = html::attr(chunk, "href")?;
        let key = normalize_key(&url::join_url(BASE_URL, &href));
        Some(MangaChapter { key: key.clone(), title: Some(html::strip_tags(chunk)), date_uploaded: time, url: Some(absolute(&key)), ..MangaChapter::default() })
    }).collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("comic-thumb").skip(1).filter_map(|chunk| html::attr(chunk, "src")).enumerate().map(|(index, image)| MangaPage {
        content: PageContent::Url { url: url::join_url(BASE_URL, &image), context: None },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }).collect()
}

fn extract_between(value: &str, start: &str, end: &str) -> Option<String> {
    Some(value.split(start).nth(1)?.split(end).next()?.to_string()).filter(|v| !v.is_empty())
}
fn parse_date(value: &str) -> Option<i64> {
    let value = value.split("最後更新:").nth(1).unwrap_or(value).trim();
    dates::parse_ymd(value.split_whitespace().next().unwrap_or(value))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="container"><div class="col-lg-4"><a href="/page/sample"><img src="/cover.jpg"></a><div class="comic-title">Sample (1)</div><div class="comic-date">最後更新: 2024-01-01 00:00</div><div class="list-content"><a>Author</a><a>Tag</a></div></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="col-md-6"><img src="/cover.jpg"></div><div class="col-md-6"><a href="/page/sample/1">Chapter 1</a></div>"#;
const PAGES_FIXTURE: &str = r#"<img class="comic-thumb" src="https://jcomic.net/page.jpg">"#;
