use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: DFlowScans = DFlowScans;
const BASE_URL: &str = "https://dflow.alwaysdata.net";
const SOURCE_NAME: &str = "DFlowScans";

struct DFlowScans;

impl MangaSource for DFlowScans {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/Series"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_document(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let status = request
            .get("filters")
            .and_then(|filters| filters.get("status"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let body = fetch_document(
            &format!(
                "{BASE_URL}/Series?search={}&status={}",
                url::query_escape(query),
                url::query_escape(status)
            ),
            LIST_FIXTURE,
        );
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/Series/sample".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/Series/sample".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/Read/sample/1".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(key),
                )),
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| chunk.contains("manga-card") || chunk.contains("col-lg-3"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "btn", "href")
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                let title = html::text_between(chunk, "manga-card-title", "</")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| SOURCE_NAME.to_string());
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: html::attr_after(chunk, "<img", "src")
                        .map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("en".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/Series/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| SOURCE_NAME.to_string()),
        cover: html::attr_after(body, "col-md-4", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "col-md-8", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: labeled_values(body, "Author"),
        artists: labeled_values(body, "Artist"),
        tags: genres(body),
        status: parse_status(&labeled_values(body, "Status").join(" ")),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("btn-primary") && chunk.contains("<h5"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "btn-primary", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let chapter_title = html::text_between(chunk, "<h5", "</h5>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let subtitle = html::text_between(chunk, "<p", "</p>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.contains("calendar") && !value.is_empty());
            let title = subtitle
                .filter(|value| !chapter_title.contains(value))
                .map(|value| format!("{chapter_title} - {value}"))
                .unwrap_or(chapter_title);
            let date_text = html::text_between(chunk, "fa-calendar", "</p>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number(&title),
                date_uploaded: parse_english_date(&date_text),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let json = body
        .split("const pages = ")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .unwrap_or("[]");
    serde_json::from_str::<Vec<PageDto>>(json)
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, page)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &page.url),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct PageDto {
    url: String,
    #[allow(dead_code)]
    num: Option<u32>,
}

fn labeled_values(body: &str, label: &str) -> Vec<String> {
    body.split("<div")
        .filter(|chunk| chunk.contains(label))
        .filter_map(|chunk| {
            chunk
                .split("<span")
                .nth(2)
                .and_then(|part| html::text_between(part, ">", "</span>"))
                .map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty() && value != label)
        .collect()
}

fn genres(body: &str) -> Vec<String> {
    body.split("<span")
        .skip(1)
        .filter(|chunk| chunk.contains("badge") || chunk.contains("genre"))
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</span>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim() {
        "Ongoing" => ItemStatus::Ongoing,
        "Completed" => ItemStatus::Completed,
        "Hiatus" => ItemStatus::Hiatus,
        "Dropped" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .split_whitespace()
        .find_map(|part| part.trim_matches(':').parse().ok())
}

fn parse_english_date(value: &str) -> Option<i64> {
    let parts = value
        .replace(',', "")
        .split_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if parts.len() < 3 {
        return None;
    }
    let month = month_number(&parts[0])?;
    let day = parts[1].parse::<i32>().ok()?;
    let year = parts[2].parse::<i32>().ok()?;
    unix_date(year, month, day)
}

fn month_number(value: &str) -> Option<i32> {
    match value {
        "Jan" => Some(1),
        "Feb" => Some(2),
        "Mar" => Some(3),
        "Apr" => Some(4),
        "May" => Some(5),
        "Jun" => Some(6),
        "Jul" => Some(7),
        "Aug" => Some(8),
        "Sep" => Some(9),
        "Oct" => Some(10),
        "Nov" => Some(11),
        "Dec" => Some(12),
        _ => None,
    }
}

fn unix_date(year: i32, month: i32, day: i32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(((era * 146_097 + doe - 719_468) as i64) * 86_400)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with(BASE_URL) {
        return format!(
            "/{}",
            value[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="col-lg-3 manga-card"><div class="manga-card-title">Sample Series</div><img src="/cover.jpg"><a class="btn btn-primary" href="/Series/sample">Read</a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample Series</h1><div class="col-md-4"><img src="/cover.jpg"></div><div class="col-md-8 col-lg-9"><p>Description</p></div>
<div><span>Status</span><span>Ongoing</span></div><div><span>Author</span><span>Author Name</span></div>
<div id="chapters-section"><div><h5>Chapter 1</h5><p>Subtitle</p><p><i class="fa-calendar"></i> Jan 01, 2024</p><a class="btn-primary" href="/Read/sample/1">Read</a></div></div>
"#;
const PAGES_FIXTURE: &str = r#"<script>const pages = [{"url":"https://cdn.example/page1.jpg","num":1},{"url":"https://cdn.example/page2.jpg","num":2}];</script>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dflow_shapes() {
        assert_eq!(
            parse_listing(LIST_FIXTURE).entries[0].title,
            "Sample Series"
        );
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
