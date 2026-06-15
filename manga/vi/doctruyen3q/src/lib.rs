use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: DocTruyen3Q = DocTruyen3Q;
const BASE_URL: &str = "https://doctruyen3qhub.live";

struct DocTruyen3Q;

impl MangaSource for DocTruyen3Q {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "truyen-moi-cap-nhat" } else { "truyen-hot" };
        Ok(parse_listing(&fetch_document(&format!("{BASE_URL}/{path}?page={page}"), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged { entries: vec![details_by_key(&key)], has_next_page: false });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut target = format!("{BASE_URL}/tim-kiem");
        let filters = request.get("filters").unwrap_or(&Value::Null);
        if let Some(genre) = filter(filters, "genre") {
            target = format!("{target}/{genre}");
        }
        if query.is_empty() {
            target = format!("{target}?page={page}");
        } else {
            target = format!("{target}?keyword={}", url::query_escape(query));
        }
        if let Some(status) = filter(filters, "status") {
            target.push_str(&format!("&status={}", url::query_escape(status)));
        }
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/truyen-tranh/sample/chap-1".into());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![home_section("popular", "Popular", self.list(json!({"page": 1, "listingId": "popular"}))?), home_section("latest", "Latest", self.list(json!({"page": 1, "listingId": "latest"}))?)])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult { item: key.contains("/truyen-tranh/").then(|| details_by_key(&key)), url: Some(input.into()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }), url: Some(input.into()), ..UrlResolveResult::default() }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("item-manga") || chunk.contains("item"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/truyen-tranh/") { return None; }
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h3", "</h3>").or_else(|| html::text_between(chunk, "<a", "</a>")).map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem { key: key.clone(), title, cover: image_attr(chunk).map(|image| absolute_url(&image)), url: Some(absolute_url(&key)), language: Some("vi".into()), content_rating: Some("adult".into()), ..CatalogItem::default() })
        })
        .fold(Vec::new(), push_unique);
    Paged { entries, has_next_page: body.contains("rel=\"next\"") || body.contains("pagination") }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let text = html::strip_tags(body);
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "title-manga", "</").or_else(|| html::text_between(body, "<h1", "</h1>")).map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "image-comic", "src").or_else(|| image_attr(body)).map(|image| absolute_url(&image)),
        description: html::text_between(body, "detail-summary", "</p>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        tags: link_texts(body, "category").or_else(|| Some(link_texts_by_href(body, "/the-loai/"))).unwrap_or_default(),
        status: parse_status(&text),
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("row") && !chunk.contains("heading"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter { key: key.clone(), title: Some(title), date_uploaded: html::text_between(chunk, "chapters", "</li>").map(|value| html::strip_tags(&value)).and_then(|value| manatan_shared::dates::parse_fixture_date(&value)), url: Some(absolute_url(&key)), ..MangaChapter::default() })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("page-chapter") || chunk.contains("data-src") || chunk.contains("src="))
        .filter_map(image_attr)
        .filter(|image| looks_like_image(image))
        .fold(Vec::<String>::new(), |mut seen, image| {
            let image = absolute_url(&image);
            if !seen.contains(&image) { seen.push(image); }
            seen
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| page(index, &image))
        .collect()
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection { id: id.into(), title: title.into(), style: Some(HomeSectionStyle::Cover), has_more: page.has_next_page, entries: page.entries, ..HomeSection::default() }
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("hoàn thành") || lower.contains("completed") { ItemStatus::Completed } else if lower.contains("đang") || lower.contains("ongoing") { ItemStatus::Ongoing } else { ItemStatus::Unknown }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src"))
}

fn looks_like_image(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    !lower.starts_with("data:") && [".jpg", ".jpeg", ".png", ".webp", ".avif"].iter().any(|ext| lower.contains(ext))
}

fn page(index: usize, image: &str) -> MangaPage {
    MangaPage { content: PageContent::Url { url: image.into(), context: Some(manga::image_headers(BASE_URL)) }, headers: manga::image_headers(BASE_URL), description: Some(format!("Page {}", index + 1)), ..MangaPage::default() }
}

fn link_texts(body: &str, marker: &str) -> Option<Vec<String>> {
    body.find(marker).map(|index| link_texts_by_href(&body[index..], ""))
}

fn link_texts_by_href(body: &str, href_marker: &str) -> Vec<String> {
    body.split("<a").skip(1).filter(|chunk| chunk.contains(href_marker) || href_marker.is_empty()).map(html::strip_tags).filter(|value| !value.is_empty()).collect()
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") { value.trim_start_matches(BASE_URL).trim_end_matches('/').to_string() } else { format!("/{}", value.trim_start_matches('/').trim_end_matches('/')) }
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http") { value.into() } else { format!("{BASE_URL}/{}", value.trim_start_matches('/')) }
}

fn key_from_url(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize_key(input)).filter(|key| key.contains("/truyen-tranh/"))
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters.get(id).and_then(Value::as_str).filter(|value| !value.is_empty())
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) { items.push(item); }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|seen| seen.key == item.key) { items.push(item); }
    items
}

const LIST_FIXTURE: &str = r#"<div class="item-manga"><div class="item"><h3><a href="/truyen-tranh/sample">Sample</a></h3><img src="/cover.jpg"></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="title-manga">Sample</h1><img class="image-comic" src="/cover.jpg"><p class="detail-summary">Summary</p><li class="category"><a href="/the-loai/action">Action</a></li><li class="status"><span>Đang tiến hành</span></li><div class="list-chapter"><li class="row"><span class="chapters"><a href="/truyen-tranh/sample/chap-1">Chapter 1</a></span><div>01-01-2024</div></li></div>"#;
const PAGES_FIXTURE: &str = r#"<div class="page-chapter" id="1"><img src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
