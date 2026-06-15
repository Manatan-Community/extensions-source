use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: ManhuaRock = ManhuaRock;
const DEFAULT_BASE_URL: &str = "https://manhuarock4.site";

struct ManhuaRock;

impl MangaSource for ManhuaRock {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest-updated"
        } else {
            "most-viewd"
        };
        Ok(parse_listing(&fetch_document(
            &base,
            &format!("{base}/tat-ca-truyen/{page}/?sort={sort}"),
            LIST_FIXTURE,
        ), &base))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(&base, query) {
            return Ok(Paged { entries: vec![details_by_key(&base, &key)], has_next_page: false });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if query.is_empty() {
            let filters = request.get("filters").unwrap_or(&Value::Null);
            let genre = filter(filters, "genre").unwrap_or("tat-ca-truyen");
            let sort = filter(filters, "sort").unwrap_or("most-viewd");
            format!("{base}/{genre}/{page}/?sort={}", url::query_escape(sort))
        } else {
            format!("{base}/search/{page}/?keyword={}", url::query_escape(query))
        };
        Ok(parse_listing(&fetch_document(&base, &target, LIST_FIXTURE), &base))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(details_by_key(&base, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(parse_chapters(&fetch_document(&base, &absolute_url(&base, &key), DETAILS_FIXTURE), &base))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/truyen/sample/chapter-1".into());
        let chapter_url = absolute_url(&base, &key);
        let chapter_id = get_chapter_id(&base, &chapter_url);
        let payload = client(&base)
            .get(format!("{base}/ajax/image/list/chap/{chapter_id}?mode=vertical&quality=high"))
            .referer(&chapter_url)
            .xhr()
            .send_text()
            .unwrap_or_else(|_| PAGES_API_FIXTURE.to_string());
        let parsed = serde_json::from_str::<AjaxImageListResponse>(&payload).unwrap_or_default();
        if !parsed.status {
            return Ok(vec![manga::text_page(parsed.msg.as_deref().unwrap_or("Khong lay duoc danh sach anh"))]);
        }
        Ok(parse_pages(parsed.html.as_deref().unwrap_or_default(), &base, &chapter_url))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section("popular", "Popular", self.list(with_listing(&request, "popular"))?),
            home_section("latest", "Latest", self.list(with_listing(&request, "latest"))?),
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&base, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&base, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(key) = key_from_url(&base, input) {
            return Ok(Some(UrlResolveResult {
                item: key.contains("/truyen-tranh/").then(|| details_by_key(&base, &key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }),
            url: Some(input.into()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_referer(format!("{base}/"))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_document(base: &str, target: &str, fixture: &str) -> String {
    client(base).get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("latest-manga-card")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "latest-manga-title", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(base, &href);
            let title = html::text_between(chunk, "latest-manga-title", "</a>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| absolute_url(base, &image)),
                url: Some(absolute_url(base, &key)),
                language: Some("vi".into()),
                content_rating: Some("suggestive".into()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("li next") && !body.contains("next disabled"),
    }
}

fn details_by_key(base: &str, key: &str) -> CatalogItem {
    parse_details(&fetch_document(base, &absolute_url(base, key), DETAILS_FIXTURE), base, key)
}

fn parse_details(body: &str, base: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "post-title", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "summary_image", "data-src")
            .or_else(|| html::attr_after(body, "summary_image", "src"))
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(base, &image)),
        authors: info_text(body, "author-content").map(|v| vec![v]).unwrap_or_default(),
        artists: info_text(body, "artist-content").map(|v| vec![v]).unwrap_or_default(),
        tags: link_texts(body, "genres-content"),
        description: html::text_between(body, "div.dsct", "</div>")
            .or_else(|| html::text_between(body, "description-summary", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(&info_text(body, "summary-heading:contains(Tình Trạng)").unwrap_or_else(|| html::strip_tags(body))),
        url: Some(absolute_url(base, key)),
        language: Some("vi".into()),
        content_rating: Some("suggestive".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("row-content-chapter") || chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(base, &href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: parse_relative_date(&html::strip_tags(chunk)),
                url: Some(absolute_url(base, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn get_chapter_id(base: &str, chapter_url: &str) -> String {
    let last = chapter_url.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    if last.chars().all(|c| c.is_ascii_digit()) {
        return last.to_string();
    }
    let body = fetch_document(base, chapter_url, DETAILS_FIXTURE);
    body.split("chapter_id")
        .nth(1)
        .and_then(|tail| tail.split('=').nth(1))
        .and_then(|tail| tail.split([',', ';']).next())
        .map(|value| value.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "1".into())
}

fn parse_pages(body: &str, base: &str, chapter_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:"))
        .fold(Vec::<String>::new(), |mut seen, image| {
            let image = absolute_url(base, &image);
            if !seen.contains(&image) { seen.push(image); }
            seen
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| page(index, &image, chapter_url))
        .collect()
}

fn page(index: usize, image: &str, referer: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url { url: image.into(), context: Some(manga::image_headers(referer)) },
        headers: manga::image_headers(referer),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
}

fn info_text(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn link_texts(body: &str, marker: &str) -> Vec<String> {
    body.find(marker)
        .map(|index| body[index..].split("<a").skip(1).map(html::strip_tags).filter(|v| !v.is_empty()).collect())
        .unwrap_or_default()
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("hoàn thành") { ItemStatus::Completed }
    else if lower.contains("đang tiến hành") { ItemStatus::Ongoing }
    else { ItemStatus::Unknown }
}

fn parse_relative_date(_text: &str) -> Option<i64> {
    None
}

fn base_url(request: &Value) -> String {
    request.get("preferences")
        .and_then(|p| p.get("overrideBaseUrl"))
        .and_then(Value::as_str)
        .filter(|v| v.starts_with("http://") || v.starts_with("https://"))
        .unwrap_or(DEFAULT_BASE_URL)
        .trim_end_matches('/')
        .to_string()
}

fn normalize_key(base: &str, value: &str) -> String {
    if value.starts_with("http") { value.trim_start_matches(base).trim_end_matches('/').to_string() }
    else { format!("/{}", value.trim_start_matches('/').trim_end_matches('/')) }
}

fn absolute_url(base: &str, value: &str) -> String {
    if value.starts_with("http") { value.into() } else { format!("{base}/{}", value.trim_start_matches('/')) }
}

fn key_from_url(base: &str, input: &str) -> Option<String> {
    input.starts_with(base).then(|| normalize_key(base, input)).filter(|key| key.contains("/truyen"))
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters.get(id).and_then(Value::as_str).filter(|value| !value.is_empty())
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut value = request.clone();
    value["page"] = json!(1);
    value["listingId"] = json!(listing);
    value
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection { id: id.into(), title: title.into(), style: Some(HomeSectionStyle::Cover), entries: page.entries, has_more: page.has_next_page, ..HomeSection::default() }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) { items.push(item); }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|seen| seen.key == item.key) { items.push(item); }
    items
}

#[derive(Default, Deserialize)]
struct AjaxImageListResponse {
    status: bool,
    msg: Option<String>,
    html: Option<String>,
}

const LIST_FIXTURE: &str = r#"<div class="latest-manga-card"><div class="latest-manga-title"><a href="/truyen-tranh/sample">Sample</a></div><img src="/cover.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="post-title"><h1>Sample</h1></div><div class="summary_image"><img src="/cover.jpg"></div><div class="author-content">Author</div><div class="genres-content"><a>Action</a></div><div class="dsct">Summary</div><ul class="row-content-chapter"><li><a href="/truyen-tranh/sample/chapter-1.html">Chapter 1</a><span class="chapter-time">1 ngày</span></li></ul><script>chapter_id = 1,</script>"#;
const PAGES_API_FIXTURE: &str = r#"{"status":true,"html":"<img src=\"/page1.jpg\">"}"#;

export_manga_source!(SOURCE);
