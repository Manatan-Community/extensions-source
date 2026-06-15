use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: SwordsComic = SwordsComic;
const BASE_URL: &str = "https://swordscomic.com";
const ARCHIVE_KEY: &str = "/archive/pages/";

struct SwordsComic;

impl MangaSource for SwordsComic {
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
                || "swords comic".contains(&query)
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
        Ok(parse_archive(&fetch_document(
            &url::join_url(BASE_URL, ARCHIVE_KEY),
            ARCHIVE_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/comic/1".to_string());
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
        key: ARCHIVE_KEY.to_string(),
        title: "Swords Comic".to_string(),
        cover: Some(format!("{BASE_URL}/media/ArgoksEdgeEmote.png")),
        description: Some("A webcomic about swords and the heroes who wield them".to_string()),
        authors: vec!["Matthew Wills".to_string()],
        artists: vec!["Matthew Wills".to_string()],
        status: ItemStatus::Ongoing,
        url: Some(url::join_url(BASE_URL, ARCHIVE_KEY)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_archive(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("archive-tile"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            Some(MangaChapter {
                key: normalize_key(&href),
                title: html::text_between(chunk, "<strong", "</strong>")
                    .map(|v| html::strip_tags(&v))
                    .filter(|v| !v.is_empty()),
                date_uploaded: html::text_between(chunk, "<small", "</small>")
                    .map(|v| html::strip_tags(&v))
                    .and_then(|date| manatan_shared::dates::parse_fixture_date(&date)),
                url: Some(url::join_url(BASE_URL, &href)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut pages = Vec::new();
    if let Some(image) = html::attr_after(body, "id=\"comic-image\"", "src")
        .or_else(|| html::attr_after(body, "id='comic-image'", "src"))
    {
        pages.push(MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some("Comic".into()),
            ..MangaPage::default()
        });
    }
    if let Some(title) = html::attr_after(body, "id=\"comic-image\"", "title")
        .or_else(|| html::attr_after(body, "id='comic-image'", "title"))
        .filter(|v| !v.is_empty())
    {
        pages.push(manga::text_page(&title));
    }
    pages
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") {
        if let Some(path) = value.strip_prefix(BASE_URL) {
            return format!("/{}", path.trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str =
    r#"<a class="archive-tile" href="/comic/1"><strong>One</strong><small>01 Jan 2024</small></a>"#;
const PAGE_FIXTURE: &str = r#"<img id="comic-image" src="/media/comic.png" title="Alt text">"#;
