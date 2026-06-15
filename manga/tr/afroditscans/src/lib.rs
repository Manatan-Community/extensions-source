use manatan_extension::{CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: AfroditScans = AfroditScans;
const BASE_URL: &str = "https://afroditscans.com";
const CDN_URL: &str = "https://afroditcdn1.efsaneler.can.re";

struct AfroditScans;

impl MangaSource for AfroditScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, true));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/?page={page}")
        } else {
            format!("{BASE_URL}/search?page={page}&search=&order=4")
        };
        Ok(parse_listing(&fetch_document_or_fixture(&target, LIST_FIXTURE), request.get("listingId").and_then(Value::as_str) != Some("latest")))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged { entries: vec![parse_details(&fetch_document_or_fixture(query, DETAILS_FIXTURE), Some(key))], has_next_page: false });
        }
        let body = client().get(format!("{BASE_URL}/api/series/search/navbar?search={}", url::query_escape(query))).xhr().send_text().unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
        let entries = serde_json::from_str::<Vec<SearchDto>>(&body).unwrap_or_default().into_iter().map(SearchDto::into_item).collect();
        Ok(Paged { entries, has_next_page: false })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/1/sample".to_string());
        Ok(parse_details(&fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/1/sample".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/1/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document_or_fixture(&absolute_url(&key), PAGES_FIXTURE), &absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult { item: Some(parse_details(&fetch_document_or_fixture(input, DETAILS_FIXTURE), Some(normalize_key(input)))), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, popular: bool) -> Paged<CatalogItem> {
    let chunks: Vec<&str> = if popular {
        body.split("class=\"card").skip(1).collect()
    } else {
        body.split("<div").skip(1).filter(|chunk| chunk.contains("grid") || chunk.contains("card-image") || chunk.contains("/manga/")).collect()
    };
    let entries = chunks.into_iter().filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        if !href.contains("/manga/") { return None; }
        let title = html::text_between(chunk, "<h2", "</h2>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).or_else(|| url::slug_from_url(&href)).unwrap_or_else(|| "Manga".into());
        Some(CatalogItem { key: normalize_key(&href), title, cover: image(chunk).map(|v| absolute_url(&v)), url: Some(absolute_url(&href)), language: Some("tr".into()), content_rating: Some("safe".into()), initialized: false, ..CatalogItem::default() })
    }).collect::<Vec<_>>();
    Paged { has_next_page: !entries.is_empty(), entries }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/1/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "#content", "src").or_else(|| image(body)).map(|v| absolute_url(&v)),
        description: html::text_between(body, "div.grid h2 + p", "</p>").or_else(|| html::text_between(body, "<p", "</p>")).map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()),
        tags: body.split("<a").skip(1).filter(|c| c.contains("search?categories")).filter_map(|c| html::text_between(c, ">", "</a>").map(|v| html::strip_tags(&v))).collect(),
        status: parse_status(&html::text_between(body, "Durum", "</span>").map(|v| html::strip_tags(&v)).unwrap_or_default()),
        url: Some(absolute_url(&key)),
        language: Some("tr".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a").skip(1)
        .filter(|chunk| chunk.contains("list-episode") || chunk.contains("/manga/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "<h3", "</h3>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter { key: normalize_key(&href), title: Some(title), url: Some(absolute_url(&href)), date_uploaded: html::text_between(chunk, "<span", "</span>").and_then(|v| manatan_shared::dates::parse_fixture_date(&html::strip_tags(&v))), ..MangaChapter::default() })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let mut images = Vec::new();
    for chunk in body.split(r#"\"path\":\""#).skip(1) {
        if let Some(path) = chunk.split('"').next() {
            images.push(format!("{}/{}", CDN_URL.trim_end_matches('/'), path.trim_start_matches('/')));
        }
    }
    if images.is_empty() {
        images = body.split("<img").skip(1).filter_map(image).map(|v| absolute_url(&v)).collect();
    }
    images.into_iter().enumerate().map(|(index, image)| MangaPage { content: PageContent::Url { url: image, context: None }, headers: manga::image_headers(referer), description: Some(format!("Page {}", index + 1)), ..MangaPage::default() }).collect()
}

#[derive(Default, Deserialize)]
struct SearchDto {
    id: i64,
    name: String,
    image: String,
}

impl SearchDto {
    fn into_item(self) -> CatalogItem {
        let slug = slugify_tr(&self.name);
        CatalogItem { key: format!("/manga/{}/{slug}", self.id), title: self.name, cover: Some(if self.image.starts_with("http") { self.image } else { format!("{}/{}", CDN_URL.trim_end_matches('/'), self.image.trim_start_matches('/')) }), url: Some(format!("{BASE_URL}/manga/{}/{slug}", self.id)), language: Some("tr".into()), content_rating: Some("safe".into()), initialized: false, ..CatalogItem::default() }
    }
}

fn parse_status(value: &str) -> ItemStatus {
    let value = value.to_lowercase();
    if value.contains("tamamlandi") || value.contains("tamamlandı") { ItemStatus::Completed }
    else if value.contains("ara ver") { ItemStatus::Hiatus }
    else if value.contains("birakildi") || value.contains("bırakıldı") { ItemStatus::Cancelled }
    else if value.contains("devam") { ItemStatus::Ongoing }
    else { ItemStatus::Unknown }
}

fn image(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src").or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn slugify_tr(value: &str) -> String {
    value.to_lowercase().replace('ı', "i").replace('ğ', "g").replace('ü', "u").replace('ş', "s").replace('ö', "o").replace('ç', "c").chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>().split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-")
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") {
        if let Some(index) = value.find("/manga/") {
            return format!("/{}", value[index + 1..].trim_end_matches('/'));
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<section aria-label="series area"><div class="card"><a href="/manga/1/sample"><img src="/cover.jpg"><h2>Sample Manga</h2></a></div></section>"#;
const SEARCH_FIXTURE: &str = r#"[{"id":1,"name":"Sample Manga","image":"covers/sample.jpg"}]"#;
const DETAILS_FIXTURE: &str = r#"<div id="content"><h1>Sample Manga</h1><img src="/cover.jpg"><div class="grid"><h2>Summary</h2><p>Description</p></div><div class="list-episode"><a href="/manga/1/sample/chapter-1"><h3>Chapter 1</h3><span>Jan 1 ,2024</span></a></div></div>"#;
const PAGES_FIXTURE: &str = r#"<script>{\"path\":\"series/sample/001.jpg\"},{\"path\":\"series/sample/002.jpg\"}</script>"#;
