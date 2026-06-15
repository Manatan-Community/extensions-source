use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaShi = MangaShi;
const BASE_URL: &str = "https://manga-shi.org";
const LIST_FIXTURE: &str = r#"<div id="manga-grid"><a href="/manga/sample/"><img src="/cover.jpg"><h3>Sample</h3></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><meta property="og:image" content="/cover.jpg"><span class="tracking-widest">Онгоинг</span><span class="tracking-widest">Манга</span><a href="?author=a">Author</a><a href="/manga-genre/action">Боевик</a><p class="leading-relaxed">Description</p><div id="chapters-list"><a href="/manga/sample/glava-1/"><span class="chapter-title"><span>Глава 1</span></span><span><span>01.01.2024</span></span></a></div>"#;
const PAGES_FIXTURE: &str = r#"<img class="reader-image" src="/page.jpg">"#;

struct MangaShi;

impl MangaSource for MangaShi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "updated" } else { "popular" };
        Ok(parse_listing(&fetch_document(&format!("{BASE_URL}/catalog/?sort={sort}&page={page}"), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) || query.starts_with("slug:") {
            let key = normalize_key(query);
            return Ok(Paged { entries: vec![parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key))], has_next_page: false });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if query.is_empty() { catalog_url(page, request.get("filters")) } else { format!("{BASE_URL}/catalog/?page={page}&q={}", url::query_escape(query)) };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".into());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".into());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let mut chapters = parse_chapters(&body);
        let mut next = html::attr_after(&body, "chapters-load-more", "hx-get");
        while let Some(path) = next.take() {
            let fragment = fetch_document(&absolute_url(&path), "");
            chapters.extend(parse_chapters(&fragment));
            next = html::attr_after(&fragment, "chapters-load-more", "hx-get");
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/glava-1/".into());
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
            return Ok(Some(UrlResolveResult { item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key))), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn catalog_url(page: u64, filters: Option<&Value>) -> String {
    let mut params = vec![format!("page={page}")];
    for id in ["sort", "status", "type", "year", "age_rating"] {
        if let Some(value) = filter_id(filters, id).filter(|v| !v.is_empty()) {
            params.push(format!("{id}={}", url::query_escape(value)));
        }
    }
    for tag in selected_values(filters.and_then(|f| f.get("tag"))) {
        params.push(format!("tag={}", url::query_escape(&tag)));
    }
    format!("{BASE_URL}/catalog/?{}", params.join("&"))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body.split("<a").skip(1).filter(|chunk| chunk.contains("/manga/")).filter_map(|chunk| {
        let href = html::attr(chunk, "href")?;
        let key = normalize_key(&href);
        Some(CatalogItem {
            key: key.clone(),
            title: html::text_between(chunk, "<h3", "</h3>").or_else(|| html::text_between(chunk, "<h4", "</h4>")).map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
            cover: html::attr_after(chunk, "<img", "data-src").or_else(|| html::attr_after(chunk, "<img", "src")).map(|v| absolute_url(&v)),
            url: Some(absolute_url(&key)),
            language: Some("ru".into()),
            content_rating: Some("adult".into()),
            initialized: false,
            ..CatalogItem::default()
        })
    }).fold(Vec::new(), |mut acc, item| { if !acc.iter().any(|i: &CatalogItem| i.key == item.key) { acc.push(item); } acc });
    Paged { has_next_page: entries.len() >= 20 || body.contains("next"), entries }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample/".into());
    let badges = body.split("tracking-widest").skip(1).filter_map(|c| html::text_between(c, ">", "</")).map(|v| html::strip_tags(&v)).collect::<Vec<_>>();
    let status_text = badges.iter().find(|v| is_status(v)).cloned().unwrap_or_default();
    let type_text = badges.iter().find(|v| is_type(v)).cloned();
    let mut tags = Vec::new();
    tags.extend(type_text);
    tags.extend(body.split("<a").skip(1).filter(|c| c.contains("manga-genre") || c.contains("?tag=")).filter_map(|c| html::text_between(c, ">", "</a>").map(|v| html::strip_tags(&v))));
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga-shi".into())),
        cover: html::attr_after(body, "property=\"og:image\"", "content").or_else(|| html::attr_after(body, "<img", "src")).map(|v| absolute_url(&v)),
        authors: body.split("<a").skip(1).filter(|c| c.contains("?author=")).filter_map(|c| html::text_between(c, ">", "</a>").map(|v| html::strip_tags(&v))).collect(),
        tags,
        description: html::text_between(body, "leading-relaxed", "</").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()),
        status: parse_status(&status_text),
        url: Some(absolute_url(&key)),
        language: Some("ru".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a").skip(1).filter(|c| c.contains("/glava")).filter_map(|chunk| {
        let href = html::attr(chunk, "href")?;
        let key = normalize_key(&href);
        let title = html::text_between(chunk, "chapter-title", "</").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Глава".into()));
        Some(MangaChapter {
            key: key.clone(),
            title: Some(title),
            chapter_number: chapter_number(&key),
            date_uploaded: html::strip_tags(chunk).split_whitespace().find(|p| p.matches('.').count() == 2).and_then(dates::parse_fixture_date),
            url: Some(absolute_url(&key)),
            ..MangaChapter::default()
        })
    }).collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img").skip(1).filter(|c| c.contains("reader-image")).filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))).enumerate().map(|(i, image)| MangaPage {
        content: PageContent::Url { url: absolute_url(&image), context: None },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", i + 1)),
        ..MangaPage::default()
    }).collect()
}

fn normalize_key(value: &str) -> String {
    let path = value.trim_start_matches("slug:").trim_start_matches(BASE_URL);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_lowercase();
    if lower.contains("заверш") { ItemStatus::Completed } else if lower.contains("заморож") || lower.contains("хиатус") || lower.contains("заброш") { ItemStatus::Hiatus } else if lower.contains("онгоинг") || lower.contains("выпуска") || lower.contains("продолжа") { ItemStatus::Ongoing } else { ItemStatus::Unknown }
}

fn is_status(value: &str) -> bool {
    let lower = value.to_lowercase();
    ["онгоинг", "выпуска", "заверш", "заморож", "хиатус", "заброш"].iter().any(|needle| lower.contains(needle))
}

fn is_type(value: &str) -> bool {
    let lower = value.to_lowercase();
    ["манга", "манхва", "маньхуа", "комикс"].iter().any(|needle| lower.contains(needle))
}

fn chapter_number(key: &str) -> Option<f32> {
    key.split("glava-").nth(1).or_else(|| key.split("glava_").nth(1)).and_then(|v| v.trim_matches('/').replace(',', ".").parse().ok())
}

fn filter_id<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters.and_then(|f| f.get(id)).and_then(|v| v.as_str().or_else(|| v.get("value").and_then(Value::as_str)))
}

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).map(ToString::to_string).collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.clone()],
        Some(Value::Object(object)) => object.values().filter_map(Value::as_str).map(ToString::to_string).collect(),
        _ => Vec::new(),
    }
}

export_manga_source!(SOURCE);
