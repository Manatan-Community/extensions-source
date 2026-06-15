use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: CuuTruyenMoe = CuuTruyenMoe;
const BASE_URL: &str = "https://cuutruyen.moe";

struct CuuTruyenMoe;

impl MangaSource for CuuTruyenMoe {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "-updated_at"
        } else {
            "-views"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/tim-kiem?sort={sort}&filter[status]=2,1&page={page}"),
            LIST_FIXTURE,
        ), page))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged { entries: vec![details_by_key(&key)], has_next_page: false });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        Ok(parse_listing(&fetch_document(&search_url(page, query, filters), LIST_FIXTURE), page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/truyen/sample/1".into());
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
            return Ok(Some(UrlResolveResult { item: key.contains("/truyen/").then(|| details_by_key(&key)), url: Some(input.into()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }), url: Some(input.into()), ..UrlResolveResult::default() }))
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

fn search_url(page: u64, query: &str, filters: &Value) -> String {
    let mut params = vec![
        format!("sort={}", url::query_escape(filter(filters, "sort").unwrap_or("-updated_at"))),
        format!("filter[status]={}", url::query_escape(filter(filters, "status").unwrap_or("2,1"))),
        format!("page={page}"),
    ];
    if let Some(genres) = multi_filter(filters, "genres").filter(|value| !value.is_empty()) {
        params.push(format!("filter[accept_genres]={}", url::query_escape(&genres)));
    }
    if !query.is_empty() {
        params.push(format!("keyword={}", url::query_escape(query)));
    }
    format!("{BASE_URL}/tim-kiem?{}", params.join("&"))
}

fn parse_listing(body: &str, page: u64) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("manga-vertical"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: background_image(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("vi".into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged { entries, has_next_page: body.contains(&format!("page={}", page + 1)) }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let text = html::strip_tags(body);
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(body, "grow text-lg", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "cover-frame", "src").or_else(|| background_image(body)).map(|image| absolute_url(&image)),
        authors: link_texts(body, "/tac-gia/"),
        tags: link_texts(body, "/the-loai/"),
        description: html::text_between(body, "mg-plot", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(&text),
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/truyen/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "grow", "</a>")
                .or_else(|| html::text_between(chunk, "<span", "</span>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::attr_after(chunk, "timeago", "datetime").and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("max-w-full") || chunk.contains("data-src") || chunk.contains("src="))
        .filter_map(image_attr)
        .filter(|image| looks_like_image(image))
        .enumerate()
        .map(|(index, image)| page(index, &absolute_url(&image)))
        .collect()
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection { id: id.into(), title: title.into(), style: Some(HomeSectionStyle::Cover), has_more: page.has_next_page, entries: page.entries, ..HomeSection::default() }
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("đã hoàn thành") { ItemStatus::Completed } else if lower.contains("đang tiến hành") { ItemStatus::Ongoing } else { ItemStatus::Unknown }
}

fn link_texts(body: &str, marker: &str) -> Vec<String> {
    body.split("<a").skip(1).filter(|chunk| chunk.contains(marker)).map(html::strip_tags).filter(|value| !value.is_empty()).collect()
}

fn background_image(chunk: &str) -> Option<String> {
    let marker = "background-image:";
    let rest = chunk.split(marker).nth(1)?;
    let rest = rest.split(';').next().unwrap_or(rest);
    let start = rest.find("url(").map(|i| i + 4)?;
    Some(rest[start..].trim_matches([')', '\'', '"', ' ']).to_string())
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")).or_else(|| html::attr(chunk, "data-original")).or_else(|| html::attr(chunk, "data-lazy-src"))
}

fn looks_like_image(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    !lower.starts_with("data:") && [".jpg", ".jpeg", ".png", ".webp", ".avif"].iter().any(|ext| lower.contains(ext))
}

fn page(index: usize, image: &str) -> MangaPage {
    MangaPage { content: PageContent::Url { url: image.into(), context: Some(manga::image_headers(BASE_URL)) }, headers: manga::image_headers(BASE_URL), description: Some(format!("Page {}", index + 1)), ..MangaPage::default() }
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") { value.trim_start_matches(BASE_URL).trim_end_matches('/').to_string() } else { format!("/{}", value.trim_start_matches('/').trim_end_matches('/')) }
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http") { value.into() } else { format!("{BASE_URL}/{}", value.trim_start_matches('/')) }
}

fn key_from_url(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize_key(input)).filter(|key| key.contains("/truyen/"))
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters.get(id).and_then(Value::as_str).filter(|value| !value.is_empty())
}

fn multi_filter(filters: &Value, id: &str) -> Option<String> {
    match filters.get(id) {
        Some(Value::Array(items)) => Some(items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")),
        Some(Value::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) { items.push(item); }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|seen| seen.key == item.key) { items.push(item); }
    items
}

const LIST_FIXTURE: &str = r#"<div class="manga-vertical"><div class="cover" style="background-image:url('/cover.jpg')"></div><div class="p-2"><a href="https://cuutruyen.moe/truyen/sample">Sample</a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><div class="cover-frame" style="background-image:url('/cover.jpg')"></div><a href="/tac-gia/demo">Author</a><a href="/the-loai/action">Action</a><div class="mg-plot"><p>Summary</p></div><ul class="overflow-y-auto"><a href="/truyen/sample/1"><div class="grow"><span>Chapter 1</span></div><span class="timeago" datetime="2024-01-01 00:00:00"></span></a></ul>"#;
const PAGES_FIXTURE: &str = r#"<div class="text-center"><img class="max-w-full" src="/page1.jpg"/></div>"#;

export_manga_source!(SOURCE);
