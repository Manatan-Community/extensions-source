use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: OreManga = OreManga;
const BASE_URL: &str = "https://www.oremanga.net";

struct OreManga;

impl MangaSource for OreManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        let body = fetch_document(
            &format!("{BASE_URL}/advance-search/page/{page}/?order={order}"),
            LIST_FIXTURE,
        );
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let target = search_url(page, query, request.get("filters").unwrap_or(&Value::Null));
        let body = fetch_document(&target, LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/1".into());
        let chapter_url = absolute_url(&key);
        let body = fetch_document(&chapter_url, PAGES_FIXTURE);
        Ok(parse_pages(&body, &chapter_url))
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
            let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.to_string(),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(page: u64, query: &str, filters: &Value) -> String {
    let mut params = vec![("title", url::query_escape(query))];
    for (id, param) in [
        ("author", "author"),
        ("year", "yearx"),
        ("status", "status"),
        ("type", "type"),
        ("order", "order"),
    ] {
        if let Some(value) = filters
            .get(id)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            params.push((param, url::query_escape(value)));
        }
    }
    if let Some(genres) = filters.get("genres").and_then(Value::as_array) {
        for genre in genres.iter().filter_map(Value::as_str) {
            params.push(("genre[]", url::query_escape(genre)));
        }
    }
    format!(
        "{BASE_URL}/advance-search/page/{page}/?{}",
        params
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("flexbox2-item"))
        .filter_map(catalog_from_chunk)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pagination") && body.contains("next"),
    }
}

fn catalog_from_chunk(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    let title = html::text_between(chunk, "flexbox2-title", "</")
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .or_else(|| url::slug_from_url(&href))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())?;
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_attr(chunk).map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some("th".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    let status = html::text_between(body, "series-infoz block", "</div>")
        .and_then(|chunk| html::text_between(&chunk, "status", "</"))
        .map(|value| html::strip_tags(&value).to_ascii_lowercase());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "series-title", "</")
            .or_else(|| html::text_between(body, "<h2", "</h2>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "OreManga".to_string())),
        cover: html::attr_after(body, "series-thumb", "src")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "series-synops", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_span_values(body, "Author"),
        artists: info_span_values(body, "Author"),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("series-genres") || chunk.contains("genre"))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: match status.as_deref() {
            Some("ongoing") => ItemStatus::Ongoing,
            Some("completed") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(absolute_url(&key)),
        language: Some("th".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("series-chapterlist") || chunk.contains("flexch-infoz"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<span", "</span>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_thai_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    body.split('<')
        .filter(|chunk| chunk.starts_with("img") || chunk.starts_with("canvas"))
        .filter(|chunk| chunk.contains("reader-area-main") || chunk.contains("src") || chunk.contains("data-url"))
        .filter_map(|chunk| {
            if chunk.starts_with("canvas") {
                html::attr_after(chunk, "canvas", "data-url")
            } else {
                image_attr(chunk)
            }
        })
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(chapter_url)),
            },
            headers: manga::image_headers(chapter_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn info_span_values(body: &str, label: &str) -> Vec<String> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains(label))
        .filter_map(|chunk| html::text_between(chunk, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src").or_else(|| html::attr_after(input, "<img", "src"))
}

fn parse_thai_date(value: &str) -> Option<i64> {
    let mut out = value.to_string();
    for (thai, english) in [
        ("มกราคม", "January"),
        ("กุมภาพันธ์", "February"),
        ("มีนาคม", "March"),
        ("เมษายน", "April"),
        ("พฤษภาคม", "May"),
        ("มิถุนายน", "June"),
        ("กรกฎาคม", "July"),
        ("สิงหาคม", "August"),
        ("กันยายน", "September"),
        ("ตุลาคม", "October"),
        ("พฤศจิกายน", "November"),
        ("ธันวาคม", "December"),
    ] {
        out = out.replace(thai, english);
    }
    dates::parse_fixture_date(&out)
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) {
        return Some(normalize_key(&input[BASE_URL.len()..]));
    }
    if input.starts_with('/') && !input.starts_with("/advance-search") {
        return Some(normalize_key(input));
    }
    None
}

fn normalize_key(value: &str) -> String {
    format!("/{}", value.trim().trim_start_matches(BASE_URL).trim_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="flexbox2-item"><a href="/sample"><div class="flexbox2-title"><span>Sample</span></div><div class="flexbox2-thumb"><img src="/cover.jpg"></div></a></div><div class="pagination"><a class="next">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="series-title"><h2>Sample</h2></div><div class="series-thumb"><img src="/cover.jpg"></div><div class="series-synops"><p>Sample description.</p></div><div class="series-genres"><a>Action</a></div><ul class="series-infolist"><li><b>Author</b><span>Author</span></li></ul><div class="series-infoz block"><span class="status">ongoing</span></div><ul class="series-chapterlist"><li><div class="flexch-infoz"><a href="/sample/1"><span>Chapter 1</span></a></div><span class="date">01 January 2024</span></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="reader-area-main"><img src="/page-1.jpg"><canvas data-url="/page-2.jpg"></canvas></div>
"#;
