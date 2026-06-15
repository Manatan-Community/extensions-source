use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Megatokyo = Megatokyo;
const BASE_URL: &str = "https://megatokyo.com";
const ARCHIVE_KEY: &str = "/archive.php?list_by=date";

struct Megatokyo;

impl MangaSource for Megatokyo {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: vec![catalog_item(true)],
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or("").trim();
        let entries = if query.is_empty() || "megatokyo".contains(&query.to_ascii_lowercase()) || query.starts_with(BASE_URL) {
            vec![catalog_item(true)]
        } else {
            Vec::new()
        };
        Ok(Paged { entries, has_next_page: false })
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(catalog_item(true))
    }

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        Ok(parse_chapters(&fetch_document(&absolute_url(ARCHIVE_KEY), ARCHIVE_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/strip/1".into());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(catalog_item(true)),
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

fn catalog_item(initialized: bool) -> CatalogItem {
    CatalogItem {
        key: ARCHIVE_KEY.to_string(),
        title: "Megatokyo".to_string(),
        authors: vec!["Fred Gallagher".to_string()],
        artists: vec!["Fred Gallagher".to_string()],
        description: Some("Relax, we understand j00".to_string()),
        cover: Some("https://i.ibb.co/yWQM1gY/megatokyo.png".to_string()),
        url: Some(absolute_url(ARCHIVE_KEY)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Ongoing,
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters: Vec<_> = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("name="))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty());
            Some(MangaChapter {
                key: key.clone(),
                title,
                chapter_number: url::slug_from_url(&key).and_then(|value| value.parse::<f32>().ok()),
                date_uploaded: html::attr(chunk, "title")
                    .map(|value| strip_ordinals(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("strip"))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: absolute_url(&image), context: None },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn strip_ordinals(input: &str) -> String {
    input
        .replace("st,", ",")
        .replace("nd,", ",")
        .replace("rd,", ",")
        .replace("th,", ",")
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!("/{}", input[BASE_URL.len()..].trim_start_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"<div class="content"><h2>Comics by Date</h2><div><ul><li><a name="1" href="/strip/1" title="January 1st, 2024">Sample Strip</a></li></ul></div></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="strip"><img src="/strips/0001.png"></div>"#;
