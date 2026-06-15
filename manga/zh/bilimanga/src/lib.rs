use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, url};
use serde_json::Value;

const SOURCE: BiliManga = BiliManga;
const BASE_URL: &str = "https://www.bilimanga.net";

struct BiliManga;

impl MangaSource for BiliManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/top/lastupdate/{page}.html")
        } else {
            format!("{BASE_URL}/top/weekvisit/{page}.html")
        };
        Ok(parse_listing_page(&fetch(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) && query.contains("/detail/") {
            let key = normalize_key(query);
            return Ok(Paged { entries: vec![parse_details(&fetch(query, DETAILS_FIXTURE), &key)], has_next_page: false });
        }
        let target = if query.is_empty() {
            filter_url(&request)
        } else {
            format!("{BASE_URL}/search/{}_{}.html", url::query_escape(query), page(&request))
        };
        Ok(parse_listing_page(&fetch(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/detail/1.html".to_string());
        Ok(parse_details(&fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE), &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/detail/1.html".to_string());
        let manga_id = key.split("/detail/").nth(1).and_then(|rest| rest.split('.').next()).unwrap_or("1");
        let body = fetch(&format!("{BASE_URL}/read/{manga_id}/catalog"), CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/read/1/1.html".to_string());
        Ok(parse_pages(&fetch(&url::join_url(BASE_URL, &key), PAGES_FIXTURE)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) && input.contains("/detail/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult { item: Some(parse_details(&fetch(input, DETAILS_FIXTURE), &key)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}

fn fetch(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_url(request: &Value) -> String {
    let filters = request.get("filters").unwrap_or(&Value::Null);
    let get = |key: &str, default: &str| filters.get(key).and_then(Value::as_str).unwrap_or(default).to_string();
    format!(
        "{BASE_URL}/filter/{}_{}_{}_0_0_0_0_0_{}_0_0_0.html",
        get("sort", "lastupdate"),
        get("theme", "0"),
        get("status", "0"),
        page(request)
    )
}

fn normalize_key(input: &str) -> String {
    let path = input.split(".net").nth(1).unwrap_or(input);
    format!("/{}", path.trim_matches('/'))
}

fn parse_listing_page(body: &str) -> Paged<CatalogItem> {
    let entries = body.split("<a").skip(1).filter(|chunk| chunk.contains("book-layout") || chunk.contains("/detail/")).filter_map(|chunk| {
        let href = html::attr(chunk, "href")?;
        if !href.contains("/detail/") { return None; }
        let key = normalize_key(&href);
        let title = html::attr_after(chunk, "<img", "alt").or_else(|| html::attr(chunk, "title")).unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Manga".to_string()));
        Some(CatalogItem {
            key: key.clone(),
            title,
            cover: html::attr_after(chunk, "<img", "data-src").or_else(|| html::attr_after(chunk, "<img", "src")).map(|image| url::join_url(BASE_URL, &image)),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("zh".to_string()),
            content_rating: Some("safe".to_string()),
            ..CatalogItem::default()
        })
    }).fold(Vec::new(), push_unique);
    Paged { has_next_page: body.contains("#pagelink") || entries.len() >= 50, entries }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let meta = html::text_between(body, "book-meta", "</div>").map(|value| html::strip_tags(&value)).unwrap_or_default();
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(body, "book-title", "</").map(|value| html::strip_tags(&value)).unwrap_or_else(|| "Manga".to_string()),
        cover: html::attr_after(body, "book-cover", "src").or_else(|| html::attr_after(body, "<img", "src")).map(|image| url::join_url(BASE_URL, &image)),
        authors: html::text_between(body, "authorname", "</").map(|value| vec![html::strip_tags(&value)]).unwrap_or_default(),
        artists: html::text_between(body, "illname", "</").map(|value| vec![html::strip_tags(&value)]).unwrap_or_default(),
        description: html::text_between(body, "bookSummary", "</").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        tags: body.split("tag-small").skip(1).filter_map(|chunk| html::text_between(chunk, ">", "</")).map(|value| html::strip_tags(&value)).collect(),
        status: if meta.contains("連載中") { ItemStatus::Ongoing } else if meta.contains("已完結") { ItemStatus::Completed } else { ItemStatus::Unknown },
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("zh".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let date = first_ymd(body).and_then(|value| dates::parse_ymd(&value));
    let mut chapters = body.split("<a").skip(1).filter(|chunk| chunk.contains("chapter-li-a") || chunk.contains("/read/")).filter_map(|chunk| {
        let href = html::attr(chunk, "href")?;
        let key = normalize_key(&href);
        let title = html::strip_tags(chunk);
        Some(MangaChapter {
            key: key.clone(),
            title: Some(if title.is_empty() { "Chapter".to_string() } else { title }),
            scanlators: volume_name_before(body, chunk).into_iter().collect(),
            date_uploaded: date,
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        })
    }).collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<").filter(|chunk| chunk.contains("imagecontent") || chunk.contains("data-src")).filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))).enumerate().map(|(index, image)| MangaPage {
        content: PageContent::Url { url: url::join_url(BASE_URL, &image), context: None },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }).collect()
}

fn first_ymd(body: &str) -> Option<String> {
    for token in body.split(|ch: char| !(ch.is_ascii_digit() || ch == '-')) {
        if token.len() >= 8 && dates::parse_ymd(token).is_some() {
            return Some(token.to_string());
        }
    }
    None
}

fn volume_name_before(body: &str, chunk: &str) -> Option<String> {
    let index = body.find(chunk)?;
    body[..index].rsplit("chapter-bar").next().map(html::strip_tags).filter(|value| !value.is_empty())
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<a class="book-layout" href="/detail/1.html"><img data-src="/cover.jpg" alt="Sample"></a>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="book-title">Sample</h1><img class="book-cover" src="/cover.jpg"><div id="bookSummary"><content>Sample description.</content></div><div class="book-meta"><em>連載中</em></div>"#;
const CHAPTERS_FIXTURE: &str = r#"<div class="chapter-sub-title">2024-01-01</div><a class="chapter-li-a" href="/read/1/1.html">Chapter 1</a>"#;
const PAGES_FIXTURE: &str = r#"<img class="imagecontent" data-src="https://www.bilimanga.net/page-1.jpg">"#;
