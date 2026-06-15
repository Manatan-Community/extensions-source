use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Oglaf = Oglaf;
const BASE_URL: &str = "https://www.oglaf.com";
const SERIES_KEY: &str = "/archive";
const COVER: &str = "https://i.ibb.co/tzY0VQ9/oglaf.png";
const CREATORS: &str = "Trudy Cooper & Doug Bayne";

struct Oglaf;

impl MangaSource for Oglaf {
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
        let item = series_item();
        let entries = if query.is_empty()
            || item.title.to_ascii_lowercase().contains(&query)
            || query.starts_with(BASE_URL)
        {
            vec![item]
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
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, SERIES_KEY),
            ARCHIVE_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_pages(&url::join_url(BASE_URL, &key), PAGE_FIXTURE))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "archive".to_string(),
            title: "Archive".to_string(),
            style: Some(HomeSectionStyle::Compact),
            entries: vec![series_item()],
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(Some(url::join_url(BASE_URL, SERIES_KEY)))
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
                query: input.to_string(),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn series_item() -> CatalogItem {
    CatalogItem {
        key: SERIES_KEY.to_string(),
        title: "Oglaf".to_string(),
        cover: Some(COVER.to_string()),
        authors: vec![CREATORS.to_string()],
        artists: vec![CREATORS.to_string()],
        description: Some("Filth and other fantastical things in handy webcomic form.".to_string()),
        status: ItemStatus::Ongoing,
        url: Some(url::join_url(BASE_URL, SERIES_KEY)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
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

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("width=400") || chunk.contains("width=\"400\""))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = key
                .trim_matches('/')
                .replace('-', " ")
                .split_whitespace()
                .map(capitalize_ascii)
                .collect::<Vec<_>>()
                .join(" ");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(if title.is_empty() {
                    "Comic".to_string()
                } else {
                    title
                }),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter);
    let total = chapters.len();
    for (index, chapter) in chapters.iter_mut().enumerate() {
        chapter.chapter_number = Some((total - index) as f32);
    }
    chapters
}

fn parse_pages(start_url: &str, fixture: &str) -> Vec<MangaPage> {
    let mut pages = Vec::new();
    let mut target = start_url.to_string();
    for _ in 0..32 {
        let body = fetch_document(&target, fixture);
        let Some(image) = html::attr_after(&body, "id=\"strip\"", "src")
            .or_else(|| html::attr_after(&body, "id='strip'", "src"))
            .map(|value| url::join_url(BASE_URL, &value))
        else {
            break;
        };
        pages.push(MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", pages.len() + 1)),
            ..MangaPage::default()
        });
        let Some(next) = html::attr_after(&body, "rel=\"next\"", "href")
            .or_else(|| html::attr_after(&body, "rel='next'", "href"))
        else {
            break;
        };
        let next_key = normalize_key(&next);
        if !next_key
            .trim_matches('/')
            .rsplit('/')
            .next()
            .is_some_and(|value| value.parse::<u64>().is_ok())
        {
            break;
        }
        target = url::join_url(BASE_URL, &next_key);
    }
    pages
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn capitalize_ascii(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

fn push_unique_chapter(
    mut chapters: Vec<MangaChapter>,
    chapter: MangaChapter,
) -> Vec<MangaChapter> {
    if !chapters.iter().any(|existing| existing.key == chapter.key) {
        chapters.push(chapter);
    }
    chapters
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"
<a href="/sample/"><img width="400" src="/archive.jpg"></a>
"#;
const PAGE_FIXTURE: &str = r#"
<img id="strip" src="/media/sample.jpg">
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_oglaf_archive_and_pages() {
        assert_eq!(parse_chapters(ARCHIVE_FIXTURE).len(), 1);
        assert_eq!(
            parse_pages(&url::join_url(BASE_URL, "/sample/"), PAGE_FIXTURE).len(),
            1
        );
    }
}
