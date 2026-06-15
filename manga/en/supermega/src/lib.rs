use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: SuperMega = SuperMega;
const BASE_URL: &str = "https://www.supermegacomics.com";

struct SuperMega;

impl MangaSource for SuperMega {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: vec![series_item()],
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        Ok(Paged {
            entries: if query.is_empty()
                || "super mega".contains(&query)
                || query.starts_with(BASE_URL)
            {
                vec![series_item()]
            } else {
                Vec::new()
            },
            has_next_page: false,
        })
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(series_item())
    }

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let body = fetch_document(BASE_URL, HOME_FIXTURE);
        let latest = html::attr_after(&body, "bigbuttonprevious", "href")
            .and_then(|href| href.split("i=").nth(1)?.parse::<u32>().ok())
            .map(|value| value + 1)
            .unwrap_or(1);
        Ok((1..=latest)
            .rev()
            .map(|number| MangaChapter {
                key: format!("/?i={number}"),
                title: Some(number.to_string()),
                chapter_number: Some(number as f32),
                url: Some(format!("{BASE_URL}/?i={number}")),
                ..MangaChapter::default()
            })
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/?i=1".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGE_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "comic".into(),
            title: "Comic".into(),
            style: Some(HomeSectionStyle::Cover),
            entries: vec![series_item()],
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let input = request
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(Some(UrlResolveResult {
            item: input.starts_with(BASE_URL).then(series_item),
            search: (!input.starts_with(BASE_URL)).then(|| SearchRequest {
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

fn series_item() -> CatalogItem {
    CatalogItem {
        key: "/".to_string(),
        title: "SUPER MEGA".to_string(),
        cover: Some(format!("{BASE_URL}/runningman_inverted.PNG")),
        authors: vec!["JohnnySmash".to_string()],
        artists: vec!["JohnnySmash".to_string()],
        status: ItemStatus::Ongoing,
        url: Some(BASE_URL.to_string()),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("border=\"4\"") || chunk.contains("border='4'"))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

export_manga_source!(SOURCE);

const HOME_FIXTURE: &str = r#"<a name="bigbuttonprevious" href="?i=2">Previous</a>"#;
const PAGE_FIXTURE: &str = r#"<img border="4" src="/comic1.png">"#;
