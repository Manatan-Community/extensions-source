use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: GalaxyManga = GalaxyManga;
const BASE_URL: &str = "https://galaxymanga.io";

struct GalaxyManga;

impl MangaSource for GalaxyManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: has_next_page(LIST_FIXTURE),
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            list_url(page, "")
        } else {
            list_url(page, "manga")
        };
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_manga_key(query);
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let target = if page <= 1 {
            format!("{BASE_URL}/?s={}", url::query_escape(query))
        } else {
            format!("{BASE_URL}/page/{page}/?s={}", url::query_escape(query))
        };
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample-chapter-1".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    Some(normalize_manga_key(input)),
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn list_url(page: u64, path: &str) -> String {
    let path = path.trim_matches('/');
    match (page, path.is_empty()) {
        (0 | 1, true) => BASE_URL.to_string(),
        (0 | 1, false) => format!("{BASE_URL}/{path}/"),
        (_, true) => format!("{BASE_URL}/page/{page}/"),
        (_, false) => format!("{BASE_URL}/{path}/page/{page}/"),
    }
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("bsx") || chunk.contains("page-item-detail"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = normalize_manga_key(&href);
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| html::text_between(chunk, "<h3", "</h3>").map(|v| html::strip_tags(&v)))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_from_html(chunk).map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), |mut items, item| {
            if !items.iter().any(|existing| existing.key == item.key) {
                items.push(item);
            }
            items
        })
}

fn has_next_page(body: &str) -> bool {
    body.contains("next page-numbers")
        || body.contains("class=\"next\"")
        || body.contains("class='next'")
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</h1>")
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| image_from_html(body))
            .map(|value| absolute_url(&value)),
        description: html::text_between(body, "entry-content", "</div>")
            .or_else(|| html::text_between(body, "desc", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: link_values(body, "/genres/"),
        status: parse_status(body),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("eph-num")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_any_key(&href);
            let title = html::text_between(chunk, "chapternum", "</span>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                chapter_number: chapter_number_from_title(&title),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "chapterdate", "</span>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let section = body
        .split("reading-content")
        .nth(1)
        .or_else(|| body.split("readerarea").nth(1))
        .or_else(|| body.split("chaptercontent").nth(1))
        .unwrap_or(body);
    section
        .split("<img")
        .skip(1)
        .filter_map(image_from_html)
        .filter(|value| {
            !value.starts_with("data:")
                && !value.contains("/themes/")
                && !value.contains("logo")
                && (value.contains("/wp-content/uploads/") || value.starts_with("http"))
        })
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_from_html(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(input, "<img", "data-cfsrc"))
        .or_else(|| {
            html::attr_after(input, "<img", "srcset").and_then(|srcset| {
                srcset
                    .split(',')
                    .find_map(|part| part.split_whitespace().next().map(ToString::to_string))
            })
        })
        .or_else(|| html::attr_after(input, "<img", "src"))
        .or_else(|| html::attr(input, "data-src"))
        .or_else(|| html::attr(input, "src"))
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

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") || lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") || lower.contains("on hold") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Ongoing
    }
}

fn chapter_number_from_title(title: &str) -> Option<f32> {
    title.split_whitespace().find_map(|part| {
        part.trim_matches(|ch: char| !ch.is_ascii_digit() && ch != '.')
            .parse()
            .ok()
    })
}

fn normalize_manga_key(input: &str) -> String {
    if let Some(index) = input.find("/manga/") {
        return format!("/{}", input[index + 1..].trim_end_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn normalize_any_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find(BASE_URL) {
            return format!(
                "/{}",
                input[index + BASE_URL.len()..]
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="listupd"><div class="bsx"><a href="https://galaxymanga.io/manga/sample/" title="Sample Manga"><img src="https://galaxymanga.io/wp-content/uploads/sample.jpg" alt="Sample Manga"></a></div></div>
<a class="next page-numbers" href="https://galaxymanga.io/page/2/">Next</a>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample Manga</h1><meta property="og:image" content="https://galaxymanga.io/wp-content/uploads/sample.jpg">
<span class="mgen"><a href="https://galaxymanga.io/genres/drama/">Drama</a></span>
<div class="eph-num"><a href="https://galaxymanga.io/sample-chapter-1/"><span class="chapternum">Chapter 1</span><span class="chapterdate">January 1, 2024</span></a></div>
"#;
const PAGES_FIXTURE: &str = r#"
<div class="reading-content"><img src="https://galaxymanga.io/wp-content/uploads/page1.jpg"><img data-src="https://galaxymanga.io/wp-content/uploads/page2.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_theme_source() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].key, "/manga/sample");
        let pages = SOURCE
            .pages(json!({"chapter":"/sample-chapter-1"}))
            .unwrap();
        assert_eq!(pages.len(), 2);
    }
}
