use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, MangaPageImage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Keenspot = Keenspot;
const BASE_URL: &str = "https://twokinds.keenspot.com";

struct Keenspot;

#[derive(Clone, Copy)]
struct SeriesMode {
    key: &'static str,
    title: &'static str,
}

const MODES: [SeriesMode; 2] = [
    SeriesMode {
        key: "1",
        title: "TwoKinds (1 page per chapter)",
    },
    SeriesMode {
        key: "20",
        title: "TwoKinds (20 pages per chapter)",
    },
];

impl MangaSource for Keenspot {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: MODES.into_iter().map(series_item).collect(),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let entries = MODES
            .into_iter()
            .filter(|mode| query.is_empty() || mode.title.to_ascii_lowercase().contains(&query))
            .map(series_item)
            .collect();
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(mode_from_key(&key)
            .map(series_item)
            .unwrap_or_else(|| series_item(MODES[0])))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let mode = mode_from_key(&key).unwrap_or(MODES[0]);
        let body = fetch_document(&format!("{BASE_URL}/archive/"), ARCHIVE_FIXTURE);
        Ok(parse_chapters(&body, mode))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1-0001".into());
        if key.starts_with("1-") {
            let page = key.trim_start_matches("1-");
            return Ok(vec![lazy_page(0, page)]);
        }
        let first_page = key.trim_start_matches("20-");
        let body = fetch_document(&format!("{BASE_URL}/archive/"), ARCHIVE_FIXTURE);
        let pages = archive_pages(&body);
        let start = pages
            .iter()
            .position(|page| page.url == first_page)
            .unwrap_or(0);
        Ok(pages
            .into_iter()
            .skip(start)
            .take(20)
            .enumerate()
            .map(|(index, page)| lazy_page(index, &page.url))
            .collect())
    }

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<MangaPageImage> {
        let key = manga::request_key(&request, "page")
            .or_else(|| {
                request
                    .get("page")
                    .and_then(|page| page.get("content"))
                    .and_then(|content| content.get("lazy"))
                    .and_then(|lazy| lazy.get("pageUrl").or_else(|| lazy.get("key")))
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .unwrap_or_else(|| "0001".to_string());
        let target = format!("{BASE_URL}/comic/{}/", key.trim_matches('/'));
        let body = fetch_document(&target, PAGE_FIXTURE);
        let image = html::attr_after(&body, "#content", "src")
            .or_else(|| html::attr_after(&body, "<article", "src"))
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image))
            .unwrap_or_else(|| format!("{BASE_URL}/comic/{key}/sample.jpg"));
        Ok(MangaPageImage {
            url: image,
            headers: manga::image_headers(BASE_URL),
            ..MangaPageImage::default()
        })
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(series_item(MODES[0])),
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

fn series_item(mode: SeriesMode) -> CatalogItem {
    CatalogItem {
        key: mode.key.to_string(),
        title: mode.title.to_string(),
        cover: Some(format!(
            "{BASE_URL}/wp-content/uploads/2021/03/cropped-TKIcon.png"
        )),
        url: Some(BASE_URL.to_string()),
        authors: vec!["Tom Fischbach".to_string()],
        artists: vec!["Tom Fischbach".to_string()],
        description: Some("Fantasy webcomic from TwoKinds, grouped by reader mode.".to_string()),
        status: ItemStatus::Unknown,
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
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

#[derive(Clone)]
struct ArchivePage {
    url: String,
    name: String,
}

fn parse_chapters(body: &str, mode: SeriesMode) -> Vec<MangaChapter> {
    let pages = archive_pages(body);
    if mode.key == "1" {
        return pages
            .into_iter()
            .map(|page| MangaChapter {
                key: format!("1-{}", page.url),
                title: Some(format!("Page {}", page.name)),
                url: Some(format!("{BASE_URL}/comic/{}/", page.url)),
                ..MangaChapter::default()
            })
            .rev()
            .collect();
    }
    pages
        .chunks(20)
        .filter_map(|chunk| {
            let first = chunk.first()?;
            let last = chunk.last()?;
            Some(MangaChapter {
                key: format!("20-{}", first.url),
                title: Some(format!("Pages {}-{}", first.name, last.name)),
                url: Some(format!("{BASE_URL}/comic/{}/", first.url)),
                ..MangaChapter::default()
            })
        })
        .rev()
        .collect()
}

fn archive_pages(body: &str) -> Vec<ArchivePage> {
    body.split("chapter-links")
        .skip(1)
        .flat_map(|season| season.split("<a").skip(1))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let url = href
                .split("/comic/")
                .nth(1)
                .or_else(|| href.trim_matches('/').rsplit('/').next())
                .unwrap_or("0001")
                .trim_matches('/')
                .to_string();
            let name = html::text_between(chunk, "<span", "</span>")
                .or_else(|| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url.clone());
            Some(ArchivePage { url, name })
        })
        .collect()
}

fn lazy_page(index: usize, page_key: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Lazy {
            key: page_key.to_string(),
            url: None,
            page_url: Some(format!("{BASE_URL}/comic/{page_key}/")),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn mode_from_key(key: &str) -> Option<SeriesMode> {
    MODES.into_iter().find(|mode| mode.key == key)
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"
<div class="chapter-links"><a href="/comic/0001/"><span>1</span></a><a href="/comic/0002/"><span>2</span></a><a href="/comic/0003/"><span>3</span></a></div>
"#;
const PAGE_FIXTURE: &str =
    r#"<div id="content"><article><img src="/comic/0001/page.jpg"></article></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modes_chapters_and_pages() {
        assert_eq!(parse_chapters(ARCHIVE_FIXTURE, MODES[0]).len(), 3);
        assert_eq!(parse_chapters(ARCHIVE_FIXTURE, MODES[1]).len(), 1);
        assert_eq!(archive_pages(ARCHIVE_FIXTURE)[0].url, "0001");
    }
}
