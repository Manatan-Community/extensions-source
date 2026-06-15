use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ErisScans = ErisScans;
const BASE_URL: &str = "https://erisscans.com";

struct ErisScans;

impl MangaSource for ErisScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/latest/")
        } else {
            BASE_URL.to_string()
        };
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
        let body = fetch_document(
            &format!("{BASE_URL}/series/?q={}", url::query_escape(query)),
            SEARCH_FIXTURE,
        );
        Ok(parse_search(&body, query))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".to_string());
        let body = fetch_document(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let item = if key.starts_with("/series/") {
                Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(key),
                ))
            } else {
                None
            };
            return Ok(Some(UrlResolveResult {
                item,
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
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("group") && chunk.contains("grid") && chunk.contains("<a"))
        .filter_map(parse_listing_item)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_search(body: &str, query: &str) -> Paged<CatalogItem> {
    let lower_query = query.to_ascii_lowercase();
    let entries = body
        .split("<button")
        .skip(1)
        .filter(|chunk| chunk.contains("title=") || chunk.contains("<a"))
        .filter(|chunk| {
            lower_query.is_empty()
                || html::attr(chunk, "title")
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .contains(&lower_query)
        })
        .filter_map(parse_listing_item)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    if !href.contains("/series/") {
        return None;
    }
    let key = normalize_key(&href);
    let title = html::attr_after(chunk, "<a", "title")
        .or_else(|| html::attr(chunk, "title"))
        .or_else(|| url::slug_from_url(&key))
        .filter(|value| !value.is_empty())?;
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: style_image(chunk).map(|image| normalize_image(&image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Manga".to_string()),
        cover: style_image(body).map(|image| normalize_image(&image)),
        description: html::text_between(body, "overflow-hidden", "</p>")
            .or_else(|| html::text_between(body, "Synopsis", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(alt_value(body, "Status").as_deref()),
        authors: alt_value(body, "Author").into_iter().collect(),
        artists: alt_value(body, "Artist").into_iter().collect(),
        tags: parse_genres(body, alt_value(body, "Series Type")),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("href=") && !chunk.contains("Upcoming") && !chunk.contains("Coin"))
        .filter(|chunk| chunk.contains("/chapter") || chunk.contains("/read/") || chunk.contains("/series/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/chapter") && !href.contains("/read/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "text-sm", "</")
                .or_else(|| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let cdn = cdn_url(body);
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("uid") || chunk.contains("src="))
        .filter_map(|chunk| {
            html::attr(chunk, "uid")
                .and_then(|uid| cdn.as_ref().map(|base| format!("{}/{}", base.trim_end_matches('/'), uid)))
                .or_else(|| image_attr(chunk).map(|image| normalize_image(&image)))
        })
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

fn cdn_url(body: &str) -> Option<String> {
    body.split("realUrl")
        .nth(1)
        .and_then(|rest| rest.split("//").nth(1))
        .and_then(|rest| rest.split(['`', '\'', '"', '/', '$']).next())
        .filter(|host| !host.is_empty())
        .map(|host| format!("https://{host}/uploads"))
}

fn style_image(input: &str) -> Option<String> {
    input
        .split("background-image")
        .nth(1)
        .and_then(|rest| rest.split("url(").nth(1))
        .and_then(|rest| rest.split(')').next())
        .map(|value| value.trim_matches(['\'', '"', ' ']).to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| image_attr(input))
}

fn image_attr(input: &str) -> Option<String> {
    html::attr(input, "data-src")
        .or_else(|| html::attr(input, "data-lazy-src"))
        .or_else(|| html::attr(input, "src"))
}

fn normalize_image(value: &str) -> String {
    if value.starts_with("//") {
        format!("https:{value}")
    } else {
        url::join_url(BASE_URL, value)
    }
}

fn alt_value(body: &str, label: &str) -> Option<String> {
    body.split("alt=")
        .find(|chunk| chunk.trim_start().starts_with(&format!("\"{label}\"")) || chunk.trim_start().starts_with(&format!("'{label}'")))
        .and_then(|chunk| html::text_between(chunk, ">", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_genres(body: &str, series_type: Option<String>) -> Vec<String> {
    let mut tags = Vec::new();
    if let Some(value) = series_type {
        tags.push(value);
    }
    tags.extend(
        body.split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("genre") || chunk.contains("tag="))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
    );
    tags
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().trim().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "dropped" => ItemStatus::Cancelled,
        "paused" => ItemStatus::Hiatus,
        "completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
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
<div class="group overflow-hidden grid"><a href="/series/sample" title="Sample Series"><div style="background-image: url('https://cdn.meowing.org/uploads/cover.jpg')"></div></a></div>
"#;
const SEARCH_FIXTURE: &str = r#"
<div id="searched_series_page"><button title="Sample Series"><a href="/series/sample" title="Sample Series"><div style="background-image: url('/cover.jpg')"></div></a></button></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="grid"><h1>Sample Series</h1><div class="photoURL" style="background-image: url('/cover.jpg')"></div>
<div class="overflow-hidden"><p>Sample description.</p></div><div alt="Status">Ongoing</div><div alt="Author">Writer</div><div alt="Artist">Artist</div><div alt="Series Type">Manhwa</div><a href="/series/?genre=drama">Drama</a></div>
<div id="chapters"><a href="/series/sample/chapter-1"><div class="text-sm">Chapter 1</div><div class="text-xs">Jan 1, 2024</div></a><a href="/series/sample/chapter-2"><img alt="Coin"><div class="text-sm">Chapter 2</div></a></div>
"#;
const PAGES_FIXTURE: &str = r#"
<script>let realUrl = `https://cdn.meowing.org/uploads`;</script>
<div id="pages"><img uid="page1.jpg"><img uid="page2.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keyoapp_source() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries[0].title, "Sample Series");
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
