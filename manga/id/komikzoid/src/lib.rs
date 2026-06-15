use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Komikzoid = Komikzoid;
const BASE_URL: &str = "https://komikzoid.id";
const CONTENT_RATING: &str = "safe";

struct Komikzoid;

impl MangaSource for Komikzoid {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing_page(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updated"
        } else {
            "view"
        };
        Ok(parse_listing_page(&fetch_document_or_fixture(
            &search_url(page, "", sort),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let sort = request
            .get("filters")
            .and_then(|filters| filters.get("sort"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("view");
        Ok(parse_listing_page(&fetch_document_or_fixture(
            &search_url(page, query, sort),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document_or_fixture(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
                )),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    if let Some(path) = value.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_matches('/'));
    }
    format!("/{}", value.trim_matches('/'))
}

fn search_url(page: u64, query: &str, sort: &str) -> String {
    format!(
        "{BASE_URL}/manga?page={page}&sort={}&search={}",
        url::query_escape(sort),
        url::query_escape(query)
    )
}

fn parse_listing_page(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: parse_listing(body),
        has_next_page: body.contains("fa-angle-right"),
    }
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("product__item"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "img-link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h5", "</h5>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<a", "title"))
                .or_else(|| url::slug_from_url(&href))
                .unwrap_or_else(|| "Komikzoid".to_string());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "set-bg", "data-setbg")
                    .or_else(|| image_attr(chunk))
                    .map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("id".to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique_catalog_item)
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "anime__details__content", "</h3>")
            .or_else(|| html::text_between(body, "<h3", "</h3>"))
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Komikzoid".into())),
        authors: html::text_between(body, "<h3", "</span>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .into_iter()
            .collect(),
        description: html::text_between(body, "<p", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        cover: html::attr_after(body, "set-bg", "data-setbg")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        status: parse_status(body),
        url: Some(absolute_url(&key)),
        language: Some("id".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("href"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains(BASE_URL) && !href.starts_with('/') {
                return None;
            }
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, ">", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| url::slug_from_url(&href)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), |mut chapters: Vec<MangaChapter>, chapter| {
            if !chapters.iter().any(|existing| existing.key == chapter.key) {
                chapters.push(chapter);
            }
            chapters
        })
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(input: &str) -> Option<String> {
    html::attr(input, "data-src")
        .or_else(|| html::attr(input, "data-setbg"))
        .or_else(|| html::attr(input, "src"))
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn push_unique_catalog_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="product__page__content"><div style=""><div class="col-6"><div class="product__item"><a class="img-link" href="/manga/sample"></a><div class="set-bg" data-setbg="/cover.jpg"></div><h5>Sample Manga</h5></div></div></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="anime__details__content"><div class="set-bg" data-setbg="/cover.jpg"></div><h3>Sample Manga</h3><span>Author</span><p>Sample description.</p><ul><li>Status Ongoing</li></ul></div><div class="anime__details__episodes"><a href="/manga/sample/chapter-1">Chapter 1</a></div>
"#;
const PAGES_FIXTURE: &str = r#"<div class="container"><div class="read-img"><img src="/page1.jpg"><img src="/page2.jpg"></div></div>"#;
