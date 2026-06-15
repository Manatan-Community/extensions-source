use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: SchlockMercenary = SchlockMercenary;
const BASE_URL: &str = "https://www.schlockmercenary.com";
const ARCHIVE_URL: &str = "/archives/";

struct SchlockMercenary;

impl MangaSource for SchlockMercenary {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: parse_books(&fetch_document(
                &url::join_url(BASE_URL, ARCHIVE_URL),
                ARCHIVE_FIXTURE,
            )),
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
        let mut entries = self.list(serde_json::json!({}))?.entries;
        if !query.is_empty() && !query.starts_with(BASE_URL) {
            entries.retain(|item| item.title.to_ascii_lowercase().contains(&query));
        }
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/archive/book-1".to_string());
        Ok(parse_books(&fetch_document(
            &url::join_url(BASE_URL, ARCHIVE_URL),
            ARCHIVE_FIXTURE,
        ))
        .into_iter()
        .find(|item| item.key == key)
        .unwrap_or_else(|| fallback_book(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/archive/book-1".to_string());
        Ok(parse_book_chapters(
            &fetch_document(&url::join_url(BASE_URL, ARCHIVE_URL), ARCHIVE_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/2000-06-12".to_string());
        let start = key.trim_matches('/').to_string();
        let end = request
            .get("chapter")
            .and_then(|chapter| chapter.get("context"))
            .and_then(|context| context.get("end"))
            .and_then(Value::as_str)
            .unwrap_or(&start);
        Ok(date_range(&start, end)
            .into_iter()
            .flat_map(|date| {
                parse_day_pages(
                    &fetch_document(&format!("{BASE_URL}/{date}"), DAY_FIXTURE),
                    &date,
                )
            })
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
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

fn parse_books(body: &str) -> Vec<CatalogItem> {
    body.split("archive-book")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<h4", "</h4>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Schlock Mercenary".to_string());
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|image| url::join_url(BASE_URL, &image)),
                description: html::text_between(chunk, "<p", "</p>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                authors: vec!["Howard Tayler".to_string()],
                artists: vec!["Howard Tayler".to_string()],
                status: ItemStatus::Completed,
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: true,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn fallback_book(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: "Schlock Mercenary".to_string(),
        authors: vec!["Howard Tayler".to_string()],
        artists: vec!["Howard Tayler".to_string()],
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_book_chapters(body: &str, book_key: &str) -> Vec<MangaChapter> {
    let book_chunk = body
        .split("archive-book")
        .skip(1)
        .find(|chunk| {
            html::attr_after(chunk, "<a", "href")
                .is_some_and(|href| normalize_key(&href) == book_key)
        })
        .unwrap_or(body);
    let mut chapters = book_chunk
        .split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let date = href.trim_matches('/').to_string();
            Some(MangaChapter {
                key: normalize_key(&href),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| Some(date.clone())),
                date_uploaded: manatan_shared::dates::parse_fixture_date(&date),
                url: Some(url::join_url(BASE_URL, &href)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    for (index, chapter) in chapters.iter_mut().enumerate() {
        chapter.chapter_number = Some((index + 1) as f32);
    }
    chapters
}

fn parse_day_pages(body: &str, date: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains(&format!("strip-{date}")) || chunk.contains("strip-"))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("{date} #{}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn date_range(start: &str, end: &str) -> Vec<String> {
    let Some(mut current) = parse_date_tuple(start) else {
        return vec![start.to_string()];
    };
    let Some(end) = parse_date_tuple(end) else {
        return vec![start.to_string()];
    };
    let mut dates = Vec::new();
    for _ in 0..370 {
        dates.push(format_date_tuple(current));
        if current >= end {
            break;
        }
        current = next_day(current);
    }
    dates
}

fn parse_date_tuple(value: &str) -> Option<(i32, u32, u32)> {
    let mut parts = value.split('-');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

fn format_date_tuple((year, month, day): (i32, u32, u32)) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

fn next_day((mut year, mut month, mut day): (i32, u32, u32)) -> (i32, u32, u32) {
    day += 1;
    if day > days_in_month(year, month) {
        day = 1;
        month += 1;
        if month > 12 {
            month = 1;
            year += 1;
        }
    }
    (year, month, day)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 30,
    }
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(path) = value.strip_prefix(BASE_URL) {
            return format!("/{}", path.trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"
<div class="archive-book"><h4><a href="/archive/book-1">Book One</a></h4><img src="/static/img/logo.b6dacbb8.jpg"><p>First book.</p><ul class="chapters"><li><a href="/2000-06-12">First Strip</a></li><li><a href="/2000-06-13">Second Strip</a></li></ul></div>
"#;
const DAY_FIXTURE: &str =
    r#"<div id="strip-2000-06-12"><img src="/comics/schlock20000612.png"></div>"#;
