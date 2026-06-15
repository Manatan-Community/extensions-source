use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Oots = Oots;
const BASE_URL: &str = "https://www.giantitp.com";
const ARCHIVE: &str = "/comics/oots.html";

struct Oots;

impl MangaSource for Oots {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged { entries: vec![series_item()], has_next_page: false })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        let entries = if query.is_empty() || "the order of the stick".contains(&query.to_ascii_lowercase()) {
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
        Ok(parse_chapters(&fetch_document(&absolute_url(ARCHIVE), CHAPTERS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| ARCHIVE.to_string());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
    }

    fn manga_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(Some(absolute_url(ARCHIVE)))
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
        key: ARCHIVE.to_string(),
        title: "The Order Of The Stick".to_string(),
        cover: Some("https://i.giantitp.com/redesign/Icon_Comics_OOTS.gif".to_string()),
        authors: vec!["Rich Burlew".to_string()],
        artists: vec!["Rich Burlew".to_string()],
        description: Some("Having fun with games.".to_string()),
        status: ItemStatus::Ongoing,
        url: Some(absolute_url(ARCHIVE)),
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
            if !href.contains("oots") || !href.ends_with(".html") {
                return None;
            }
            let key = normalize_key(&href);
            let number = href
                .split("oots")
                .nth(1)
                .and_then(|part| part.split(".html").next())
                .and_then(|part| part.parse::<f32>().ok());
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
                chapter_number: number,
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|image| image.contains("oots") || image.contains("comics"))
        .take(1)
        .enumerate()
        .map(|(index, image)| {
            let image = url::join_url(BASE_URL, &image);
            MangaPage {
                content: PageContent::Url { url: image, context: Some(manga::image_headers(BASE_URL)) },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
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

const CHAPTERS_FIXTURE: &str = r#"<p class="ComicList"><a href="/comics/oots0001.html">#1 Sample</a></p>"#;
const PAGES_FIXTURE: &str = r#"<td align="center"><img src="/comics/images/oots0001.gif"></td>"#;
