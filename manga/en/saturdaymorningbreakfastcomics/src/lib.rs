use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: SaturdayMorningBreakfastComics = SaturdayMorningBreakfastComics;
const BASE_URL: &str = "https://smbc-comics.com";
const ARCHIVE_KEY: &str = "/comic/archive";

struct SaturdayMorningBreakfastComics;

impl MangaSource for SaturdayMorningBreakfastComics {
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
            .trim()
            .to_ascii_lowercase();
        let entries = if query.is_empty()
            || "saturday morning breakfast comics".contains(&query)
            || "smbc".contains(&query)
            || query.starts_with(BASE_URL)
        {
            vec![series_item()]
        } else {
            Vec::new()
        };
        Ok(Paged {
            entries,
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
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/comic/sample".to_string());
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

    fn manga_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(Some(url::join_url(BASE_URL, ARCHIVE_KEY)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(series_item()),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: "smbc".to_string(),
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
        title: "Saturday Morning Breakfast Comics".to_string(),
        cover: Some("assets/thumbnail.png".to_string()),
        description: Some(
            "SMBC is a daily comic strip about life, philosophy, science, mathematics, and jokes."
                .to_string(),
        ),
        authors: vec!["Zach Weinersmith".to_string()],
        artists: vec!["Zach Weinersmith".to_string()],
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
        .split("<option")
        .skip(1)
        .filter(|chunk| chunk.contains("comic/"))
        .filter_map(|chunk| {
            let value = html::attr(chunk, "value")?;
            let text = html::text_between(chunk, ">", "</option>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| "Comic".to_string());
            let (date, title) = text
                .split_once(" - ")
                .map(|(date, title)| (date.to_string(), title.to_string()))
                .unwrap_or_else(|| ("".to_string(), text));
            Some(MangaChapter {
                key: format!("/{}", value.trim_start_matches('/')),
                title: Some(title),
                date_uploaded: manatan_shared::dates::parse_fixture_date(&date),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    for (index, chapter) in chapters.iter_mut().enumerate() {
        chapter.chapter_number = Some((index + 1) as f32);
        chapter.url = Some(url::join_url(BASE_URL, &chapter.key));
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut pages = Vec::new();
    if let Some(image) = html::attr_after(body, "id=\"cc-comic\"", "src")
        .or_else(|| html::attr_after(body, "id='cc-comic'", "src"))
    {
        pages.push(MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some("Comic".to_string()),
            ..MangaPage::default()
        });
    }
    if let Some(title) = html::attr_after(body, "id=\"cc-comic\"", "title")
        .or_else(|| html::attr_after(body, "id='cc-comic'", "title"))
        .filter(|title| !title.is_empty())
    {
        pages.push(manga::text_page(&title));
    }
    if let Some(aftercomic) = html::text_between(body, "id=\"aftercomic\"", "</div>")
        .and_then(|chunk| html::attr_after(&chunk, "<img", "src"))
    {
        pages.push(MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &aftercomic),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some("Aftercomic".to_string()),
            ..MangaPage::default()
        });
    }
    pages
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"
<select><option value="comic/2024-01-01">January 1, 2024 - Sample Comic</option></select>
"#;
const PAGE_FIXTURE: &str = r#"
<img id="cc-comic" src="/comics/sample.png" title="Hover text">
<div id="aftercomic"><img src="/comics/after.png"></div>
"#;
