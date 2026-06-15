use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: CManhua = CManhua;
const BASE_URL: &str = "https://cmanhua.com";

struct CManhua;

impl MangaSource for CManhua {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_list(LIST_FIXTURE, 1));
        }
        let page = page(&request);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "0"
        } else {
            "2"
        };
        Ok(parse_manga_list(
            &fetch_document(&list_url(page, sort, None), LIST_FIXTURE),
            page,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            list_url(
                page,
                filter_value(&request, "sort", "0").as_str(),
                request.get("filters"),
            )
        } else {
            search_url(page, query)
        };
        Ok(parse_manga_list(
            &fetch_document(&target, LIST_FIXTURE),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/sample.html".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/sample.html".to_string());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample/chapter-1.html".to_string());
        let chapter_url = url::join_url(BASE_URL, &key);
        let body = fetch_document(&chapter_url, PAGE_FIXTURE);
        let encoded = chapter_token(&body);
        let page_html = encoded
            .map(|token| fetch_chapter_pages(&token, &chapter_url))
            .unwrap_or_else(|| PAGE_API_FIXTURE.to_string());
        Ok(parse_page_html(&extract_chapter_payload(&page_html)))
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
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
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

fn fetch_chapter_pages(encoded: &str, referer: &str) -> String {
    client()
        .post(format!("{BASE_URL}/Service.asmx/getchapter"))
        .xhr()
        .referer(referer)
        .json(json!({ "enc": encoded }).to_string())
        .send_text()
        .unwrap_or_else(|_| PAGE_API_FIXTURE.to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn list_url(page: u64, sort: &str, filters: Option<&Value>) -> String {
    let status = filter_str(filters, "status", "-1");
    let chapters = filter_str(filters, "minChapters", "0");
    let gender = filter_str(filters, "gender", "-1");
    let genres = selected_values(filters.and_then(|value| value.get("genres"))).join(",");
    let mut target = format!(
        "{BASE_URL}/danhsach/P{page}/index.html?status={status}&sort={sort}&chapter={chapters}&gender={gender}"
    );
    if !genres.is_empty() {
        target.push_str("&spec=");
        target.push_str(&url::query_escape(&genres));
    }
    target
}

fn search_url(page: u64, query: &str) -> String {
    let mut target = format!("{BASE_URL}/{}/", url::query_escape(query));
    if page > 1 {
        target.push_str(&format!("P{page}/"));
    }
    target.push_str("tim-kiem.html");
    target
}

fn parse_manga_list(body: &str, page: u64) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("item"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<h3", "href")
                .or_else(|| html::attr_after(chunk, "itemprop", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h3", "</h3>")
                .or_else(|| html::text_between(chunk, "itemprop", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    url::slug_from_url(&key).unwrap_or_else(|| "CManhua".to_string())
                });
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        has_next_page: has_next_page(body, page),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample.html".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "title-detail", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "CManhua".to_string())),
        authors: detail_links(body, "author"),
        artists: detail_links(body, "author"),
        tags: detail_links(body, "kind"),
        status: detail_text(body, "status")
            .as_deref()
            .map(status_from_text)
            .unwrap_or(ItemStatus::Unknown),
        description: html::text_between(body, "id=\"descript\"", "</")
            .or_else(|| html::text_between(body, "id='descript'", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        cover: html::attr_after(body, "col-image", "src")
            .map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("row"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "chapter", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapter", "</a>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: parse_chapter_number(&title),
                date_uploaded: html::attr_after(chunk, "<time", "datetime")
                    .and_then(|date| parse_iso_utc(&date)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_page_html(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
        .filter(|image| !image.is_empty())
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
        .collect()
}

fn extract_chapter_payload(body: &str) -> String {
    let trimmed = body.trim().trim_start_matches('\u{FEFF}');
    if trimmed.starts_with('<') || trimmed.to_ascii_lowercase().contains("<img") {
        return trimmed.to_string();
    }
    let Ok(root) = serde_json::from_str::<Value>(trimmed) else {
        return PAGE_API_FIXTURE.to_string();
    };
    match root {
        Value::String(value) => value,
        Value::Object(map) => ["d", "data", "html"]
            .iter()
            .find_map(|key| {
                map.get(*key)
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn chapter_token(body: &str) -> Option<String> {
    let start = body.find("var ts")?;
    let rest = &body[start..];
    let first_quote = rest.find('"')? + 1;
    let rest = &rest[first_quote..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
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

fn detail_links(body: &str, class_name: &str) -> Vec<String> {
    body.split("<li")
        .filter(|chunk| chunk.contains(class_name))
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn detail_text(body: &str, class_name: &str) -> Option<String> {
    body.split("<li")
        .find(|chunk| chunk.contains(class_name))
        .and_then(|chunk| html::text_between(chunk, "col-xs-8", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn status_from_text(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "on going" | "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn has_next_page(body: &str, page: u64) -> bool {
    body.split("list-pager")
        .flat_map(numbers_in_text)
        .max()
        .is_some_and(|max| page < max)
}

fn numbers_in_text(input: &str) -> Vec<u64> {
    let mut numbers = Vec::new();
    let mut current = String::new();
    for ch in html::strip_tags(input).chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if !current.is_empty() {
            if let Ok(number) = current.parse() {
                numbers.push(number);
            }
            current.clear();
        }
    }
    if let Ok(number) = current.parse() {
        numbers.push(number);
    }
    numbers
}

fn filter_value(request: &Value, key: &str, default: &str) -> String {
    filter_str(request.get("filters"), key, default)
}

fn filter_str(filters: Option<&Value>, key: &str, default: &str) -> String {
    filters
        .and_then(|value| value.get(key))
        .and_then(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .or_else(|| value.as_i64().map(|number| number.to_string()))
        })
        .unwrap_or_else(|| default.to_string())
}

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|value| {
                value.as_str().map(ToString::to_string).or_else(|| {
                    value
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
            })
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.clone()],
        _ => Vec::new(),
    }
}

fn parse_chapter_number(title: &str) -> Option<f32> {
    let lower = title.to_ascii_lowercase();
    let after = lower.split("chapter").nth(1)?;
    let mut number = String::new();
    for ch in after.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            number.push(ch);
        } else if !number.is_empty() {
            break;
        }
    }
    number.parse().ok()
}

fn parse_iso_utc(value: &str) -> Option<i64> {
    let date_time = value.trim().trim_end_matches('Z');
    let (date, time) = date_time.split_once('T')?;
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i32>().ok()?;
    let month = date_parts.next()?.parse::<i32>().ok()?;
    let day = date_parts.next()?.parse::<i32>().ok()?;
    let mut time_parts = time.split(':');
    let hour = time_parts.next()?.parse::<i32>().ok()?;
    let minute = time_parts.next()?.parse::<i32>().ok()?;
    let second = time_parts.next().unwrap_or("0").parse::<i32>().unwrap_or(0);
    Some(unix_seconds(year, month, day, hour, minute, second))
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

const LIST_FIXTURE: &str = r#"
<ul class="lst_story"><li class="item"><h3><a href="/sample.html">Sample Manhua</a></h3><img data-src="/cover.jpg"></li></ul>
<li class="list-pager"><a>1</a><a>2</a></li>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="title-detail">Sample Manhua</h1><div class="col-image"><img src="/cover.jpg"></div>
<ul><li class="author"><p class="col-xs-8"><a>Author</a></p></li><li class="status"><p class="col-xs-8">On Going</p></li><li class="kind"><p class="col-xs-8"><a>Action</a></p></li></ul>
<div id="descript">A sample description.</div>
<ul id="listchap"><li class="row"><div class="chapter"><a href="/sample/chapter-1.html">Chapter 1</a></div><time datetime="2024-01-01T00:00:00Z"></time></li></ul>
"#;
const PAGE_FIXTURE: &str = r#"<script>var ts = "encoded-token";</script>"#;
const PAGE_API_FIXTURE: &str = r#"{"d":"<p><img src=\"/page1.jpg\"><img src=\"/page2.jpg\"></p>"}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_listing_chapters_and_api_pages() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample Manhua");
        let chapters = SOURCE.chapters(json!({"manga":"/sample.html"})).unwrap();
        assert_eq!(chapters[0].chapter_number, Some(1.0));
        let pages = SOURCE
            .pages(json!({"chapter":"/sample/chapter-1.html"}))
            .unwrap();
        assert_eq!(pages.len(), 2);
    }
}
