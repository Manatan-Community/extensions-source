use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: QuestionableContent = QuestionableContent;
const BASE_URL: &str = "https://www.questionablecontent.net";
const COVER: &str = "https://i.ibb.co/ZVL9ncS/qc-teh.png";
const AUTHOR: &str = "Jeph Jacques";

struct QuestionableContent;

impl MangaSource for QuestionableContent {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: vec![catalog_item()],
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
        let include = query.is_empty()
            || "questionable content".contains(&query)
            || query.starts_with(BASE_URL)
            || AUTHOR.to_ascii_lowercase().contains(&query);
        Ok(Paged {
            entries: include.then(catalog_item).into_iter().collect(),
            has_next_page: false,
        })
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(catalog_item())
    }

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        Ok(parse_chapters(&fetch_document(
            &format!("{BASE_URL}/archive.php"),
            ARCHIVE_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/view.php?comic=1".to_string());
        let show_notes = request
            .get("preferences")
            .and_then(|prefs| prefs.get("show_authors_notes"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(parse_pages(
            &fetch_document(&url::join_url(BASE_URL, &key), PAGE_FIXTURE),
            show_notes,
        ))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "series".to_string(),
            title: "Series".to_string(),
            style: Some(HomeSectionStyle::Compact),
            entries: vec![catalog_item()],
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(Some(format!("{BASE_URL}/archive.php")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let chapter = input.contains("view.php?comic=").then(|| {
                json!(MangaChapter {
                    key: normalize_key(input),
                    title: Some(format!("Comic {}", comic_number(input).unwrap_or_default())),
                    chapter_number: comic_number(input).map(|number| number as f32),
                    url: Some(input.to_string()),
                    ..MangaChapter::default()
                })
            });
            return Ok(Some(UrlResolveResult {
                item: Some(catalog_item()),
                chapter,
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
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn catalog_item() -> CatalogItem {
    CatalogItem {
        key: "/archive.php".to_string(),
        title: "Questionable Content".to_string(),
        cover: Some(COVER.to_string()),
        authors: vec![AUTHOR.to_string()],
        artists: vec![AUTHOR.to_string()],
        description: Some("An internet comic strip about romance and robots.".to_string()),
        status: ItemStatus::Ongoing,
        url: Some(format!("{BASE_URL}/archive.php")),
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
        .filter(|chunk| chunk.contains("view.php?comic="))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let number = comic_number(&href)?;
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("Comic {number}"));
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                chapter_number: Some(number as f32),
                url: Some(url::join_url(BASE_URL, &href)),
                language: Some("en".to_string()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|left, right| {
        right
            .chapter_number
            .partial_cmp(&left.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters.dedup_by(|left, right| left.key == right.key);
    chapters
}

fn parse_pages(body: &str, show_notes: bool) -> Vec<MangaPage> {
    let mut pages = body
        .split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("id=\"strip\"") || chunk.contains("id='strip'"))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect::<Vec<_>>();
    if show_notes {
        if let Some(notes) = html::text_between(body, "id=\"newspost\"", "</div>")
            .or_else(|| html::text_between(body, "id='newspost'", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
        {
            pages.push(manga::text_page(&notes));
        }
    }
    pages
}

fn comic_number(value: &str) -> Option<u64> {
    value
        .split("comic=")
        .nth(1)?
        .split(['&', '#', '"', '\''])
        .next()?
        .parse()
        .ok()
}

fn normalize_key(value: &str) -> String {
    if let Some(number) = comic_number(value) {
        format!("/view.php?comic={number}")
    } else {
        "/archive.php".to_string()
    }
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"
<div id="container">
  <a href="view.php?comic=2">Second Comic</a>
  <a href="view.php?comic=1">First Comic</a>
</div>
"#;
const PAGE_FIXTURE: &str = r#"
<img id="strip" src="/comics/sample.png">
<div id="newspost"><p>Author notes.</p></div>
"#;
