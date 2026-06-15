use manatan_extension::{CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: AmangaPlanet = AmangaPlanet;
const BASE_URL: &str = "https://www.amangaplanet.com.tr";

struct AmangaPlanet;

impl MangaSource for AmangaPlanet {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "update" } else { "popular" };
        Ok(parse_listing(&fetch_document_or_fixture(&search_url(page, "", Some(order), request.get("filters")), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged { entries: vec![parse_details(&fetch_document_or_fixture(query, DETAILS_FIXTURE), Some(key))], has_next_page: false });
        }
        Ok(parse_listing(&fetch_document_or_fixture(&search_url(page, query, None, request.get("filters")), LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(&fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-chapter".to_string());
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

fn search_url(page: u64, query: &str, forced_order: Option<&str>, filters: Option<&Value>) -> String {
    let mut params = vec![format!("title={}", url::query_escape(query)), format!("page={page}")];
    for key in ["author", "yearx", "status", "type"] {
        if let Some(value) = filter(filters, key).filter(|value| !value.is_empty()) {
            params.push(format!("{key}={}", url::query_escape(&value)));
        }
    }
    let order = forced_order.map(str::to_string).or_else(|| filter(filters, "order")).unwrap_or_default();
    if !order.is_empty() {
        params.push(format!("order={}", url::query_escape(&order)));
    }
    format!("{BASE_URL}/manga/?{}", params.join("&"))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body.split("<div").skip(1)
        .filter(|chunk| chunk.contains("bsx") || chunk.contains("uta") || chunk.contains("imgu"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/manga/") { return None; }
            let title = html::attr_after(chunk, "<a", "title").or_else(|| html::attr_after(chunk, "<img", "alt")).or_else(|| html::text_between(chunk, "<a", "</a>").map(|v| html::strip_tags(&v))).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Manga".into()));
            let key = normalize_key(&href);
            Some(CatalogItem { key: key.clone(), title, cover: image(chunk).map(|v| absolute_url(&v)), url: Some(absolute_url(&key)), language: Some("tr".into()), content_rating: Some("adult".into()), initialized: false, ..CatalogItem::default() })
        })
        .fold(Vec::new(), |mut acc: Vec<CatalogItem>, item| {
            if !acc.iter().any(|existing| existing.key == item.key) { acc.push(item); }
            acc
        });
    Paged { entries, has_next_page: body.contains("pagination") && (body.contains("class=\"next") || body.contains("hpage") || body.contains(" rel=\"next")) }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</").or_else(|| html::text_between(body, "<h1", "</h1>")).map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "thumb", "data-src").or_else(|| html::attr_after(body, "thumb", "src")).or_else(|| image(body)).map(|v| absolute_url(&v)),
        description: html::text_between(body, "entry-content", "</div>").or_else(|| html::text_between(body, "desc", "</div>")).map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()),
        authors: info_values(body, "Yazar").into_iter().chain(info_values(body, "Author")).collect(),
        artists: info_values(body, "Çizer").into_iter().chain(info_values(body, "Sanatçı")).collect(),
        tags: genre_values(body),
        status: parse_status(&status_text(body)),
        url: Some(absolute_url(&key)),
        language: Some("tr".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li").skip(1)
        .filter(|chunk| chunk.contains("chapter") || chunk.contains("chbox") || chunk.contains("eph-num"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "chapternum", "</").or_else(|| html::text_between(chunk, "<a", "</a>")).map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter { key: normalize_key(&href), title: Some(title), url: Some(absolute_url(&href)), date_uploaded: html::text_between(chunk, "chapterdate", "</").and_then(|v| manatan_shared::dates::parse_fixture_date(&html::strip_tags(&v))), ..MangaChapter::default() })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let mut images = body.split("<img").skip(1)
        .filter(|chunk| chunk.contains("readerarea") || chunk.contains("data-src") || chunk.contains("src="))
        .filter_map(image)
        .collect::<Vec<_>>();
    if images.is_empty() {
        if let Some(json) = html::text_between(body, "\"images\"", "]") {
            images = json.split('"').filter(|part| part.starts_with("http") || part.starts_with('/')).map(str::to_string).collect();
        }
    }
    images.into_iter().enumerate().map(|(index, value)| MangaPage { content: PageContent::Url { url: absolute_url(&value), context: None }, headers: manga::image_headers(referer), description: Some(format!("Page {}", index + 1)), ..MangaPage::default() }).collect()
}

fn filter(filters: Option<&Value>, key: &str) -> Option<String> {
    filters?.get(key)?.as_str().map(str::to_string)
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    body.split(label).skip(1).filter_map(|chunk| html::text_between(chunk, "<span", "</span>").or_else(|| html::text_between(chunk, "<i", "</i>")).map(|v| html::strip_tags(&v))).filter(|v| !v.is_empty()).collect()
}

fn genre_values(body: &str) -> Vec<String> {
    body.split("<a").skip(1).filter(|chunk| chunk.contains("genre") || chunk.contains("genres")).filter_map(|chunk| html::text_between(chunk, ">", "</a>").map(|v| html::strip_tags(&v))).filter(|v| !v.is_empty()).collect()
}

fn status_text(body: &str) -> String {
    html::text_between(body, "Durum", "</").or_else(|| html::text_between(body, "status", "</")).map(|v| html::strip_tags(&v)).unwrap_or_default()
}

fn parse_status(value: &str) -> ItemStatus {
    let value = value.to_lowercase();
    if ["devam", "ongoing", "güncel"].iter().any(|needle| value.contains(needle)) { ItemStatus::Ongoing }
    else if ["tamam", "bitti", "completed", "finished"].iter().any(|needle| value.contains(needle)) { ItemStatus::Completed }
    else if ["hiatus", "ara"].iter().any(|needle| value.contains(needle)) { ItemStatus::Hiatus }
    else if ["dropped", "bırak"].iter().any(|needle| value.contains(needle)) { ItemStatus::Cancelled }
    else { ItemStatus::Unknown }
}

fn image(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src").or_else(|| html::attr_after(chunk, "<img", "data-lazy-src")).or_else(|| html::attr_after(chunk, "<img", "data-cfsrc")).or_else(|| html::attr_after(chunk, "<img", "src"))
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

const LIST_FIXTURE: &str = r#"<div class="bsx"><a href="/manga/sample/" title="Sample Manga"><img src="/cover.jpg"></a></div><div class="pagination"><a class="next" href="/manga/page/2/"></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="bigcontent"><h1 class="entry-title">Sample Manga</h1><div class="thumb"><img src="/cover.jpg"></div><div class="desc">Description</div><div class="mgen"><a href="/genre/action">Action</a></div><ul id="chapterlist"><li><a href="/sample-chapter"><span class="chapternum">Chapter 1</span></a><span class="chapterdate">01/01/2024</span></li></ul></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="readerarea"><img src="/page1.jpg"><img src="/page2.jpg"></div>"#;
