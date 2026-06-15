use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: SolarAndSundry = SolarAndSundry;
const BASE_URL: &str = "https://sas-api.fly.dev";
const ARCHIVE_URL: &str = "https://sas.ewanb.me";
const COVER_URL: &str =
    "https://imagedelivery.net/zthi1l8fKrUGB5ig08mq-Q/de292ba7-f164-4f43-ec17-1876a7a44600/public";

struct SolarAndSundry;

impl MangaSource for SolarAndSundry {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: vec![series_item(false)],
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let entries = if query.is_empty()
            || "solar and sundry".contains(&query)
            || query.starts_with(ARCHIVE_URL)
        {
            vec![series_item(false)]
        } else {
            Vec::new()
        };
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(series_item(true))
    }

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let pages: Vec<SasPage> = fetch_json_or_fixture(&format!("{BASE_URL}/page"), PAGES_FIXTURE);
        Ok(pages
            .into_iter()
            .rev()
            .map(|page| MangaChapter {
                key: format!("/page/{}", page.page_number),
                title: Some(page.name),
                chapter_number: Some(page.page_number as f32),
                url: Some(format!("{ARCHIVE_URL}/comic/{}", page.page_number)),
                language: Some("en".to_string()),
                ..MangaChapter::default()
            })
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/page/1".to_string());
        let page: SasPage = fetch_json_or_fixture(
            &url::join_url(BASE_URL, &key),
            r#"{"page_number":1,"chapter_number":1,"image_url":"https://imagedelivery.net/sample/page","thumbnail_url":"https://imagedelivery.net/sample/thumb","name":"Page 1","published_at":"2024-01-01T00:00:00Z"}"#,
        );
        Ok(vec![MangaPage {
            content: PageContent::Url {
                url: page.image_url,
                context: None,
            },
            headers: manga::image_headers(ARCHIVE_URL),
            description: Some(page.name),
            ..MangaPage::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(ARCHIVE_URL) || input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(series_item(true)),
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

fn series_item(initialized: bool) -> CatalogItem {
    CatalogItem {
        key: "/page".to_string(),
        title: "Solar and Sundry".to_string(),
        cover: Some(COVER_URL.to_string()),
        url: Some(ARCHIVE_URL.to_string()),
        authors: vec!["Ewan Breakey".to_string()],
        artists: vec!["Ewan Breakey".to_string()],
        description: Some(
            "a sci-fi horror webcomic about life blooming against all odds".to_string(),
        ),
        status: ItemStatus::Ongoing,
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(format!("{ARCHIVE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture<T: for<'de> Deserialize<'de>>(target: &str, fixture: &str) -> T {
    let text = client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    serde_json::from_str(&text).unwrap_or_else(|_| serde_json::from_str(fixture).unwrap())
}

#[derive(Debug, Deserialize)]
struct SasPage {
    page_number: u32,
    #[allow(dead_code)]
    chapter_number: u32,
    image_url: String,
    #[allow(dead_code)]
    thumbnail_url: String,
    name: String,
    #[allow(dead_code)]
    published_at: String,
}

export_manga_source!(SOURCE);

const PAGES_FIXTURE: &str = r#"[{"page_number":1,"chapter_number":1,"image_url":"https://imagedelivery.net/sample/page","thumbnail_url":"https://imagedelivery.net/sample/thumb","name":"Page 1","published_at":"2024-01-01T00:00:00Z"}]"#;
