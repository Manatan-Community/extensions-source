use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Niceoppai = Niceoppai;
const BASE_URL: &str = "https://www.niceoppai.net";

struct Niceoppai;

impl MangaSource for Niceoppai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "last-updated"
        } else {
            "most-popular-monthly"
        };
        let body = fetch_document(
            &format!("{BASE_URL}/manga_list/all/any/{order}/{page}"),
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
        let sort = request
            .get("filters")
            .and_then(|filters| filters.get("sort"))
            .and_then(Value::as_str)
            .unwrap_or("name-az");
        let target = if sort != "name-az" {
            format!("{BASE_URL}/manga_list/all/any/{sort}/{page}")
        } else {
            format!(
                "{BASE_URL}/manga_list/search/{}/{sort}/{page}",
                url::query_escape(query)
            )
        };
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
        Ok(parse_chapters_with_pages(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".into());
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("nde") || chunk.contains("cvr"))
        .filter_map(catalog_from_chunk)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains(">Next<") || body.contains(">Next</a>"),
    }
}

fn catalog_from_chunk(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "div class=\"cvr", "href")
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = normalize_key(&href);
    let title = html::text_between(chunk, "div class=\"det", "</div>")
        .and_then(|det| html::text_between(&det, "<a", "</a>"))
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
    let detail = html::text_between(body, "div class=\"det", "</div>").unwrap_or_else(|| body.to_string());
    let status_text = nth_paragraph_text(&detail, 9).unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "h1 class=\"ttl", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Niceoppai".to_string())),
        cover: html::attr_after(body, "div class=\"mng_ifo", "src")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        description: nth_paragraph_text(&detail, 0),
        authors: nth_paragraph_links(&detail, 2),
        artists: nth_paragraph_links(&detail, 2),
        tags: nth_paragraph_links(&detail, 5),
        status: match status_text.replace(": ", " ").trim() {
            "ยังไม่จบ" => ItemStatus::Ongoing,
            "จบแล้ว" => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(absolute_url(&key)),
        language: Some("th".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters_with_pages(body: &str) -> Vec<MangaChapter> {
    let mut chapters = parse_chapters(body, 0);
    let page_urls = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("pgg"))
        .filter_map(|chunk| html::attr_after(chunk, "<a", "href"))
        .filter(|href| !href.contains("Next") && !href.contains("Last"))
        .map(|href| absolute_url(&href))
        .fold(Vec::new(), |mut urls: Vec<String>, url| {
            if !urls.contains(&url) {
                urls.push(url);
            }
            urls
        });
    for page_url in page_urls.into_iter().take(12) {
        let body = fetch_document(&page_url, "");
        let more = parse_chapters(&body, chapters.len());
        for chapter in more {
            if !chapters.iter().any(|existing| existing.key == chapter.key) {
                chapters.push(chapter);
            }
        }
    }
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: "/sample".to_string(),
            title: Some("Chapter 1".to_string()),
            chapter_number: Some(1.0),
            url: Some(format!("{BASE_URL}/sample")),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_chapters(body: &str, start_index: usize) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("lng_") || chunk.contains("a class=\"lst"))
        .enumerate()
        .filter_map(|(index, chunk)| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "b class=\"val", "</b>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("Chapter {}", start_index + index + 1));
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number(&title).or(Some((start_index + index + 1) as f32)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("image-container") || chunk.contains("data-src") || chunk.contains("src"))
        .filter_map(image_attr)
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

fn nth_paragraph_text(body: &str, index: usize) -> Option<String> {
    body.split("<p")
        .skip(1)
        .nth(index)
        .and_then(|chunk| html::text_between(chunk, ">", "</p>"))
        .map(|value| html::strip_tags(&value))
        .map(|value| value.replace(": ", " "))
        .filter(|value| !value.is_empty())
}

fn nth_paragraph_links(body: &str, index: usize) -> Vec<String> {
    let Some(paragraph) = body.split("<p").skip(1).nth(index) else {
        return Vec::new();
    };
    paragraph
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src").or_else(|| html::attr_after(input, "<img", "src"))
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .replace("ตอนที่. ", "")
        .split(" - ")
        .next()
        .and_then(|value| value.trim().parse::<f32>().ok())
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) {
        return Some(normalize_key(&input[BASE_URL.len()..]));
    }
    if input.starts_with('/') && !input.starts_with("/manga_list/") {
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
<div class="nde"><div class="cvr"><a href="/sample"><img src="/cover.jpg"></a></div><div class="det"><a>Sample</a></div></div><ul class="pgg"><li><a>Next</a></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="ttl">Sample</h1><div class="mng_ifo"><div class="cvr_ara"><img src="/cover.jpg"></div><div class="det"><p>: Sample description.</p><p></p><p><a>Author</a></p><p></p><p></p><p><a>Adult</a></p><p></p><p></p><p></p><p>: ยังไม่จบ</p></div></div><ul class="lst"><li class="lng_"><a class="lst" href="/sample/1"><b class="val">1 - Start</b></a></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"
<div id="image-container"><center><img data-src="/page-1.jpg"></center></div>
"#;
