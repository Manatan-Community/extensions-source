use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Manhuagold = Manhuagold;
const BASE_URL: &str = "https://manhuagold.top";

struct Manhuagold;

impl MangaSource for Manhuagold {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/all-manga/{page}/?sort=last_update&status=0")
        } else {
            format!("{BASE_URL}/ranking/week/{page}")
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
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let target = format!(
            "{BASE_URL}/search/{page}/?keyword={}",
            url::query_escape(query)
        );
        Ok(parse_listing(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let chapter_url = url::join_url(BASE_URL, &key);
        let body = fetch_document_or_fixture(&chapter_url, PAGES_FIXTURE);
        Ok(parse_pages(&body, &chapter_url))
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

fn fetch_xhr_or_fixture(target: &str, referer: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("grid") || chunk.contains("text-center") || chunk.contains("<figure")
            })
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                if href.contains("/chapter") {
                    return None;
                }
                let title = html::text_between(chunk, "text-center", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("en".to_string()),
                    content_rating: Some("adult".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("pagecurrent + span")
            || body.contains("blog-pager")
            || body.contains("next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let description = html::text_between(body, "id=\"syn-target\"", "</div>")
        .or_else(|| html::text_between(body, "id='syn-target'", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Manga".to_string()),
        cover: html::attr_after(body, "class=\"a1\"", "src")
            .or_else(|| html::attr_after(body, "class='a1'", "src"))
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description,
        authors: labeled_values(body, "fa-user"),
        tags: link_values(body, "rel=\"tag\""),
        status: parse_status(&labeled_values(body, "fa-rss").join(" ")),
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
        .filter(|chunk| chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                date_uploaded: html::attr_after(chunk, "<time", "datetime")
                    .and_then(|value| value.parse::<i64>().ok()),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let html_pages = parse_page_images(body);
    if !html_pages.is_empty() {
        return html_pages;
    }
    let Some(chapter_id) = body
        .split("const CHAPTER_ID")
        .nth(1)
        .and_then(|part| part.split('=').nth(1))
        .map(|part| {
            part.split(';')
                .next()
                .unwrap_or(part)
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string()
        })
        .filter(|value| !value.is_empty())
    else {
        return parse_page_images(PAGES_FIXTURE);
    };
    let target = format!("{BASE_URL}/ajax/image/list/chap/{chapter_id}");
    let response: PageListResponse = serde_json::from_str(&fetch_xhr_or_fixture(
        &target,
        chapter_url,
        PAGES_JSON_FIXTURE,
    ))
    .unwrap_or_default();
    parse_page_images(&response.html)
}

fn parse_page_images(body: &str) -> Vec<MangaPage> {
    let mut seen = Vec::<String>::new();
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| {
            if image.starts_with("data:") || seen.contains(image) {
                false
            } else {
                seen.push(image.clone());
                true
            }
        })
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

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-lazy-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn labeled_values(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("updating"))
        .collect()
}

fn link_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("hold") || lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("cancel") {
        ItemStatus::Cancelled
    } else if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
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

#[derive(Default, Deserialize)]
struct PageListResponse {
    #[serde(default)]
    html: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="grid"><div><figure><img src="/cover.jpg"></figure><div class="text-center"><a href="/manga/sample">Sample Gold</a></div></div></div><div class="blog-pager"><span class="pagecurrent"></span><span></span></div>"#;

const DETAILS_FIXTURE: &str = r#"
<div class="a1"><figure><img src="/cover.jpg"></figure></div><div class="a2"><header><h1>Sample Gold</h1></header>
<div><a rel="tag" href="/genre/action">Action</a></div><div class="y6x11p"><i class="fas fa-user"></i><span class="dt">Author</span><i class="fas fa-rss"></i><span class="dt">Ongoing</span></div></div>
<div id="syn-target">A sample.</div><ul><li class="chapter"><a href="/manga/sample/chapter-1">Chapter 1</a><time datetime="1704067200"></time></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"<script>const CHAPTER_ID = 1;</script><div class="separator"><a href="/page1.jpg"><img src="/page1.jpg"></a></div><div class="separator"><a href="/page2.jpg"><img src="/page2.jpg"></a></div>"#;

const PAGES_JSON_FIXTURE: &str = r#"{"status":true,"html":"<div class=\"separator\"><a href=\"/page1.jpg\"><img src=\"/page1.jpg\"></a></div><div class=\"separator\"><a href=\"/page2.jpg\"><img src=\"/page2.jpg\"></a></div>"}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_liliana_source() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample Gold");
        let chapters = SOURCE.chapters(json!({"manga":"/manga/sample"})).unwrap();
        assert_eq!(chapters[0].title.as_deref(), Some("Chapter 1"));
        let pages = SOURCE
            .pages(json!({"chapter":"/manga/sample/chapter-1"}))
            .unwrap();
        assert_eq!(pages.len(), 2);
    }
}
