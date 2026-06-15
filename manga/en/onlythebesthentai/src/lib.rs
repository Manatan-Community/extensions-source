use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: OnlyTheBestHentai = OnlyTheBestHentai;
const BASE_URL: &str = "https://onlythebesthentai.com";

struct OnlyTheBestHentai;

impl MangaSource for OnlyTheBestHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document(&page_url(BASE_URL, page), LIST_FIXTURE)))
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
        let target = if query.is_empty() {
            let filters = request.get("filters");
            let path = filter_path(filters).unwrap_or_else(|| BASE_URL.to_string());
            page_url(&path, page)
        } else {
            let suffix = if page > 1 { format!("&paged={page}") } else { String::new() };
            format!("{BASE_URL}/?s={}{}", url::query_escape(query), suffix)
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let page_count = page_count(&body);
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some(page_count.map(|count| format!("Chapter [{count} pages]")).unwrap_or_else(|| "Chapter".to_string())),
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
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn page_url(base: &str, page: u64) -> String {
    if page > 1 {
        format!("{}/page/{page}/", base.trim_end_matches('/'))
    } else {
        format!("{}/", base.trim_end_matches('/'))
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("article")
        .skip(1)
        .filter(|chunk| chunk.contains("post"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "entry-title", "href")
                .or_else(|| html::attr_after(chunk, "blog-entry-title", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "entry-title", "</")
                .or_else(|| html::text_between(chunk, "blog-entry-title", "</"))
                .map(|value| html::strip_tags(&value).split('[').next().unwrap_or("").trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Only The Best Hentai".to_string()));
            Some(catalog_item(key, title, image_from_chunk(chunk), false))
        })
        .collect();
    Paged { entries, has_next_page: body.contains("next page-numbers") }
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
    let key = key.unwrap_or_else(|| "/sample".to_string());
    let mut item = catalog_item(
        key,
        html::text_between(body, "manga-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Only The Best Hentai".to_string()),
        body.split("manga-box").nth(1).and_then(image_from_chunk).or_else(|| image_from_chunk(body)),
        true,
    );
    item.tags = tag_group(body, "Tags");
    item.authors = tag_group(body, "Artist");
    item.artists = item.authors.clone();
    item.description = Some(build_description(body));
    item.status = ItemStatus::Completed;
    item
}

fn build_description(body: &str) -> String {
    let mut lines = Vec::new();
    let parodies = tag_group(body, "Parody");
    if !parodies.is_empty() {
        lines.push(format!("Parody: {}", parodies.join(", ")));
    }
    let characters = tag_group(body, "Characters");
    if !characters.is_empty() {
        lines.push(format!("Characters: {}", characters.join(", ")));
    }
    if let Some(count) = page_count(body) {
        lines.push(format!("Pages: {count}"));
    }
    if let Some(text) = html::text_between(body, "manga-info", "</p>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()) {
        lines.push(text.trim_start_matches("Description:").trim().to_string());
    }
    lines.join("\n")
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body
        .split("wp-block-image")
        .skip(1)
        .filter_map(|chunk| image_from_srcset(chunk).or_else(|| image_from_chunk(chunk)))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: image, context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn tag_group(body: &str, label: &str) -> Vec<String> {
    body
        .split("manga-tags-container")
        .filter(|chunk| chunk.contains(label))
        .flat_map(|chunk| chunk.split("tag-button").skip(1))
        .filter_map(|chunk| html::text_between(chunk, "<", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn page_count(body: &str) -> Option<u32> {
    body.split("manga-tags-container")
        .find(|chunk| chunk.contains("Pages"))
        .and_then(|chunk| {
            let digits = chunk.chars().filter(|ch| ch.is_ascii_digit()).collect::<String>();
            digits.parse().ok()
        })
}

fn filter_path(filters: Option<&Value>) -> Option<String> {
    let filters = filters?;
    for (key, prefix) in [("tag", "tag"), ("parody", "parody"), ("character", "characters"), ("artist", "artist")] {
        if let Some(value) = filters.get(key).and_then(Value::as_str).filter(|value| !value.is_empty()) {
            return Some(format!("{BASE_URL}/{prefix}/{value}/"));
        }
    }
    None
}

fn image_from_srcset(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "srcset").and_then(|srcset| {
        srcset
            .split(',')
            .map(str::trim)
            .filter_map(|entry| {
                let mut parts = entry.split_whitespace();
                let url = parts.next()?.to_string();
                let width = parts.next().and_then(|value| value.trim_end_matches('w').parse::<u32>().ok()).unwrap_or(0);
                Some((width, url))
            })
            .max_by_key(|(width, _)| *width)
            .map(|(_, url)| url)
    })
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|image| absolute_url(&image))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<article class="post"><h2 class="entry-title"><a href="/sample">Sample [1]</a></h2><img src="/cover.jpg"></article>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="manga-title">Sample</h1><div class="manga-box"><img src="/cover.jpg"></div><div class="manga-tags-container"><span class="manga-tags-label">Pages</span>1</div><div class="manga-gallery-wrapper"><figure class="wp-block-image"><img src="/page1.jpg"></figure></div>"#;
const PAGES_FIXTURE: &str = DETAILS_FIXTURE;
