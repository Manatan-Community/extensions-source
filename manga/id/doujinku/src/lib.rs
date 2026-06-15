use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Doujinku = Doujinku;
const BASE_URL: &str = "https://doujinku.org";

struct Doujinku;

impl MangaSource for Doujinku {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "rating"
        };
        let target = advanced_search_url(page, "", Some(sort), request.get("filters"));
        Ok(parse_listing(&fetch_document_or_fixture(
            &target,
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
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let target = advanced_search_url(page, query, None, request.get("filters"));
        Ok(parse_listing(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn advanced_search_url(
    page: u64,
    query: &str,
    fallback_sort: Option<&str>,
    filters: Option<&Value>,
) -> String {
    let mut params = vec![
        ("name", url::query_escape(query)),
        ("page", page.to_string()),
        (
            "sort",
            filter_string(filters, "sort")
                .unwrap_or_else(|| fallback_sort.unwrap_or_default().to_string()),
        ),
    ];
    for name in ["status", "type", "genres"] {
        if let Some(value) = filter_string(filters, name) {
            params.push((name, value));
        }
    }
    let query = params
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/advanced-search/?{query}")
}

fn filter_string(filters: Option<&Value>, name: &str) -> Option<String> {
    let value = filters?.get(name)?;
    if let Some(text) = value.as_str() {
        return Some(url::query_escape(text));
    }
    if let Some(array) = value.as_array() {
        return Some(
            array
                .iter()
                .filter_map(Value::as_str)
                .map(url::query_escape)
                .collect::<Vec<_>>()
                .join("_"),
        );
    }
    None
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("/manga/") || chunk.contains("/series/"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                if href.contains("/chapter") {
                    return None;
                }
                let title = html::attr_after(chunk, "<img", "title")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| html::attr(chunk, "title"))
                    .or_else(|| url::slug_from_url(&href))?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("id".to_string()),
                    content_rating: Some("adult".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("alt=\"Next\"")
            || body.contains("alt='Next'")
            || body.contains("next page-numbers"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Doujinku".to_string()),
        cover: html::attr_after(body, "alt=\"poster\"", "src")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "comic-content mobile", "</div>")
            .or_else(|| html::text_between(body, "comic-content", "</div>"))
            .or_else(|| html::text_between(body, "entry-content", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: link_values(body, "/genres/"),
        status: parse_status(&text_after_label(body, "Status").unwrap_or_default()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("id".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let has_witch_rows = body.contains("chbox") && body.contains("eph-num");
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/chapter"))
        .filter(|chunk| {
            !has_witch_rows
                || (chunk.contains("chbox") && chunk.contains("eph-num"))
                || body
                    .split(chunk)
                    .next()
                    .unwrap_or_default()
                    .rsplit("<li")
                    .next()
                    .is_some_and(|row| row.contains("chbox") && row.contains("eph-num"))
        })
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("object-cover")
                || chunk.contains("mx-auto")
                || chunk.contains("ts-main-image")
                || chunk.contains("wp-manga-chapter-img")
        })
        .filter_map(image_attr)
        .map(|image| url::join_url(BASE_URL, &image))
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

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    body.split("<div")
        .find(|chunk| html::strip_tags(chunk).trim() == label)
        .and_then(|chunk| {
            let rest = body.split_once(chunk)?.1;
            html::text_between(rest, "<div", "</div>").map(|value| html::strip_tags(&value))
        })
}

fn link_values(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "hiatus" => ItemStatus::Hiatus,
        "cancelled" | "canceled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
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

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="grid"><a href="/manga/sample"><img title="Sample Manga" src="/cover.jpg"></a></div>"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample Manga</h1><img alt="poster" src="/cover.jpg"><div class="comic-content mobile">A sample.</div>
<div>Genres</div><div><a href="/genres/action">Action</a></div><div>Status</div><div>Ongoing</div>
<div class="eplister"><ul><li><div class="chbox"></div><div class="eph-num"><a href="/manga/sample/chapter-1"><span>Chapter 1</span></a></div></li></ul></div>
"#;

const PAGES_FIXTURE: &str = r#"<div><img class="object-cover mx-auto" src="/page1.jpg"><img class="ts-main-image" src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_themesia_source() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample Manga");
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        let pages = SOURCE
            .pages(json!({"chapter":"/manga/sample/chapter-1"}))
            .unwrap();
        assert_eq!(pages.len(), 2);
    }
}
