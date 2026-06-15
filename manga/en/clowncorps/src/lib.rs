use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: ClownCorps = ClownCorps;
const BASE_URL: &str = "https://clowncorps.net";
const CREATOR: &str = "Joe Chouinard";

struct ClownCorps;

impl MangaSource for ClownCorps {
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
        Ok(fetch_all_chapters())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/comic/sample".to_string());
        let chapter_url = url::join_url(BASE_URL, &key);
        let body = fetch_document(&chapter_url, PAGE_FIXTURE);
        Ok(parse_pages(&body, show_author_notes(&request)))
    }

    fn manga_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(Some(format!("{BASE_URL}/comic")))
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
        key: "/comic".to_string(),
        title: "Clown Corps".to_string(),
        authors: vec![CREATOR.to_string()],
        artists: vec![CREATOR.to_string()],
        status: ItemStatus::Ongoing,
        cover: Some(format!("{BASE_URL}/wp-content/uploads/2022/11/clowns41.jpg")),
        description: Some(
            "Clown Corps is a comic about crime-fighting clowns.\nIt's pronounced \"core.\" Like marine corps."
                .to_string(),
        ),
        url: Some(format!("{BASE_URL}/comic")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
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

fn fetch_all_chapters() -> Vec<MangaChapter> {
    let first = fetch_document(&format!("{BASE_URL}/comic"), CHAPTERS_FIXTURE);
    let page_count = parse_archive_page_count(&first).unwrap_or(1);
    let mut chapters = BTreeMap::new();
    for chapter in extract_chapters(&first) {
        chapters.insert(chapter.key.clone(), chapter);
    }
    for page in 2..=page_count {
        let body = fetch_document(&format!("{BASE_URL}/comic/page/{page}/"), CHAPTERS_FIXTURE);
        let before = chapters.len();
        for chapter in extract_chapters(&body) {
            chapters.insert(chapter.key.clone(), chapter);
        }
        if chapters.len() == before {
            break;
        }
    }
    let mut out = chapters.into_values().collect::<Vec<_>>();
    out.sort_by(|left, right| right.date_uploaded.cmp(&left.date_uploaded));
    out
}

fn parse_archive_page_count(body: &str) -> Option<u32> {
    html::text_between(body, "paginav-pages", "</")
        .map(|value| html::strip_tags(&value))
        .and_then(|text| text.split_whitespace().last()?.parse().ok())
}

fn extract_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("class=\"comic")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "post-title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "post-title", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let date_text = format!(
                "{} {}",
                html::text_between(chunk, "post-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .unwrap_or_default(),
                html::text_between(chunk, "post-time", "</")
                    .map(|value| html::strip_tags(&value))
                    .unwrap_or_default()
            );
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: parse_english_datetime(&date_text),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, include_notes: bool) -> Vec<MangaPage> {
    let Some(image) = html::attr_after(body, "id=\"comic\"", "src")
        .or_else(|| html::attr_after(body, "id='comic'", "src"))
        .or_else(|| html::attr_after(body, "<img", "src"))
    else {
        return Vec::new();
    };
    let mut pages = vec![MangaPage {
        content: PageContent::Url {
            url: url::join_url(BASE_URL, &image),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some("Page 1".to_string()),
        ..MangaPage::default()
    }];
    if include_notes {
        let note = html::attr_after(body, "id=\"comic\"", "title")
            .or_else(|| html::attr_after(body, "id='comic'", "title"))
            .or_else(|| html::attr_after(body, "<img", "title"))
            .unwrap_or_default();
        if !note.trim().is_empty() && !is_plain_chapter_page_title(&note) {
            pages.push(manga::text_page(&format!(
                "Author's Notes from {CREATOR}\n\n{}",
                html::html_unescape(&note)
            )));
        }
    }
    pages
}

fn show_author_notes(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("showAuthorsNotes"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn is_plain_chapter_page_title(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    if !lower.starts_with("chapter ") || !lower.contains(" page ") {
        return false;
    }
    lower
        .chars()
        .all(|ch| ch.is_ascii_digit() || ch.is_ascii_whitespace() || "chapter page".contains(ch))
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

fn parse_english_datetime(value: &str) -> Option<i64> {
    let parts = value
        .replace(',', "")
        .split_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if parts.len() < 5 {
        return None;
    }
    let month = month_number(&parts[0])?;
    let day = parts[1].parse::<i32>().ok()?;
    let year = parts[2].parse::<i32>().ok()?;
    let mut time = parts[3].split(':');
    let mut hour = time.next()?.parse::<i32>().ok()?;
    let minute = time.next()?.parse::<i32>().ok()?;
    let ampm = parts[4].to_ascii_uppercase();
    if ampm == "PM" && hour != 12 {
        hour += 12;
    } else if ampm == "AM" && hour == 12 {
        hour = 0;
    }
    Some(unix_seconds(year, month, day, hour, minute, 0))
}

fn month_number(value: &str) -> Option<i32> {
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ]
    .iter()
    .position(|month| month.eq_ignore_ascii_case(value))
    .map(|index| index as i32 + 1)
}

fn unix_seconds(year: i32, month: i32, day: i32, hour: i32, minute: i32, second: i32) -> i64 {
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month_prime + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    (days as i64) * 86_400 + (hour as i64) * 3_600 + (minute as i64) * 60 + second as i64
}

export_manga_source!(SOURCE);

const CHAPTERS_FIXTURE: &str = r#"
<ul id="paginav"><li class="paginav-pages">Page 1 of 1</li></ul>
<div class="comic"><h2 class="post-title"><a href="https://clowncorps.net/comic/sample/">Chapter 1 Page 1</a></h2><span class="post-date">January 01, 2024</span><span class="post-time">01:30 PM</span></div>
"#;
const PAGE_FIXTURE: &str = r#"
<div id="comic"><img src="https://clowncorps.net/wp-content/uploads/sample.png" title="A sample author's note."></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exposes_single_series_and_notes() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Clown Corps");
        let pages = SOURCE
            .pages(json!({"chapter":"/comic/sample","preferences":{"showAuthorsNotes":true}}))
            .unwrap();
        assert_eq!(pages.len(), 2);
    }
}
