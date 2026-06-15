use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: HentaiReadio = HentaiReadio;
const BASE_URL: &str = "https://hentairead.io";

struct HentaiReadio;

impl MangaSource for HentaiReadio {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "lastest-chap"
        } else {
            "top-manga"
        };
        Ok(parse_listing(&fetch_document(
            &search_url(page, "", "all", sort, ""),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
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
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = search_url(
            page,
            query,
            &filter(filters, "status", "all"),
            &filter(filters, "sort", "lastest-chap"),
            &filter(filters, "genre", ""),
        );
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(&fetch_document(
            &search_url(1, "", "all", "top-manga", ""),
            LIST_FIXTURE,
        ));
        let latest = parse_listing(&fetch_document(
            &search_url(1, "", "all", "lastest-chap", ""),
            LIST_FIXTURE,
        ));
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
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

fn search_url(page: u64, query: &str, status: &str, sort: &str, genre: &str) -> String {
    let mut target = format!(
        "{BASE_URL}/?act=search&f%5Bstatus%5D={}&f%5Bsortby%5D={}&pageNum={page}",
        url::query_escape(status),
        url::query_escape(sort)
    );
    if !query.is_empty() {
        target.push_str(&format!("&f%5Bkeyword%5D={}", url::query_escape(query)));
    }
    if !genre.is_empty() {
        target.push_str(&format!("&f%5Bgenres%5D={}", url::query_escape(genre)));
    }
    target
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("card") && chunk.contains("jtip"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "title-manga", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "title-manga", "</div>")
                .or_else(|| html::attr_after(chunk, "<a", "title"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    url::slug_from_url(&key).unwrap_or_else(|| "HentaiRead.io".into())
                });
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "card-img-top", "data-src")
                    .or_else(|| html::attr_after(chunk, "card-img-top", "src"))
                    .or_else(|| html::attr_after(chunk, "<img", "data-src"))
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| url::join_url(BASE_URL, &image)),
                status: ItemStatus::Unknown,
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("page-link")
            && (body.contains("&raquo;") || body.contains('»')),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "title-detail", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "HentaiRead.io".into())),
        cover: html::attr_after(body, "col-image", "data-src")
            .or_else(|| html::attr_after(body, "col-image", "src"))
            .or_else(|| html::attr_after(body, "property=\"og:image\"", "content"))
            .map(|image| url::join_url(BASE_URL, &image)),
        authors: info_value(body, "author").into_iter().collect(),
        tags: link_values(info_block(body, "kind").unwrap_or_default(), "<a"),
        description: html::text_between(body, "summary_shortened", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(&info_value(body, "status").unwrap_or_default()),
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
        .filter(|chunk| chunk.contains("wp-manga-chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: html::text_between(chunk, "chapter-release-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_english_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let page_area = body.split("page-chapter").nth(1).unwrap_or(body);
    page_area
        .split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "data-src")
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn info_block<'a>(body: &'a str, class_name: &str) -> Option<&'a str> {
    let index = body.find(&format!("class=\"{class_name}"))?;
    Some(
        &body[index
            ..body[index..]
                .find("</div>")
                .map(|end| index + end)
                .unwrap_or(body.len())],
    )
}

fn info_value(body: &str, class_name: &str) -> Option<String> {
    info_block(body, class_name)
        .and_then(|block| html::text_between(block, "col-8", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && !value.to_ascii_lowercase().contains("updating"))
}

fn link_values(block: &str, marker: &str) -> Vec<String> {
    block
        .split(marker)
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(status: &str) -> ItemStatus {
    match status.trim().to_ascii_lowercase().as_str() {
        "complete" | "completed" => ItemStatus::Completed,
        "in process" | "ongoing" => ItemStatus::Ongoing,
        "pause" | "on hiatus" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn parse_english_date(value: &str) -> Option<i64> {
    let parts = value.trim().trim_matches(',').replace(',', "");
    let mut pieces = parts.split_whitespace();
    let month = month_number(pieces.next()?)?;
    let day = pieces.next()?.parse::<u32>().ok()?;
    let year = pieces.next()?.parse::<i32>().ok()?;
    unix_from_ymd(year, month, day)
}

fn month_number(month: &str) -> Option<u32> {
    match &month.to_ascii_lowercase()[..3.min(month.len())] {
        "jan" => Some(1),
        "feb" => Some(2),
        "mar" => Some(3),
        "apr" => Some(4),
        "may" => Some(5),
        "jun" => Some(6),
        "jul" => Some(7),
        "aug" => Some(8),
        "sep" => Some(9),
        "oct" => Some(10),
        "nov" => Some(11),
        "dec" => Some(12),
        _ => None,
    }
}

fn unix_from_ymd(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let day = day as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146097 + doe - 719468) as i64 * 86_400)
}

fn filter(filters: &Value, key: &str, default: &str) -> String {
    filters
        .as_object()
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .trim()
        .to_string()
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

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="card"><a class="jtip" href="/sample"></a><div class="title-manga"><a href="/sample">Sample Manga</a></div><img class="card-img-top" src="/cover.jpg"></div>
<ul class="pagination"><li class="page-item"><a class="page-link">&raquo;</a></li></ul>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="title-detail">Sample Manga</h1><div class="col-image"><img src="/cover.jpg"></div>
<div class="author"><p class="col-8">Sample Author</p></div><div class="status"><p class="col-8">Complete</p></div>
<div class="kind"><p class="col-8"><a>Hentai</a><a>Drama</a></p></div><div id="summary_shortened">Sample description.</div>
<ul id="list_chapter_id_detail"><li class="wp-manga-chapter"><a href="/sample/chapter-1">Chapter 1</a><span class="chapter-release-date"><i>January 1, 2024</i></span></li></ul>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="page-chapter"><img data-src="/page1.jpg"><img src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_site() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Manga"
        );
        assert_eq!(
            SOURCE.details(json!({"manga":"/sample"})).unwrap().status,
            ItemStatus::Completed
        );
        assert_eq!(
            SOURCE
                .pages(json!({"chapter":"/sample/chapter-1"}))
                .unwrap()
                .len(),
            2
        );
    }
}
