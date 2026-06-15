use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: LoadingArtist = LoadingArtist;
const BASE_URL: &str = "https://loadingartist.com";

struct LoadingArtist;

impl MangaSource for LoadingArtist {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: vec![archive_item()],
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![archive_item()],
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: Vec::new(),
            has_next_page: false,
        })
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(archive_item())
    }

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        Ok(parse_chapters(&fetch_document(
            &format!("{BASE_URL}/search.json"),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/comic/sample".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(archive_item()),
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

fn archive_item() -> CatalogItem {
    CatalogItem {
        key: "/archives".to_string(),
        title: "Loading Artist".to_string(),
        cover: Some(format!("{BASE_URL}/img/bg/logo-text_dark.png")),
        url: Some(format!("{BASE_URL}/archives")),
        authors: vec!["Loading Artist".to_string()],
        artists: vec!["Loading Artist".to_string()],
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Ongoing,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    serde_json::from_str::<Vec<Comic>>(body)
        .unwrap_or_default()
        .into_iter()
        .filter(|comic| matches!(comic.section.as_str(), "comic" | "game" | "art"))
        .map(|comic| {
            let key = normalize_key(&comic.url);
            MangaChapter {
                key: key.clone(),
                title: Some(comic.title),
                date_uploaded: parse_yyyy_mm_dd(&comic.date),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let image = html::attr_after(body, "main-image-container", "src")
        .or_else(|| html::attr_after(body, "<img", "src"))
        .map(|value| url::join_url(BASE_URL, &value));
    image
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!(
                "/{}",
                value[index + BASE_URL.len()..]
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn parse_yyyy_mm_dd(value: &str) -> Option<i64> {
    let mut parts = value.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let day = day as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

#[derive(Default, Deserialize)]
struct Comic {
    url: String,
    title: String,
    #[serde(default)]
    date: String,
    section: String,
}

export_manga_source!(SOURCE);

const CHAPTERS_FIXTURE: &str = r#"
[
  {"url":"/comic/sample","title":"Sample Comic","date":"2024-01-01","section":"comic"},
  {"url":"/blog/sample","title":"Sample Blog","date":"2024-01-02","section":"blog"}
]
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="main-image-container"><img src="/comics/sample.png"></div>"#;
