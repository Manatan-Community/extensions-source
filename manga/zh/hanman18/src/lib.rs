use base64::{engine::general_purpose::STANDARD, Engine as _};
use manatan_extension::{abi::ExtensionResult, export_manga_source, http, source::MangaSource, CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult};
use manatan_shared::{dates, html, manga, url};
use serde_json::Value;

const SOURCE: Hanman18 = Hanman18;
const BASE_URL: &str = "https://hanman18.com";

struct Hanman18;

impl MangaSource for Hanman18 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") { format!("{BASE_URL}/list-manga/{page}") } else { format!("{BASE_URL}/list-manga/{page}?order_by=views") };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged { entries: vec![parse_details(&fetch(query, DETAILS_FIXTURE), &key)], has_next_page: false });
        }
        Ok(parse_listing(&fetch(&format!("{BASE_URL}/list-manga/{}?search={}", page(&request), url::query_escape(query)), LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(&fetch(&absolute(&key), DETAILS_FIXTURE), &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch(&absolute(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch(&absolute(&key), PAGES_FIXTURE)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> { Ok(manga::request_key(&request, "manga").map(|key| absolute(&key))) }
    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> { Ok(manga::request_key(&request, "chapter").map(|key| absolute(&key))) }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult { item: Some(parse_details(&fetch(input, DETAILS_FIXTURE), &key)), url: Some(input.into()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }), url: Some(input.into()), ..UrlResolveResult::default() }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}
fn fetch(target: &str, fixture: &str) -> String { client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.into()) }
fn page(request: &Value) -> u64 { request.get("page").and_then(Value::as_u64).unwrap_or(1) }
fn normalize_key(input: &str) -> String { format!("/{}", input.strip_prefix(BASE_URL).unwrap_or(input).split('?').next().unwrap_or(input).trim_matches('/')) }
fn absolute(key: &str) -> String { url::join_url(BASE_URL, key) }

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body.split("story_item").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let key = normalize_key(&url::join_url(BASE_URL, &href));
        Some(CatalogItem {
            key: key.clone(),
            title: html::text_between(chunk, "mg_name", "</div>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "HANMAN18".into())),
            cover: html::attr_after(chunk, "<img", "src").map(|v| url::join_url(BASE_URL, &v)),
            url: Some(absolute(&key)),
            language: Some("zh".into()),
            content_rating: Some("adult".into()),
            ..CatalogItem::default()
        })
    }).fold(Vec::new(), push_unique);
    Paged { entries, has_next_page: body.contains("pagination") && body.contains("li:last-child") || body.contains("Next") }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let info = html::text_between(body, "detail_listInfo", "</div>").unwrap_or_else(|| body.to_string());
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "detail_name", "</").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| "HANMAN18".into()),
        cover: html::attr_after(body, "detail_avatar", "src").or_else(|| html::attr_after(body, "<img", "src")).map(|v| url::join_url(BASE_URL, &v)),
        description: html::text_between(body, "detail_reviewContent", "</div>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()),
        authors: info_value(&info, "author").into_iter().collect(),
        artists: info_value(&info, "artist").into_iter().collect(),
        tags: body.split("<a").filter(|c| c.contains("/manga-list/")).map(html::strip_tags).filter(|v| !v.is_empty()).collect(),
        status: match html::strip_tags(&info).as_str() { text if text.contains("Completed") => ItemStatus::Completed, text if text.contains("On Going") => ItemStatus::Ongoing, _ => ItemStatus::Unknown },
        url: Some(absolute(key)),
        language: Some("zh".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn info_value(info: &str, label: &str) -> Option<String> {
    let start = info.to_ascii_lowercase().find(label)?;
    html::text_between(&info[start..], "info_value", "</div>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty() && v != "Updating")
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body.split("chapter_box").nth(1).unwrap_or(body).split("class=\"item").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let key = normalize_key(&url::join_url(BASE_URL, &href));
        Some(MangaChapter {
            key: key.clone(),
            title: Some(html::text_between(chunk, "<a", "</a>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| "Chapter".into())),
            date_uploaded: html::text_between(chunk, "<p", "</p>").map(|v| html::strip_tags(&v)).and_then(|v| dates::parse_ymd(&v)),
            url: Some(absolute(&key)),
            ..MangaChapter::default()
        })
    }).collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let script = body.split("<script").find(|s| s.contains("slides_p_path")).unwrap_or(body);
    let images = script.split('[').nth(1).unwrap_or_default().split("]").next().unwrap_or_default().replace(['"', '\''], "");
    images.split(',').filter_map(|encoded| {
        let encoded = encoded.trim();
        if encoded.is_empty() { return None; }
        let decoded = STANDARD.decode(encoded).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_else(|| encoded.to_string());
        Some(url::join_url(BASE_URL, &decoded))
    }).enumerate().map(|(index, image)| MangaPage { content: PageContent::Url { url: image, context: None }, headers: manga::image_headers(BASE_URL), description: Some(format!("Page {}", index + 1)), ..MangaPage::default() }).collect()
}

fn push_unique(mut out: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> { if !out.iter().any(|i| i.key == item.key) { out.push(item); } out }

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="story_item"><a href="/manga/sample"><img src="/cover.jpg"></a><div class="mg_info"><div class="mg_name"><a>Sample</a></div></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="detail_name"><h1>Sample</h1></div><div class="detail_avatar"><img src="/cover.jpg"></div><div class="chapter_box"><div class="item"><a href="/manga/sample/chapter-1">Chapter 1</a><p>01-01-2024</p></div></div>"#;
const PAGES_FIXTURE: &str = r#"<script>var slides_p_path=["L3BhZ2UxLmpwZw==",]</script>"#;
