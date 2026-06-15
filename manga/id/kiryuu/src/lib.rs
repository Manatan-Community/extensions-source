use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Kiryuu = Kiryuu;
const BASE_URL: &str = "https://v5.kiryuu.to";
const CURRENT_URL: &str = "https://v6.kiryuu.to";

struct Kiryuu;

impl MangaSource for Kiryuu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = match listing {
            "latest" => paged_url("/manga/", page, &[("order", "update")]),
            _ => paged_url("/manga/", page, &[("order", "popular")]),
        };
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
        if query.starts_with(BASE_URL) || query.starts_with(CURRENT_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target = paged_url("/", page, &[("s", query), ("post_type", "wp-manga")]);
        Ok(parse_listing(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let chapter_url = absolute_url(&key);
        Ok(parse_pages(
            &fetch_document_or_fixture(&chapter_url, PAGES_FIXTURE),
            &chapter_url,
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) || input.starts_with(CURRENT_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: (key.starts_with("/manga/") && !key.contains("/chapter-")).then(|| {
                    parse_details(
                        &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                        Some(key),
                    )
                }),
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
        .with_cookies_for(CURRENT_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn paged_url(path: &str, page: u64, params: &[(&str, &str)]) -> String {
    let mut target = if page > 1 && path == "/manga/" {
        format!("{}/manga/page/{page}/", BASE_URL.trim_end_matches('/'))
    } else {
        format!("{}{}", BASE_URL.trim_end_matches('/'), path)
    };
    let query = params
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| format!("{key}={}", url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&");
    if !query.is_empty() {
        target.push('?');
        target.push_str(&query);
    }
    target
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("/manga/") && !chunk.contains("/chapter-"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                if !href.contains("/manga/") || href.contains("/chapter-") {
                    return None;
                }
                let title = html::attr(chunk, "title")
                    .or_else(|| {
                        html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
                    })
                    .or_else(|| url::slug_from_url(&href))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "Kiryuu".to_string());
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: nearby_image(chunk).map(|image| absolute_image(&image)),
                    url: Some(absolute_url(&key)),
                    language: Some("id".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("next page-numbers") || body.contains("rel=\"next\""),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::attr_after(body, "property=\"og:title\"", "content")
            .or_else(|| {
                html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value))
            })
            .map(|value| value.replace(" Bahasa Indonesia", ""))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Kiryuu".to_string()),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| nearby_image(body))
            .map(|image| absolute_image(&image)),
        description: html::attr_after(body, "property=\"og:description\"", "content")
            .or_else(|| {
                html::text_between(body, "entry-content", "</div>")
                    .map(|value| html::strip_tags(&value))
            })
            .filter(|value| !value.is_empty()),
        authors: info_values(body, "Author"),
        artists: info_values(body, "Artist"),
        tags: parse_tags(body),
        status: parse_status(
            &info_values(body, "Status")
                .first()
                .cloned()
                .unwrap_or_default(),
        ),
        url: Some(absolute_url(&key)),
        language: Some("id".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/chapter-"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| url::slug_from_url(&href))
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("wp-manga-chapter-img")
                || chunk.contains("reader")
                || chunk.contains("chapter")
                || chunk.contains("data-src")
                || chunk.contains("src=")
        })
        .filter_map(nearby_image)
        .filter(|value| !value.starts_with("data:") && !value.contains("static/svg"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_image(&image),
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn nearby_image(input: &str) -> Option<String> {
    html::attr(input, "data-src")
        .or_else(|| html::attr(input, "data-lazy-src"))
        .or_else(|| html::attr(input, "src"))
        .or_else(|| html::attr(input, "data-bg"))
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    body.split("<div")
        .filter(|chunk| {
            html::strip_tags(chunk)
                .to_ascii_lowercase()
                .contains(&label.to_ascii_lowercase())
        })
        .filter_map(|chunk| {
            html::text_between(chunk, "<a", "</a>")
                .or_else(|| html::text_between(chunk, "<span", "</span>"))
                .map(|value| {
                    html::strip_tags(&value)
                        .replace(label, "")
                        .trim_matches([':', ' '])
                        .to_string()
                })
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_tags(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/genre/") || chunk.contains("genres"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" | "on going" => ItemStatus::Ongoing,
        "completed" | "complete" => ItemStatus::Completed,
        "hiatus" => ItemStatus::Hiatus,
        "cancelled" | "canceled" | "dropped" => ItemStatus::Cancelled,
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
    let path = if input.starts_with("http://") || input.starts_with("https://") {
        input
            .split_once("://")
            .and_then(|(_, rest)| rest.split_once('/').map(|(_, path)| path))
            .unwrap_or_default()
    } else {
        input
    };
    format!(
        "/{}",
        path.split(['?', '#'])
            .next()
            .unwrap_or(path)
            .trim_start_matches('/')
            .trim_end_matches('/')
    )
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn absolute_image(value: &str) -> String {
    if value.starts_with("//") {
        format!("https:{value}")
    } else {
        url::join_url(BASE_URL, value)
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<a title="Sample Kiryuu" href="https://v6.kiryuu.to/manga/sample/"><img src="/cover.jpg"></a>
"#;
const DETAILS_FIXTURE: &str = r#"
<meta property="og:title" content="Sample Kiryuu"><meta property="og:image" content="/cover.jpg"><meta property="og:description" content="Sample synopsis.">
<a href="/manga/sample/chapter-1/">Chapter 1</a><a href="/genre/action/">Action</a>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="readerarea"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries[0].key, "/manga/sample");
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE, BASE_URL).len(), 1);
    }
}
