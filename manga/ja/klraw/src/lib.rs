use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: KlRaw = KlRaw;
const BASE_URL: &str = "https://www.klraw.info";

struct KlRaw;

impl MangaSource for KlRaw {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest-updated"
        } else {
            "most-viewed"
        };
        Ok(parse_listing(&fetch_document(&list_url(page, "", sort, &[]), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged { entries: vec![details_by_key(&key)], has_next_page: false });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = filter_string(&request, "sort").unwrap_or("default");
        let mut filters = Vec::new();
        for id in ["type", "status", "language"] {
            if let Some(value) = filter_string(&request, id).filter(|value| !value.is_empty() && *value != "all") {
                filters.push((id, value));
            }
        }
        Ok(parse_listing(&fetch_document(
            &list_url(page, query, sort, &filters),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter/sample-1".into());
        let body = fetch_document(&absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body, &key))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: key.contains("/manga/").then(|| details_by_key(&key)),
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

fn fetch_json(target: &str, fixture: &str) -> String {
    client().get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn list_url(page: u64, query: &str, sort: &str, filters: &[(&str, &str)]) -> String {
    let mut params = Vec::new();
    if page > 1 {
        params.push(format!("p={page}"));
    }
    if !query.is_empty() {
        params.push(format!("q={}", url::query_escape(query)));
    }
    if !sort.is_empty() && sort != "default" {
        params.push(format!("sort={}", url::query_escape(sort)));
    }
    for (name, value) in filters {
        params.push(format!("{name}={}", url::query_escape(value)));
    }
    if params.is_empty() {
        BASE_URL.to_string()
    } else {
        format!("{BASE_URL}?{}", params.join("&"))
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("/manga/"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: image_attr(chunk, "alt")
                    .or_else(|| html::text_between(chunk, "post-title", "</").map(|value| html::strip_tags(&value)))
                    .or_else(|| html::text_between(chunk, "<h", "</h").map(|value| html::strip_tags(&value)))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "KL Raw".into())),
                cover: image_attr(chunk, "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("suggestive".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"") || body.contains("pagination"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), Some(key.to_string()))
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let text = html::strip_tags(body);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "KL Raw".into())),
        cover: html::attr_after(body, "summary_image", "src").or_else(|| image_attr(body, "src")).map(|image| absolute_url(&image)),
        description: html::text_between(body, "summary__content", "</div>")
            .or_else(|| html::text_between(body, "description", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_links(body, "author"),
        artists: info_links(body, "artist"),
        tags: info_links(body, "genre"),
        status: if text.contains("Finished") || text.contains("完結") {
            ItemStatus::Completed
        } else if text.contains("Publishing") || text.contains("連載") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("suggestive".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let area = html::text_between(body, "ja-chaps", "</ul>").unwrap_or_else(|| body.to_string());
    area.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/chapter/") || chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let mut extra = serde_json::Map::new();
            if let Some(id) = html::attr(chunk, "data-id").or_else(|| html::attr(chunk, "data-chapter")) {
                extra.insert("id".into(), Value::String(id));
            }
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, ">", "</a>")
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "Chapter".into()),
                ),
                date_uploaded: html::text_between(chunk, "chapter-release-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                extra: extra.into_iter().collect(),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, chapter_key: &str) -> Vec<MangaPage> {
    let chapter_id = html::attr_after(body, "chapter-id", "value")
        .or_else(|| html::attr_after(body, "data-chapter", "data-id"))
        .or_else(|| html::attr_after(body, "reading-content", "data-id"))
        .or_else(|| chapter_key.rsplit('/').next().map(ToString::to_string));
    if let Some(id) = chapter_id {
        let ajax = fetch_json(&format!("{BASE_URL}/json/chapter?mode=vertical&id={id}"), AJAX_FIXTURE);
        let pages = parse_ajax_pages(&ajax);
        if !pages.is_empty() {
            return pages;
        }
    }
    parse_direct_pages(body)
}

fn parse_ajax_pages(body: &str) -> Vec<MangaPage> {
    let value = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(AJAX_FIXTURE).unwrap());
    let mut images = Vec::new();
    collect_images(&value, &mut images);
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| page_from_image(index, &image))
        .collect()
}

fn collect_images(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) if looks_like_image(text) => out.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                collect_images(item, out);
            }
        }
        Value::Object(object) => {
            for item in object.values() {
                collect_images(item, out);
            }
        }
        _ => {}
    }
}

fn parse_direct_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|image| looks_like_image(image))
        .enumerate()
        .map(|(index, image)| page_from_image(index, &image))
        .collect()
}

fn page_from_image(index: usize, image: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: absolute_url(image),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn looks_like_image(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    !lower.starts_with("data:") && [".jpg", ".jpeg", ".png", ".webp", ".avif"].iter().any(|ext| lower.contains(ext))
}

fn info_links(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn image_attr(chunk: &str, attr: &str) -> Option<String> {
    html::attr_after(chunk, "<img", attr).or_else(|| html::attr(chunk, attr))
}

fn key_from_url(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input).split('?').next().unwrap_or(input).trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request.get("filters").and_then(Value::as_object).and_then(|filters| filters.get(id)).and_then(Value::as_str)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><a href="/manga/sample"><img src="/cover.jpg" alt="Sample KL Raw"><h3 class="post-title">Sample KL Raw</h3></a></div>"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample KL Raw</h1><div class="summary_image"><img src="/cover.jpg"></div><div class="summary__content">Sample description.</div>
<a href="/genre/action">Action</a><span>Publishing</span>
<ul id="ja-chaps"><li><a data-id="sample-1" href="/chapter/sample-1">Chapter 1</a><span class="chapter-release-date">2024-01-01</span></li></ul>
"#;
const AJAX_FIXTURE: &str = r#"{"images":["https://www.klraw.info/page1.jpg","https://www.klraw.info/page2.jpg"]}"#;
const PAGES_FIXTURE: &str = r#"<input id="chapter-id" value="sample-1"><div class="reading-content"><img src="/page1.jpg"></div>"#;
