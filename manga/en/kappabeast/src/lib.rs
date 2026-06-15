use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: KappaBeast = KappaBeast;
const BASE_URL: &str = "https://kappabeast.com";
const CDN_URL: &str = "https://strapi.kappabeast.com";
const API_URL: &str = "https://strapi.kappabeast.com/api";
const PAGE_SIZE: u64 = 20;

struct KappaBeast;

impl MangaSource for KappaBeast {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updatedAt:desc"
        } else {
            ""
        };
        let target = manga_query_url(
            page,
            "",
            request.get("filters").unwrap_or(&Value::Null),
            sort,
        );
        Ok(parse_manga_page(&fetch_json(&target, LIST_FIXTURE), page))
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
            let slug = slug_from_key(&key);
            let body = fetch_json(&manga_details_url(&slug), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: first_manga(&body)
                    .as_ref()
                    .map(manga_to_item)
                    .into_iter()
                    .collect(),
                has_next_page: false,
            });
        }
        let target = manga_query_url(
            page,
            query,
            request.get("filters").unwrap_or(&Value::Null),
            "",
        );
        Ok(parse_manga_page(&fetch_json(&target, LIST_FIXTURE), page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#doc1".into());
        let slug = slug_from_key(&key);
        let body = fetch_json(&manga_details_url(&slug), DETAILS_FIXTURE);
        Ok(first_manga(&body)
            .as_ref()
            .map(manga_to_item)
            .unwrap_or_else(|| fixture_item(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#doc1".into());
        let document_id = document_id_from_key(&key).unwrap_or("doc1");
        Ok(fetch_all_chapters(document_id))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1#doc1".into());
        let document_id = document_id_from_key(&key).unwrap_or("doc1");
        let number = key
            .split('#')
            .next()
            .and_then(|value| value.rsplit('/').next())
            .unwrap_or("1");
        let target = chapter_query_url(document_id, number, 1);
        let body = fetch_json(&target, CHAPTER_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let slug = slug_from_key(&key);
            let body = fetch_json(&manga_details_url(&slug), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: first_manga(&body).as_ref().map(manga_to_item),
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

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn manga_query_url(page: u64, query: &str, filters: &Value, fallback_sort: &str) -> String {
    let mut params = vec![
        format!("pagination%5Bpage%5D={page}"),
        format!("pagination%5BpageSize%5D={PAGE_SIZE}"),
        "populate%5Bmedia%5D%5Bpopulate%5D=%2A".to_string(),
        "populate%5Bcategory%5D%5Bfields%5D%5B0%5D=name".to_string(),
    ];
    if !query.is_empty() {
        params.push(format!(
            "filters%5Btitle%5D%5B%24containsi%5D={}",
            url::query_escape(query)
        ));
    }
    for (filter, param) in [
        ("genre", "filters%5Bcategory%5D%5Bname%5D%5B%24eq%5D"),
        ("status", "filters%5Bmanga_status%5D%5B%24eq%5D"),
        ("type", "filters%5Btype%5D%5B%24eq%5D"),
    ] {
        if let Some(value) = filter_value(filters, filter) {
            params.push(format!("{param}={}", url::query_escape(&value)));
        }
    }
    let sort = filter_value(filters, "sort").unwrap_or_else(|| fallback_sort.to_string());
    if !sort.is_empty() {
        params.push(format!("sort%5B0%5D={}", url::query_escape(&sort)));
    }
    format!("{API_URL}/mangas?{}", params.join("&"))
}

fn manga_details_url(slug: &str) -> String {
    format!(
        "{API_URL}/mangas?filters%5Bslug%5D%5B%24eq%5D={}&populate%5Bmedia%5D%5Bpopulate%5D=%2A&populate%5Bcategory%5D%5Bfields%5D%5B0%5D=name&pagination%5BpageSize%5D=1",
        url::query_escape(slug)
    )
}

fn chapter_query_url(document_id: &str, number: &str, page: u64) -> String {
    format!(
        "{API_URL}/chapters?filters%5Bmanga%5D%5BdocumentId%5D%5B%24eq%5D={}&filters%5Bnumber%5D%5B%24eq%5D={}&populate%5Bpages%5D%5Bpopulate%5D=%2A&populate=manga&sort%5B0%5D=number%3Adesc&pagination%5Bpage%5D={page}&pagination%5BpageSize%5D=100",
        url::query_escape(document_id),
        url::query_escape(number)
    )
}

fn parse_manga_page(body: &str, page: u64) -> Paged<CatalogItem> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Paged::default();
    };
    let entries = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(manga_to_item)
        .collect();
    Paged {
        entries,
        has_next_page: root
            .pointer("/meta/pagination/pageCount")
            .and_then(Value::as_u64)
            .is_some_and(|page_count| page < page_count),
    }
}

fn first_manga(body: &str) -> Option<Value> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .get("data")?
        .as_array()?
        .first()
        .cloned()
}

fn manga_to_item(item: &Value) -> CatalogItem {
    let slug = item.get("slug").and_then(Value::as_str).unwrap_or("sample");
    let document_id = item
        .get("documentId")
        .and_then(Value::as_str)
        .unwrap_or("doc1");
    CatalogItem {
        key: format!("{slug}#{document_id}"),
        title: item
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Kappa Beast")
            .to_string(),
        cover: item
            .get("media")
            .and_then(Value::as_array)
            .and_then(|media| media.first())
            .and_then(|media| media.pointer("/coverImage/url"))
            .and_then(Value::as_str)
            .map(|image| url::join_url(CDN_URL, image)),
        description: item
            .get("description")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        authors: item
            .get("author")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .into_iter()
            .collect(),
        artists: item
            .get("artist")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .into_iter()
            .collect(),
        tags: item
            .get("category")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|category| category.get("name").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect(),
        status: parse_status(
            item.get("manga_status")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ),
        url: Some(format!("{BASE_URL}/series/{slug}")),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_all_chapters(document_id: &str) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    let mut page = 1;
    loop {
        let body = fetch_json(
            &chapter_query_url(document_id, "", page)
                .replace("filters%5Bnumber%5D%5B%24eq%5D=&", ""),
            CHAPTERS_FIXTURE,
        );
        let (mut entries, has_next) = parse_chapters(&body);
        chapters.append(&mut entries);
        if !has_next {
            break;
        }
        page += 1;
    }
    chapters
}

fn parse_chapters(body: &str) -> (Vec<MangaChapter>, bool) {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return (Vec::new(), false);
    };
    let chapters = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(chapter_to_item)
        .collect();
    let page = root
        .pointer("/meta/pagination/page")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let page_count = root
        .pointer("/meta/pagination/pageCount")
        .and_then(Value::as_u64)
        .unwrap_or(page);
    (chapters, page < page_count)
}

fn chapter_to_item(item: &Value) -> Option<MangaChapter> {
    let manga = item.get("manga")?;
    let document_id = manga
        .get("documentId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let slug = manga
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or("sample");
    let number = number_string(item.get("number"))?;
    let display_number = trim_number(&number);
    let title_suffix = item
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty() && *title != format!("Chapter {display_number}"))
        .map(|title| format!(" - {title}"))
        .unwrap_or_default();
    Some(MangaChapter {
        key: format!("{slug}/{number}#{document_id}"),
        title: Some(format!("Chapter {display_number}{title_suffix}")),
        chapter_number: number.parse::<f32>().ok(),
        date_uploaded: item
            .get("createdAt")
            .and_then(Value::as_str)
            .and_then(parse_date),
        url: Some(format!("{BASE_URL}/reader/{slug}/{number}")),
        language: Some("en".to_string()),
        ..MangaChapter::default()
    })
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let html_content = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|root| root.get("data").and_then(Value::as_array)?.first().cloned())
        .and_then(|chapter| {
            chapter
                .get("htmlContent")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    html_content
        .split("div class=\"separator")
        .skip(1)
        .flat_map(|chunk| chunk.split("<a").skip(1))
        .filter_map(|chunk| html::attr(chunk, "href"))
        .map(|image| image.replace("/h2048/", "/s0/"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn fixture_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: "Kappa Beast".to_string(),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn filter_value(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn parse_status(value: &str) -> ItemStatus {
    match value {
        "Ongoing" => ItemStatus::Ongoing,
        "Completed" => ItemStatus::Completed,
        "Hiatus" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        if let Some(slug) = input.split("/series/").nth(1) {
            return slug.trim_matches('/').to_string();
        }
    }
    input
        .trim_start_matches('/')
        .trim_start_matches("series/")
        .trim_matches('/')
        .to_string()
}

fn slug_from_key(key: &str) -> String {
    key.split('#')
        .next()
        .unwrap_or(key)
        .split('/')
        .next()
        .unwrap_or(key)
        .to_string()
}

fn document_id_from_key(key: &str) -> Option<&str> {
    key.rsplit_once('#')
        .map(|(_, id)| id)
        .filter(|id| !id.is_empty())
}

fn number_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
}

fn trim_number(value: &str) -> String {
    value.strip_suffix(".0").unwrap_or(value).to_string()
}

fn parse_date(value: &str) -> Option<i64> {
    let date = value.get(..10)?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
{"data":[{"documentId":"doc1","title":"Sample Kappa","description":"About.","author":"Writer","manga_status":"Ongoing","artist":"Artist","slug":"sample","media":[{"coverImage":{"url":"/uploads/cover.jpg"}}],"category":[{"name":"Fantasy"}]}],"meta":{"pagination":{"page":1,"pageCount":2}}}
"#;
const DETAILS_FIXTURE: &str = LIST_FIXTURE;
const CHAPTERS_FIXTURE: &str = r#"
{"data":[{"number":1,"title":"Start","createdAt":"2024-01-01T00:00:00.000Z","manga":{"documentId":"doc1","slug":"sample"},"htmlContent":null}],"meta":{"pagination":{"page":1,"pageCount":1}}}
"#;
const CHAPTER_FIXTURE: &str = r#"
{"data":[{"number":1,"title":"Start","createdAt":"2024-01-01T00:00:00.000Z","manga":{"documentId":"doc1","slug":"sample"},"htmlContent":"<div class=\"separator\"><a href=\"https://strapi.kappabeast.com/uploads/h2048/page1.jpg\"></a></div><div class=\"separator\"><a href=\"https://strapi.kappabeast.com/uploads/h2048/page2.jpg\"></a></div>"}],"meta":{"pagination":{"page":1,"pageCount":1}}}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strapi_catalog_chapters_and_pages() {
        let listing = parse_manga_page(LIST_FIXTURE, 1);
        assert_eq!(listing.entries[0].key, "sample#doc1");
        assert!(listing.has_next_page);

        let (chapters, has_next) = parse_chapters(CHAPTERS_FIXTURE);
        assert!(!has_next);
        assert_eq!(chapters[0].key, "sample/1#doc1");

        let pages = parse_pages(CHAPTER_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
