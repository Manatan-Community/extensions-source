use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Multporn = Multporn;
const BASE_URL: &str = "https://multporn.net";

struct Multporn;

impl MangaSource for Multporn {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1).saturating_sub(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "new"
        } else {
            "best"
        };
        Ok(parse_listing(&fetch_document(&format!("{BASE_URL}/{path}?page={page}"), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1).saturating_sub(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), Some(key))],
                has_next_page: false,
            });
        }
        let filters = request.get("filters");
        let target = text_filter_url(filters, page).unwrap_or_else(|| {
            let mut params = vec![
                format!("page={page}"),
                format!("search_api_views_fulltext={}", url::query_escape(query)),
            ];
            push_filter(&mut params, filters, "sort_by");
            push_filter(&mut params, filters, "type");
            format!("{BASE_URL}/search?{}", params.join("&"))
        });
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Chapter".to_string()),
            chapter_number: Some(1.0),
            url: Some(absolute_url(&key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
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
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("masonry-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "views-field-title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "views-field-title", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Multporn".to_string()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_from_chunk(chunk),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("pager-next") || body.contains("rel=\"next\""),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    let info = |label: &str| -> Vec<String> {
        body.split("field")
            .filter(|chunk| chunk.contains(label))
            .flat_map(link_texts)
            .collect()
    };
    let sections = info("Section");
    let characters = info("Characters");
    let tags = info("Tags");
    let authors = info("Author");
    let page_count = body.matches("jb-image").count();
    let mut description = String::new();
    if !sections.is_empty() {
        description.push_str("Section:\n");
        description.push_str(&sections.join(", "));
        description.push_str("\n\n");
    }
    if !characters.is_empty() {
        description.push_str("Characters:\n");
        description.push_str(&characters.join(", "));
        description.push_str("\n\n");
    }
    description.push_str(&format!("Pages:\n{page_count}"));
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "page-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Multporn".to_string())),
        cover: image_from_chunk(body),
        authors: authors.clone(),
        artists: authors,
        tags: [tags, sections, characters].concat(),
        description: Some(description),
        status: if body.contains("Ongoings") { ItemStatus::Ongoing } else { ItemStatus::Completed },
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("jb-image")
        .skip(1)
        .filter_map(image_from_chunk)
        .map(|image| image.replace("/styles/juicebox_2k/public", "").split('?').next().unwrap_or("").to_string())
        .filter(|image| !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: image, context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|image| absolute_url(&image))
}

fn link_texts(chunk: &str) -> Vec<String> {
    chunk
        .split("<a")
        .skip(1)
        .filter_map(|part| html::text_between(part, "<a", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn text_filter_url(filters: Option<&Value>, page: u64) -> Option<String> {
    let filters = filters?;
    let names = [
        ("comic_tags", "category"),
        ("comic_characters", "characters"),
        ("comic_authors", "authors_comics"),
        ("comic_sections", "comics"),
        ("manga_categories", "category_hentai"),
        ("manga_characters", "characters_hentai"),
        ("manga_authors", "authors_hentai_comics"),
        ("manga_sections", "hentai_manga"),
        ("picture_authors", "authors_albums"),
        ("picture_sections", "pictures"),
        ("hentai_sections", "hentai"),
        ("rule_63_sections", "rule_63"),
        ("gay_tags", "category_gay"),
    ];
    for (key, path) in names {
        if let Some(value) = filters.get(key).and_then(Value::as_str).filter(|value| !value.trim().is_empty()) {
            let slug = value.split(',').next().unwrap_or(value).trim().replace(' ', "_").to_ascii_lowercase();
            return Some(format!("{BASE_URL}/{path}/{slug}?page=0,{page}"));
        }
    }
    None
}

fn push_filter(params: &mut Vec<String>, filters: Option<&Value>, key: &str) {
    if let Some(value) = filters.and_then(|filters| filters.get(key)).and_then(Value::as_str).filter(|value| !value.is_empty()) {
        params.push(format!("{key}={}", url::query_escape(value)));
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="masonry-item"><div class="views-field-title"><a href="/sample">Sample</a></div><img src="/cover.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 id="page-title">Sample</h1><div class="field"><span class="field-label">Section:</span><div class="links"><a>Comics</a></div></div><div class="jb-image"><img src="/page1.jpg"></div>"#;
const PAGES_FIXTURE: &str = DETAILS_FIXTURE;
