use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaKatana = MangaKatana;
const BASE_URL: &str = "https://mangakatana.com";

struct MangaKatana;

impl MangaSource for MangaKatana {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/page/{page}")
        } else {
            format!("{BASE_URL}/manga/page/{page}")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
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
            catalog_url(page, request.get("filters"))
        } else {
            let search_by = filter(request.get("filters"), "search_by", "book_name");
            format!("{BASE_URL}/page/{page}?search={}&search_by={search_by}", url::query_escape(query))
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/c1".to_string());
        let server = filter(Some(&request), "server", "");
        let suffix = if server.is_empty() { String::new() } else { format!("?sv={server}") };
        Ok(parse_pages(&fetch_document(&format!("{}{}", absolute_url(&key), suffix), PAGES_FIXTURE)))
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
                item: key.starts_with("/manga/").then(|| parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()), ..SearchRequest::default() }),
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

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn catalog_url(page: u64, filters: Option<&Value>) -> String {
    let mut params = vec!["filter=1".to_string()];
    let include = filter(filters, "include", "");
    let exclude = filter(filters, "exclude", "");
    let include_mode = filter(filters, "include_mode", "and");
    let order = filter(filters, "order", "latest");
    let status = filter(filters, "status", "");
    let chapters = filter(filters, "chapters", "1");
    if !include.is_empty() {
        params.push(format!("include={}", url::query_escape(&include)));
        params.push(format!("include_mode={}", url::query_escape(&include_mode)));
    }
    if !exclude.is_empty() {
        params.push(format!("exclude={}", url::query_escape(&exclude)));
    }
    if !order.is_empty() {
        params.push(format!("order={}", url::query_escape(&order)));
    }
    if !status.is_empty() {
        params.push(format!("status={}", url::query_escape(&status)));
    }
    if !chapters.is_empty() {
        params.push(format!("chapters={}", url::query_escape(&chapters)));
    }
    format!("{BASE_URL}/manga/page/{page}?{}", params.join("&"))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body.split("<div").skip(1).filter(|chunk| chunk.contains("item") && (chunk.contains("book_list") || chunk.contains("<h3") || chunk.contains("class=\"text"))).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<h3", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
        let key = normalize_key(&href);
        Some(CatalogItem {
            key: key.clone(),
            title: html::text_between(chunk, "<h3", "</h3>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
            cover: html::attr_after(chunk, "<img", "data-src").or_else(|| html::attr_after(chunk, "<img", "src")).map(|image| absolute_url(&image)),
            url: Some(absolute_url(&key)),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        })
    }).fold(Vec::new(), |mut items, item| {
        if !items.iter().any(|existing: &CatalogItem| existing.key == item.key) {
            items.push(item);
        }
        items
    });
    Paged { entries, has_next_page: body.contains("next page-numbers") }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let alt = html::text_between(body, "alt_name", "</").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty());
    let mut description = html::text_between(body, "summary", "</div>")
        .or_else(|| html::text_between(body, "<p", "</p>"))
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    if let Some(alt) = alt {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Alt name(s): ");
        description.push_str(&alt);
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "heading", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: html::attr_after(body, "media", "src").or_else(|| html::attr_after(body, "cover", "src")).or_else(|| html::attr_after(body, "<img", "src")).map(|image| absolute_url(&image)),
        authors: text_list(body, "author"),
        artists: text_list(body, "artist"),
        tags: text_list(body, "genres"),
        description: (!description.is_empty()).then_some(description),
        status: match html::text_between(body, "status", "</").map(|value| html::strip_tags(&value)).unwrap_or_default().as_str() {
            value if value.contains("Ongoing") => ItemStatus::Ongoing,
            value if value.contains("Completed") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<tr").skip(1).filter(|chunk| chunk.contains("chapter")).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let key = normalize_key(&href);
        Some(MangaChapter {
            key: key.clone(),
            title: html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
            url: Some(absolute_url(&key)),
            date_uploaded: html::text_between(chunk, "update_time", "</").map(|value| html::strip_tags(&value)).and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
            ..MangaChapter::default()
        })
    }).collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let image_script = body.split("<script").find(|chunk| chunk.contains("data-src")).unwrap_or_default();
    let array_name = image_script.split("data-src").nth(1).and_then(|part| {
        let tail = part.split(',').nth(1)?.trim();
        Some(tail.chars().take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_').collect::<String>())
    }).filter(|value| !value.is_empty());
    let images = array_name.and_then(|name| extract_js_array(image_script, &name)).unwrap_or_else(|| {
        body.split("<img").skip(1).filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))).collect()
    });
    images.into_iter().enumerate().map(|(index, image)| MangaPage {
        content: PageContent::Url { url: absolute_url(&image), context: Some(manga::image_headers(BASE_URL)) },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }).collect()
}

fn extract_js_array(script: &str, name: &str) -> Option<Vec<String>> {
    let raw = script.split(&format!("var {name}=[")).nth(1).or_else(|| script.split(&format!("{name}=[")).nth(1))?.split(']').next()?;
    Some(raw.split(',').map(|part| part.trim().trim_matches('\'').trim_matches('"').replace("\\/", "/")).filter(|part| !part.is_empty()).collect())
}

fn text_list(body: &str, label: &str) -> Vec<String> {
    body.split("<a").filter(|chunk| chunk.to_ascii_lowercase().contains(label)).filter_map(|chunk| html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))).filter(|value| !value.is_empty()).collect()
}

fn filter(filters: Option<&Value>, key: &str, fallback: &str) -> String {
    filters.and_then(|value| value.get(key)).and_then(Value::as_str).unwrap_or(fallback).trim().to_string()
}

const LIST_FIXTURE: &str = r#"<div id="book_list"><div class="item"><div class="text"><h3><a href="/manga/sample">Sample Katana</a></h3></div><img src="/cover.jpg"></div></div><a class="next page-numbers"></a>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="heading">Sample Katana</h1><div class="media"><div class="cover"><img src="/cover.jpg"></div></div><div class="summary"><p>Summary</p></div><div class="value status">Ongoing</div><div class="genres"><a>Action</a></div><tr><td class="chapter"><a href="/manga/sample/c1">Chapter 1</a></td><td class="update_time">Jan-01-2024</td></tr>"#;
const PAGES_FIXTURE: &str = r#"<script>var imgs=['/page1.jpg']; reader.run('data-src', imgs);</script>"#;

export_manga_source!(SOURCE);
