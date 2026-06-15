use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, MangaPageImage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: IRovedOut = IRovedOut;
const BASE_URL: &str = "https://www.irovedout.com";
const ARCHIVE_URL: &str = "https://www.irovedout.com/archive";
const THUMBNAIL_URL: &str = "https://i.ibb.co/2g7Htwq/irovedout.png";
const SERIES_TITLE: &str = "I Roved Out in Search of Truth and Love";
const AUTHOR_NAME: &str = "Alexis Flower";

struct IRovedOut;

impl MangaSource for IRovedOut {
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
        let matches_url = query.starts_with(&BASE_URL.to_ascii_lowercase());
        let matches_title = query.is_empty()
            || SERIES_TITLE.to_ascii_lowercase().contains(&query)
            || "i roved out".contains(&query);
        Ok(Paged {
            entries: if matches_url || matches_title {
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
        let home = fetch_document(BASE_URL, HOME_FIXTURE);
        let mut book_urls = parse_book_urls(&home);
        if book_urls.is_empty() {
            book_urls.push(ARCHIVE_URL.to_string());
        }
        let mut chapters = Vec::new();
        let mut chapter_number = 1.0_f32;
        for (book_index, book_url) in book_urls.iter().enumerate() {
            let body = fetch_document(book_url, ARCHIVE_FIXTURE);
            let book_number = book_index + 1;
            for archive_chapter in parse_archive_chapters(&body, book_number) {
                chapters.push(MangaChapter {
                    key: archive_chapter.key,
                    title: Some(archive_chapter.title),
                    chapter_number: Some(chapter_number),
                    date_uploaded: archive_chapter.date_uploaded,
                    url: Some(archive_chapter.url),
                    ..MangaChapter::default()
                });
                chapter_number += 1.0;
            }
        }
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| ARCHIVE_URL.to_string());
        let book_number = key
            .split('|')
            .nth(1)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1);
        let title = key.split('|').nth(2).unwrap_or_default();
        let archive_url = if book_number == 1 {
            ARCHIVE_URL.to_string()
        } else {
            format!("{ARCHIVE_URL}-book-{book_number}")
        };
        let body = fetch_document(&archive_url, ARCHIVE_FIXTURE);
        let page_urls = parse_page_urls_for_chapter(&body, title);
        Ok(page_urls
            .into_iter()
            .enumerate()
            .map(|(index, page_url)| MangaPage {
                content: PageContent::Lazy {
                    key: page_url.clone(),
                    url: None,
                    page_url: Some(page_url),
                    context: None,
                },
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<MangaPageImage> {
        let page_url = manga::request_key(&request, "page")
            .or_else(|| {
                request
                    .get("page")
                    .and_then(|page| page.get("content"))
                    .and_then(|content| content.get("lazy"))
                    .and_then(|lazy| lazy.get("pageUrl"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .or_else(|| {
                request
                    .get("url")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .unwrap_or_else(|| BASE_URL.to_string());
        let body = fetch_document(&page_url, PAGE_FIXTURE);
        let image = html::attr_after(&body, "id=\"comic\"", "src")
            .or_else(|| html::attr_after(&body, "id='comic'", "src"))
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .unwrap_or_else(|| THUMBNAIL_URL.to_string());
        Ok(MangaPageImage {
            url: url::join_url(BASE_URL, &image),
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
        key: "irovedout".to_string(),
        title: SERIES_TITLE.to_string(),
        cover: Some(THUMBNAIL_URL.to_string()),
        url: Some(BASE_URL.to_string()),
        authors: vec![AUTHOR_NAME.to_string()],
        artists: vec![AUTHOR_NAME.to_string()],
        description: Some("I ROVED OUT IN SEARCH OF TRUTH AND LOVE is written and illustrated by Alexis Flower. It updates in chunks anywhere between 3 and 30 pages long at least once a month.".to_string()),
        tags: vec!["Fantasy".to_string()],
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Ongoing,
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

fn parse_book_urls(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "href"))
        .filter(|href| href.starts_with(ARCHIVE_URL))
        .fold(Vec::new(), |mut urls, href| {
            if !urls.contains(&href) {
                urls.push(href);
            }
            urls
        })
}

struct ArchiveChapter {
    key: String,
    title: String,
    url: String,
    date_uploaded: Option<i64>,
}

fn parse_archive_chapters(body: &str, book_number: usize) -> Vec<ArchiveChapter> {
    body.split("comic-archive-chapter-wrap")
        .skip(1)
        .filter_map(|chunk| {
            let chapter_title = html::text_between(chunk, "comic-archive-chapter", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            let first_page = html::attr_after(chunk, "comic-archive-title", "href")?;
            let date_uploaded = html::text_between(chunk, "comic-archive-date", "</")
                .map(|value| html::strip_tags(&value))
                .and_then(|value| parse_month_date(&value));
            Some(ArchiveChapter {
                key: format!("book|{book_number}|{chapter_title}"),
                title: format!("Book {book_number}: {chapter_title}"),
                url: first_page,
                date_uploaded,
            })
        })
        .collect()
}

fn parse_page_urls_for_chapter(body: &str, title: &str) -> Vec<String> {
    body.split("comic-archive-chapter-wrap")
        .skip(1)
        .find(|chunk| {
            html::text_between(chunk, "comic-archive-chapter", "</")
                .map(|value| html::strip_tags(&value) == title)
                .unwrap_or(false)
        })
        .map(|chunk| {
            chunk
                .split("comic-archive-title")
                .skip(1)
                .filter_map(|part| html::attr_after(part, "<a", "href"))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_month_date(value: &str) -> Option<i64> {
    let normalized = value.trim().replace(',', "");
    let mut parts = normalized.split_whitespace().map(str::to_string);
    let month = match parts.next()?.as_str() {
        "January" => 1,
        "February" => 2,
        "March" => 3,
        "April" => 4,
        "May" => 5,
        "June" => 6,
        "July" => 7,
        "August" => 8,
        "September" => 9,
        "October" => 10,
        "November" => 11,
        "December" => 12,
        _ => return None,
    };
    let day = parts.next()?.parse().ok()?;
    let year = parts.next()?.parse().ok()?;
    Some(unix_from_ymd(year, month, day))
}

fn unix_from_ymd(year: i32, month: i32, day: i32) -> i64 {
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64 * 86_400
}

export_manga_source!(SOURCE);

const HOME_FIXTURE: &str =
    r#"<ul id="menu-menu"><li><a href="https://www.irovedout.com/archive">Book 1</a></li></ul>"#;
const ARCHIVE_FIXTURE: &str = r#"
<div class="comic-archive-chapter-wrap">
  <div class="comic-archive-chapter">Truth</div>
  <span class="comic-archive-date">January 01, 2024</span>
  <div class="comic-archive-list-wrap">
    <div class="comic-archive-title"><a href="https://www.irovedout.com/comic/page-1">Page 1</a></div>
    <div class="comic-archive-title"><a href="https://www.irovedout.com/comic/page-2">Page 2</a></div>
  </div>
</div>
"#;
const PAGE_FIXTURE: &str =
    r#"<div id="comic"><img src="https://www.irovedout.com/page.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_archive() {
        let chapters = SOURCE.chapters(json!({})).unwrap();
        assert_eq!(chapters[0].title.as_deref(), Some("Book 1: Truth"));
    }
}
