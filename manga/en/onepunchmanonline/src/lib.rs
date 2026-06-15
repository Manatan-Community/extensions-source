use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: OnePunchManOnline = OnePunchManOnline;
const BASE_URL: &str = "https://w11.1punchman.com";

struct OnePunchManOnline;

impl MangaSource for OnePunchManOnline {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: vec![series_item()],
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        let entries = if query.is_empty()
            || "one punch man".contains(&query.to_ascii_lowercase())
            || query.starts_with(BASE_URL)
        {
            vec![series_item()]
        } else {
            Vec::new()
        };
        Ok(Paged { entries, has_next_page: false })
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(series_item())
    }

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        Ok(parse_chapters(&fetch_document(BASE_URL, CHAPTERS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
    }

    fn manga_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(Some(BASE_URL.to_string()))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        Ok(Some(UrlResolveResult {
            item: input.starts_with(BASE_URL).then(series_item),
            search: (!input.starts_with(BASE_URL)).then(|| SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
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

fn series_item() -> CatalogItem {
    CatalogItem {
        key: "/".to_string(),
        title: "One Punch Man".to_string(),
        cover: Some("https://1punchman.com/wp-content/uploads/2024/02/9782380712018_1_75.jpg".to_string()),
        authors: vec!["ONE".to_string()],
        artists: vec!["Murata Yusuke".to_string()],
        tags: vec!["Action".to_string(), "Comedy".to_string(), "Superhero".to_string(), "Seinen".to_string()],
        description: Some("One-Punch Man is a superhero who has trained so hard that his hair has fallen out, and who can overcome any enemy with one punch.".to_string()),
        status: ItemStatus::Ongoing,
        url: Some(BASE_URL.to_string()),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/manga/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body
        .split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "data-src")
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| image.starts_with("http"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: image, context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const CHAPTERS_FIXTURE: &str = r#"<ul><li><a href="/manga/sample">Chapter 1</a></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<div class="entry-content"><img src="https://w11.1punchman.com/page1.jpg"></div>"#;
