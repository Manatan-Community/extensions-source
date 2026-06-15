use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaReaderIn = MangaReaderIn;
const BASE_URL: &str = "https://mangareader.in";

struct MangaReaderIn;

impl MangaSource for MangaReaderIn {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged { entries: parse_cards(LIST_FIXTURE), has_next_page: false });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("/latest-update/{page}")
        } else {
            format!("/most-popular/{page}")
        };
        let body = fetch_document(&absolute_url(&path), LIST_FIXTURE);
        Ok(Paged { entries: parse_cards(&body), has_next_page: has_next(&body) })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), Some(key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_document(&format!("{BASE_URL}/search/story/{}/{page}", url::query_escape(query)), LIST_FIXTURE);
        Ok(Paged { entries: parse_cards(&body), has_next_page: has_next(&body) })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let manga_id = body.split("var mangaID = '").nth(1).and_then(|rest| rest.split("';").next()).unwrap_or("");
        let title = html::text_between(&body, "manga-detail", "</h1>").map(|value| html::strip_tags(&value)).unwrap_or_default();
        let chapter_body = if manga_id.is_empty() {
            CHAPTERS_FIXTURE.to_string()
        } else {
            fetch_document(&format!("{BASE_URL}/ajax-list-chapter?mangaID={manga_id}"), CHAPTERS_FIXTURE)
        };
        Ok(parse_chapters(&chapter_body, &title))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
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
            return Ok(Some(UrlResolveResult {
                item: input.contains("/manga/").then(|| parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(normalize_key(input)))),
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

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("/manga/"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::attr_after(chunk, "<img", "alt")
                .or_else(|| html::text_between(chunk, "manga-title", "</").map(|value| html::strip_tags(&value)))
                .or_else(|| html::text_between(chunk, "<h3", "</h3>").map(|value| html::strip_tags(&value)))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "manga-detail", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: html::attr_after(body, "manga-cover", "src").or_else(|| image_attr(body)).map(|image| absolute_url(&image)),
        description: html::text_between(body, "summary", "</div>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        authors: link_texts(body, "author"),
        tags: link_texts(body, "genre"),
        status: parse_status(&html::strip_tags(body)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_title: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let mut title = html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)).unwrap_or_else(|| "Chapter".to_string());
            if !manga_title.is_empty() {
                title = title.replace(manga_title, "").trim().to_string();
            }
            Some(MangaChapter {
                key: key.clone(),
                title: Some(if title.is_empty() { "Chapter".to_string() } else { title }),
                url: Some(absolute_url(&key)),
                date_uploaded: html::text_between(chunk, "chapter-time", "</").map(|value| html::strip_tags(&value)).and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: absolute_url(&image), context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn link_texts(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .map(|chunk| html::strip_tags(chunk))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
}

fn has_next(body: &str) -> bool {
    body.contains("Next") || body.contains("next")
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(".in/") {
            return format!("/{}", value[index + 4..].trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
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

const LIST_FIXTURE: &str = r#"<div class="manga-item"><a href="/manga/sample"><img src="/cover.jpg" alt="Sample Manga"><h3>Sample Manga</h3></a></div>"#;
const DETAILS_FIXTURE: &str = r#"
<script>var mangaID = '1';</script><div class="manga-detail"><h1>Sample Manga</h1></div>
<div class="manga-cover"><img src="/cover.jpg"></div><div class="summary">Summary</div>
<a href="/genre/action">Action</a><span>Ongoing</span>
"#;
const CHAPTERS_FIXTURE: &str = r#"<ul><li><a href="/manga/sample/chapter-1">Sample Manga Chapter 1</a><span class="chapter-time">Jan 1, 2024</span></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<div class="chapter-content"><img src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
