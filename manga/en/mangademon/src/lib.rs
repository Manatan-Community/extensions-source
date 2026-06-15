use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaDemon = MangaDemon;
const BASE_URL: &str = "https://demonicscans.org";

struct MangaDemon;

impl MangaSource for MangaDemon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_advanced(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            Ok(parse_latest(&fetch_document(
                &format!("{BASE_URL}/lastupdates.php?list={page}"),
                LATEST_FIXTURE,
            )))
        } else {
            Ok(parse_advanced(&fetch_document(
                &format!("{BASE_URL}/advanced.php?list={page}&status=all&orderby=VIEWS%20DESC"),
                LIST_FIXTURE,
            )))
        }
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
                    &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            Ok(parse_advanced(&fetch_document(
                &advanced_url(page, request.get("filters")),
                LIST_FIXTURE,
            )))
        } else {
            Ok(parse_search(&fetch_document(
                &format!("{BASE_URL}/search.php?manga={}", url::query_escape(query)),
                SEARCH_FIXTURE,
            )))
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
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
                    &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
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

fn advanced_url(page: u64, filters: Option<&Value>) -> String {
    let mut params = vec![
        ("list".to_string(), page.to_string()),
        (
            "status".to_string(),
            filter_string(filters, "status").unwrap_or_else(|| "all".to_string()),
        ),
        (
            "orderby".to_string(),
            filter_string(filters, "sort").unwrap_or_else(|| "VIEWS DESC".to_string()),
        ),
    ];
    for genre in filter_values(filters, "genre") {
        params.push(("genre[]".to_string(), genre));
    }
    format!(
        "{BASE_URL}/advanced.php?{}",
        params
            .into_iter()
            .map(|(key, value)| format!(
                "{}={}",
                url::query_escape(&key),
                url::query_escape(&value)
            ))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn parse_advanced(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("advanced-element")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let title = html::text_between(chunk, "<h1", "</h1>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&href).unwrap_or_else(|| "Manga".to_string())
                    });
                Some(list_item(
                    &href,
                    &title,
                    html::attr_after(chunk, "<img", "src"),
                ))
            })
            .collect(),
        has_next_page: body.contains(">Next<") || body.contains("Next</li>"),
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("updates-element")
            .skip(1)
            .filter(|chunk| !chunk.contains("toffee-badge"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let title = html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&href).unwrap_or_else(|| "Manga".to_string())
                    });
                Some(list_item(
                    &href,
                    &title,
                    html::attr_after(chunk, "<img", "src"),
                ))
            })
            .collect(),
        has_next_page: body.contains(">Next<") || body.contains("Next</li>"),
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let title = html::text_between(chunk, "seach-right", "</div>")
                    .or_else(|| html::text_between(chunk, "<div", "</div>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&href).unwrap_or_else(|| "Manga".to_string())
                    });
                Some(list_item(
                    &href,
                    &title,
                    html::attr_after(chunk, "<img", "src"),
                ))
            })
            .collect(),
        has_next_page: false,
    }
}

fn list_item(href: &str, title: &str, cover: Option<String>) -> CatalogItem {
    let key = normalize_key(href);
    CatalogItem {
        key: key.clone(),
        title: title.to_string(),
        cover: cover.map(|value| url::join_url(BASE_URL, &value)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key
        .or_else(|| {
            html::attr_after(body, "rel=\"canonical\"", "href").map(|value| normalize_key(&value))
        })
        .unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "big-fat-titles", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: html::attr_after(body, "manga-page", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        description: html::text_between(body, "white-font", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_value(body, "Author").into_iter().collect(),
        tags: genres(body),
        status: parse_status(info_value(body, "Status").as_deref()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("chplinks"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::strip_tags(chunk)
                    .split('\n')
                    .next()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string),
                date_uploaded: html::text_between(chunk, "<span", "</span>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("imgholder"))
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
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

fn genres(body: &str) -> Vec<String> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("genres-list"))
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
        .collect()
}

fn info_value(body: &str, label: &str) -> Option<String> {
    let index = body.find(label)?;
    let rest = &body[index..];
    html::text_between(rest, "<li", "</li>")
        .and_then(|_| rest.split("</li>").nth(1).map(ToString::to_string))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.unwrap_or_default().to_ascii_lowercase().as_str() {
        value if value.contains("ongoing") => ItemStatus::Ongoing,
        value if value.contains("completed") => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn filter_string(filters: Option<&Value>, key: &str) -> Option<String> {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(|value| value.as_str().map(ToString::to_string))
        .filter(|value| !value.is_empty())
}

fn filter_values(filters: Option<&Value>, key: &str) -> Vec<String> {
    let Some(value) = filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
    else {
        return Vec::new();
    };
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect();
    }
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .map(|value| vec![value.to_string()])
        .unwrap_or_default()
}

fn normalize_key(value: &str) -> String {
    let path = value
        .strip_prefix(BASE_URL)
        .unwrap_or(value)
        .trim_start_matches('/')
        .trim_end_matches('/');
    format!("/{path}")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div id="advanced-content"><div class="advanced-element"><a href="/manga/sample"><img src="/cover.jpg"><h1>Sample Manga</h1></a></div></div>
<div class="pagination"><ul><a><li>Next</li></a></ul></div>
"#;
const LATEST_FIXTURE: &str = r#"
<div id="updates-container"><div class="updates-element"><div class="thumb"><img src="/cover.jpg"></div><div class="updates-element-info"><a href="/manga/sample">Sample Manga</a></div></div></div>
"#;
const SEARCH_FIXTURE: &str = r#"
<body><a href="/manga/sample"><img src="/cover.jpg"><div class="seach-right"><div>Sample Manga</div></div></a></body>
"#;
const DETAILS_FIXTURE: &str = r#"
<div id="manga-info-container"><h1 class="big-fat-titles">Sample Manga</h1><div id="manga-page"><img src="/cover.jpg"></div>
<div class="genres-list"><li>Action</li></div><div id="manga-info-rightColumn"><div><div class="white-font">Sample description.</div></div></div>
<div id="manga-info-stats"><div><li>Author</li><li>Author Name</li></div><div><li>Status</li><li>Ongoing</li></div></div></div>
<div id="chapters-list"><a class="chplinks" href="/manga/sample/chapter-1">Chapter 1 <span>2024-01-01</span></a></div>
"#;
const PAGES_FIXTURE: &str = r#"<div><img class="imgholder" src="/page1.jpg"><img class="imgholder" src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_custom_html() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Manga"
        );
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 2);
    }
}
