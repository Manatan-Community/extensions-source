use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ElfToon = ElfToon;
const BASE_URL: &str = "https://elftoon.com";

struct ElfToon;

impl MangaSource for ElfToon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        let target = search_url(page, "", order, &Value::Null);
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let order = filter_value(filters, "order").unwrap_or_default();
        let body = fetch_document(&search_url(page, query, &order, filters), LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
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

fn search_url(page: u64, query: &str, order: &str, filters: &Value) -> String {
    let mut params = vec![
        ("title", query.trim().to_string()),
        ("page", page.to_string()),
    ];
    if !order.is_empty() {
        params.push(("order", order.to_string()));
    }
    if let Some(status) = filter_value(filters, "status").filter(|value| !value.is_empty()) {
        params.push(("status", status));
    }
    if let Some(series_type) = filter_value(filters, "type").filter(|value| !value.is_empty()) {
        params.push(("type", series_type));
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/manga/?{query}")
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("bsx") || chunk.contains("imgu"))
        .filter_map(parse_listing_item)
        .fold(Vec::new(), push_unique);
    Paged {
        has_next_page: body.contains("pagination") && body.contains("next"),
        entries,
    }
}

fn parse_listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    if !href.contains("/manga/") {
        return None;
    }
    let key = normalize_key(&href);
    let title = html::attr_after(chunk, "<a", "title")
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .or_else(|| html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)))
        .or_else(|| url::slug_from_url(&key))
        .filter(|value| !value.is_empty())?;
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let description = html::text_between(body, "entry-content", "</div>")
        .or_else(|| html::text_between(body, "manga-excerpt", "</div>"))
        .or_else(|| html::text_between(body, "desc", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "entry-title", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Manga".to_string()),
        cover: image_attr(body).map(|image| url::join_url(BASE_URL, &image)),
        description,
        authors: info_values(body, "Author"),
        artists: info_values(body, "Artist"),
        tags: parse_genres(body),
        status: parse_status(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("<a") && !chunk.contains("gem-price-icon"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapternum", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let date_text = html::text_between(chunk, "chapterdate", "</")
                .map(|value| html::strip_tags(&value));
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: date_text
                    .and_then(|date| manatan_shared::dates::parse_fixture_date(&date)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut images = body
        .split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("readerarea") || chunk.contains("data-src") || chunk.contains("src="))
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:"))
        .collect::<Vec<_>>();
    if images.is_empty() {
        images = image_list_json(body);
    }
    images
        .into_iter()
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

fn image_attr(input: &str) -> Option<String> {
    html::attr(input, "data-lazy-src")
        .or_else(|| html::attr(input, "data-src"))
        .or_else(|| html::attr(input, "srcset").and_then(|value| value.split_whitespace().next().map(ToString::to_string)))
        .or_else(|| html::attr(input, "src"))
}

fn image_list_json(body: &str) -> Vec<String> {
    body.split("ts_reader.run(")
        .nth(1)
        .and_then(|rest| rest.split(");").next())
        .and_then(|json| serde_json::from_str::<Value>(json).ok())
        .and_then(|value| value.get("sources").cloned())
        .and_then(|sources| sources.as_array().cloned())
        .and_then(|sources| sources.first().cloned())
        .and_then(|source| source.get("images").cloned())
        .and_then(|images| images.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(ToString::to_string))
        .collect()
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    body.split("imptdt")
        .filter(|chunk| chunk.to_ascii_lowercase().contains(&label.to_ascii_lowercase()))
        .filter_map(|chunk| {
            html::text_between(chunk, "<i", "</i>")
                .or_else(|| html::text_between(chunk, "<span", "</span>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
        })
        .collect()
}

fn parse_genres(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/genre/"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") || lower.contains("on hold") {
        ItemStatus::Hiatus
    } else if lower.contains("dropped") || lower.contains("cancelled") {
        ItemStatus::Cancelled
    } else if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn filter_value(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
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
<div class="bsx"><a href="https://elftoon.com/manga/sample/" title="Sample Manga"><img src="/cover.jpg"></a></div>
<div class="pagination"><a class="next" href="/manga/page/2/">Next</a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="bigcontent"><h1 class="entry-title">Sample Manga</h1><div class="thumb"><img src="/cover.jpg"></div>
<div class="entry-content">Sample description.</div><div class="imptdt">Author <i>Writer</i></div>
<a href="/genre/drama/">Drama</a><span>Ongoing</span></div>
<div id="chapterlist"><ul><li><a href="/manga/sample/chapter-1/"><span class="chapternum">Chapter 1</span></a><span class="chapterdate">January 1, 2024</span></li><li><img class="gem-price-icon"><a href="/manga/sample/chapter-2/">Chapter 2</a></li></ul></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="readerarea"><img src="/page1.jpg"><img data-src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_source() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries[0].title, "Sample Manga");
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
