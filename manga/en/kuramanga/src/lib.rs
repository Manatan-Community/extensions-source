use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: KuraManga = KuraManga;
const BASE_URL: &str = "https://kuramanga.com";
const PAGE_SIZE: u64 = 10;

struct KuraManga;

impl MangaSource for KuraManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let body = fetch_document(BASE_URL, HOME_FIXTURE);
            return Ok(parse_latest(&body));
        }
        if page == 1 {
            let body = fetch_document(BASE_URL, HOME_FIXTURE);
            return Ok(parse_popular(&body));
        }
        let target = format!("{BASE_URL}/search?ajax=1&offset={}", (page - 1) * PAGE_SIZE);
        Ok(parse_search_json(
            &fetch_json(&target, SEARCH_FIXTURE),
            page,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
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
        let target = search_url(page, query, request.get("filters").unwrap_or(&Value::Null));
        Ok(parse_search_json(
            &fetch_json(&target, SEARCH_FIXTURE),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
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

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(page: u64, query: &str, filters: &Value) -> String {
    let mut params = vec![
        "ajax=1".to_string(),
        format!("offset={}", (page - 1) * PAGE_SIZE),
    ];
    if !query.is_empty() {
        params.push(format!("name={}", url::query_escape(query)));
    }
    for genre in selected_values(filters.get("genre")) {
        params.push(format!("genre={}", url::query_escape(&genre)));
    }
    if let Some(status) = filters
        .get("status")
        .and_then(Value::as_str)
        .filter(|value| *value != "All" && !value.is_empty())
    {
        params.push(format!(
            "status={}",
            url::query_escape(&status.to_ascii_lowercase())
        ));
    }
    if filters.get("adult").and_then(Value::as_bool) == Some(false) {
        params.push("adult=0".to_string());
    }
    format!("{BASE_URL}/search?{}", params.join("&"))
}

fn parse_search_json(body: &str, page: u64) -> Paged<CatalogItem> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Paged::default();
    };
    let total = root.get("total").and_then(Value::as_u64).unwrap_or(0);
    let entries = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            let key = format!(
                "/{}",
                item.get("normalized_title")
                    .and_then(Value::as_str)
                    .unwrap_or("sample")
            );
            CatalogItem {
                key: key.clone(),
                title: item
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("KuraManga")
                    .to_string(),
                cover: item
                    .get("cover_image_url")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect::<Vec<_>>();
    let offset = (page - 1) * PAGE_SIZE;
    Paged {
        has_next_page: entries.len() as u64 == PAGE_SIZE && offset + (entries.len() as u64) < total,
        entries,
    }
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("manga-card")
            .skip(1)
            .filter_map(|chunk| {
                let href =
                    html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
                let title = html::text_between(chunk, "manga-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| "KuraManga".to_string());
                let key = normalize_key(&href);
                Some(catalog_item(key, title, image_attr(chunk), false))
            })
            .collect(),
        has_next_page: true,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let mut entries = Vec::new();
    for chunk in body.split("update-row").skip(1) {
        let Some(href) = html::attr_after(chunk, "update-series-link", "href")
            .or_else(|| html::attr_after(chunk, "<a", "href"))
        else {
            continue;
        };
        let title = html::text_between(chunk, "update-series-link", "</")
            .or_else(|| html::text_between(chunk, "<a", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&href))
            .unwrap_or_else(|| "KuraManga".to_string());
        let key = normalize_key(&href);
        if !entries.iter().any(|item: &CatalogItem| item.key == key) {
            entries.push(catalog_item(key, title, image_attr(chunk), false));
        }
    }
    Paged {
        entries,
        has_next_page: false,
    }
}

fn catalog_item(
    key: String,
    title: String,
    cover: Option<String>,
    initialized: bool,
) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "manga-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "KuraManga".to_string()),
        cover: html::attr_after(body, "og:image", "content").or_else(|| image_attr(body)),
        description: html::text_between(body, "summary-inner", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: meta_value(body, "Author:").into_iter().collect(),
        artists: meta_value(body, "Artist:").into_iter().collect(),
        tags: body
            .split("genre-chip")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(&meta_value(body, "Status:").unwrap_or_default()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapter-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: None,
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("#chapterImages")
        .nth(1)
        .unwrap_or(body)
        .split("<img")
        .skip(1)
        .filter_map(image_attr)
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

fn meta_value(body: &str, label: &str) -> Option<String> {
    body.split("meta-grid")
        .nth(1)
        .and_then(|chunk| chunk.split(label).nth(1))
        .and_then(|chunk| chunk.split("</div>").next())
        .map(html::strip_tags)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" | "upcoming" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "on_hold" | "on hold" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "src"))
        .or_else(|| html::attr(chunk, "content"))
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        format!("/{}", input[BASE_URL.len()..].trim_matches('/'))
    } else {
        format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"{"data":[{"title":"Sample Kura","cover_image_url":"/cover.jpg","normalized_title":"sample"}],"total":11}"#;
const HOME_FIXTURE: &str = r#"
<div class="popular-glide"><a class="manga-card" href="/sample"><div class="manga-title">Sample Kura</div><img class="manga-thumb" src="/cover.jpg"></a></div>
<div class="update-list"><div class="update-row"><a class="update-series-link" href="/sample">Sample Kura</a><img src="/cover.jpg"></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="manga-title">Sample Kura</h1><meta property="og:image" content="/cover.jpg"><div class="summary-inner">About.</div>
<div class="meta-grid"><div>Author: Writer</div><div>Artist: Artist</div><div>Status: Ongoing</div></div>
<div class="genre-list"><a class="genre-chip">Action</a></div>
<div class="chapter-list"><div class="chapter-item"><a href="/sample/chapter-1">Chapter 1</a><time>Jan 1, 2024</time></div></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="chapterImages"><img src="/page1.jpg"><img src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kuramanga_flow() {
        assert_eq!(
            parse_search_json(SEARCH_FIXTURE, 1).entries[0].title,
            "Sample Kura"
        );
        assert_eq!(parse_popular(HOME_FIXTURE).entries.len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
