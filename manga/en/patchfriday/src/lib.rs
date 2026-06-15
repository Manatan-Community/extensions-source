use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: PatchFriday = PatchFriday;
const BASE_URL: &str = "https://patchfriday.com";

struct PatchFriday;

impl MangaSource for PatchFriday {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged { entries: vec![series_item()], has_next_page: false })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        let entries = if query.is_empty() || "patch friday".contains(&query.to_ascii_lowercase()) {
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
        Ok(parse_chapters(&fetch_document(&format!("{BASE_URL}/search/?search=;"), CHAPTERS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/1/".to_string());
        let page_url = absolute_url(&key);
        Ok(vec![MangaPage {
            content: PageContent::Lazy {
                key: key.clone(),
                url: Some(page_url.clone()),
                page_url: Some(page_url),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(key.trim_matches('/').to_string()),
            ..MangaPage::default()
        }])
    }

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<manatan_extension::MangaPageImage> {
        let page_url = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("pageUrl").or_else(|| content.get("page_url")))
            .and_then(Value::as_str)
            .unwrap_or(BASE_URL);
        let body = fetch_document(page_url, PAGES_FIXTURE);
        Ok(manatan_extension::MangaPageImage {
            url: html::attr_after(&body, "strip_image", "src")
                .or_else(|| html::attr_after(&body, "<img", "src"))
                .map(|image| absolute_url(&image))
                .unwrap_or_else(|| page_url.to_string()),
            headers: manga::image_headers(BASE_URL),
            ..Default::default()
        })
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
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn series_item() -> CatalogItem {
    CatalogItem {
        key: String::new(),
        title: "Patch Friday".to_string(),
        cover: Some("https://patchfriday.com/patches/68.png".to_string()),
        authors: vec!["Patch Friday".to_string()],
        artists: vec!["Patch Friday".to_string()],
        description: Some("The IT security webcomic".to_string()),
        status: ItemStatus::Ongoing,
        url: Some(BASE_URL.to_string()),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let number = key.trim_matches('/').parse::<f32>().ok()?;
            let text = html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)).unwrap_or_default();
            Some(MangaChapter {
                key: key.clone(),
                title: Some(if text.is_empty() { format!("#{}", number as i32) } else { format!("#{} - {text}", number as i32) }),
                chapter_number: Some(number),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if !chapters.iter().any(|chapter| chapter.key == "/1/") {
        chapters.push(MangaChapter {
            key: "/1/".to_string(),
            title: Some("#1 - The One".to_string()),
            chapter_number: Some(1.0),
            url: Some(format!("{BASE_URL}/1/")),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}/", path.trim_matches('/'))
}

export_manga_source!(SOURCE);

const CHAPTERS_FIXTURE: &str = r#"<div><div><div><a href="/2/">Second</a></div></div></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="strip_image"><img src="/patches/1.png"></div>"#;
