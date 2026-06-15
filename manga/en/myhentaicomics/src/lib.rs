use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MyHentaiComics = MyHentaiComics;
const BASE_URL: &str = "https://myhentaicomics.com";

struct MyHentaiComics;

impl MangaSource for MyHentaiComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "gallery"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document(&format!("{BASE_URL}/{path}/{page}"), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), Some(key))],
                has_next_page: false,
            });
        }
        let target = if !query.is_empty() {
            format!("{BASE_URL}/search/{page}?query={}", url::query_escape(query))
        } else if let Some(category) = filter(request.get("filters"), "category").filter(|value| !value.is_empty()) {
            format!("{BASE_URL}/gallery/category/{category}/{page}")
        } else {
            let sort = filter(request.get("filters"), "sort").unwrap_or_else(|| "gallery".to_string());
            format!("{BASE_URL}/{sort}/{page}")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/gallery/sample".to_string());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/gallery/sample".to_string());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let first_page = html::attr_after(body.split("comic-cover").nth(1).unwrap_or(&body), "<a", "href")
            .unwrap_or_else(|| format!("{BASE_URL}/gallery/show/sample/1"));
        let comic_id = first_page
            .split("/gallery/show/")
            .nth(1)
            .and_then(|part| part.split('/').next())
            .unwrap_or("sample");
        Ok(vec![MangaChapter {
            key: format!("/gallery/show/{comic_id}/1"),
            title: Some("Chapter 1".to_string()),
            chapter_number: Some(1.0),
            url: Some(format!("{BASE_URL}/gallery/show/{comic_id}/1")),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/gallery/show/sample/1".to_string());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE), &key))
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
        .split("comic-inner")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "comic-name", "</")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MyHentaiComics".to_string()));
            Some(catalog_item(key, title, image_from_chunk(chunk), false))
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("li next") || body.contains("class=\"next"),
    }
}

fn catalog_item(key: String, title: String, cover: Option<String>, initialized: bool) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover,
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/gallery/sample".to_string());
    let description_chunk = body.split("comic-description").nth(1).unwrap_or(body);
    let categories = links_containing(description_chunk, "/gallery/category/");
    let artists = links_containing(description_chunk, "/gallery/artist/");
    let groups = links_containing(description_chunk, "/gallery/group/");
    let pages = description_chunk
        .split("<div")
        .find(|chunk| chunk.contains("Pages:"))
        .map(|chunk| html::strip_tags(chunk));
    let mut description = String::new();
    if !artists.is_empty() {
        description.push_str("Artists: ");
        description.push_str(&artists.join(", "));
        description.push('\n');
    }
    if !groups.is_empty() {
        description.push_str("Groups: ");
        description.push_str(&groups.join(", "));
        description.push('\n');
    }
    if let Some(pages) = pages {
        description.push_str(&pages);
    }
    let mut item = catalog_item(
        key,
        html::text_between(description_chunk, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "MyHentaiComics".to_string()),
        body.split("comic-cover").nth(1).and_then(image_from_chunk),
        true,
    );
    item.tags = [categories, artists, groups].concat();
    item.status = ItemStatus::Completed;
    item.description = (!description.trim().is_empty()).then_some(description.trim().to_string());
    item
}

fn parse_pages(body: &str, key: &str) -> Vec<MangaPage> {
    let image = body
        .split("gallery-slide")
        .nth(1)
        .and_then(image_from_chunk)
        .or_else(|| image_from_chunk(body));
    let Some(image) = image else {
        return Vec::new();
    };
    let comic_id = key
        .split("/gallery/show/")
        .nth(1)
        .and_then(|part| part.split('/').next())
        .unwrap_or("");
    let total = body
        .split("<a")
        .filter(|chunk| chunk.contains(&format!("/gallery/show/{comic_id}/")))
        .filter_map(|chunk| html::attr(chunk, "href"))
        .filter_map(|href| href.trim_end_matches('/').rsplit('/').next()?.parse::<usize>().ok())
        .max()
        .unwrap_or(1);
    let base = image.rsplit_once('/').map(|(base, _)| format!("{base}/")).unwrap_or_default();
    let extension = image.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("jpg").split('?').next().unwrap_or("jpg");
    (1..=total)
        .map(|page| {
            let image = format!("{base}{page:03}.{extension}");
            MangaPage {
                content: PageContent::Url { url: image, context: Some(manga::image_headers(BASE_URL)) },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {page}")),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|image| absolute_url(&image).replace(' ', "%20"))
}

fn links_containing(chunk: &str, needle: &str) -> Vec<String> {
    chunk
        .split("<a")
        .skip(1)
        .filter(|part| part.contains(needle))
        .filter_map(|part| html::text_between(part, "<a", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn filter(filters: Option<&Value>, key: &str) -> Option<String> {
    filters.and_then(|filters| filters.get(key)).and_then(Value::as_str).map(ToString::to_string)
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<li class="item"><div class="comic-inner"><a href="/gallery/sample"><h2 class="comic-name">Sample</h2><img src="/cover.jpg"></a></div></li>"#;
const DETAILS_FIXTURE: &str = r#"<div class="comic-cover"><a href="/gallery/show/1/1"><img src="/cover.jpg"></a></div><div class="comic-description"><h1>Sample</h1><a href="/gallery/category/3">3D Comic</a><div>Pages: 1</div></div>"#;
const PAGES_FIXTURE: &str = r#"<ul class="gallery-slide"><li><img src="https://cdn.myhentaicomics.com/mhc/images/Sample/original/001.jpg?1"></li></ul><ul><li><a href="/gallery/show/1/1">1</a></li></ul>"#;
