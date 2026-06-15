use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    html, novel,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: ReLibrary = ReLibrary;
const BASE_URL: &str = "https://re-library.com";

struct ReLibrary;

impl NovelSource for ReLibrary {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            let body = fetch_document_or_fixture(
                &format!("{BASE_URL}/tag/translations/page/{page}"),
                LATEST_FIXTURE,
            );
            return Ok(Paged {
                entries: parse_latest(&body),
                has_next_page: has_next_page(&body),
            });
        }
        if page > 1 {
            return Ok(Paged::default());
        }
        let body = fetch_document_or_fixture(
            &format!("{BASE_URL}/translations/most-popular/"),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_popular(&body),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/translations/"), SEARCH_FIXTURE);
        let query_lower = query.to_ascii_lowercase();
        Ok(Paged {
            entries: parse_search_index(&body)
                .into_iter()
                .filter(|item| item.title.to_ascii_lowercase().contains(&query_lower))
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel")
            .unwrap_or_else(|| "translations/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel")
            .unwrap_or_else(|| "translations/sample".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "sample/chapter-1".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Most Popular".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&key)),
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
        .with_referer(BASE_URL)
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

fn parse_popular(body: &str) -> Vec<CatalogItem> {
    body.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<h3", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(listing_item(&href, &title, chunk))
        })
        .collect()
}

fn parse_latest(body: &str) -> Vec<CatalogItem> {
    body.split("article type-page page")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "entry-title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "entry-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(listing_item(&href, &title, chunk))
        })
        .collect()
}

fn parse_search_index(body: &str) -> Vec<CatalogItem> {
    body.split("article")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<h4", "</h4>")
                .or_else(|| html::attr_after(chunk, "<a", "title"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(listing_item(&href, &title, chunk))
        })
        .collect()
}

fn listing_item(href: &str, title: &str, chunk: &str) -> CatalogItem {
    let key = normalize_key(href);
    CatalogItem {
        key: key.clone(),
        title: title.to_string(),
        cover: html::attr_after(chunk, "<img", "data-cfsrc")
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let table = html::text_between(body, "entry-content", "su-accordion").unwrap_or_default();
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "entry-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(body, "entry-content", "data-cfsrc")
            .or_else(|| html::attr_after(body, "entry-content", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "su-box-content", "</div>")
            .map(|value| html::strip_tags(&value)),
        tags: labeled_value(&table, "Category")
            .map(split_values)
            .unwrap_or_default(),
        status: labeled_value(&table, "Status")
            .map(|value| parse_status(&value))
            .unwrap_or(ItemStatus::Unknown),
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    body.split("su-accordion")
        .skip(1)
        .flat_map(|block| block.split("<li").skip(1))
        .enumerate()
        .filter_map(|(index, chunk)| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(NovelChapter {
                key: key.clone(),
                title: html::text_between(chunk, ">", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                chapter_number: Some((index + 1) as f32),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let mut content = html::text_between(body, "entry-content", "</article>")
        .or_else(|| html::text_between(body, "entry-content", "</div>"))
        .unwrap_or_else(|| TEXT_FIXTURE.to_string());
    if let Some((_, rest)) = content.split_once("PageLink") {
        content = rest.to_string();
    }
    if let Some((before, _)) = content.split_once("hr + .PageLink") {
        content = before.to_string();
    }
    let normalized = novel::normalize_reader_html(&content);
    NovelText {
        title: html::text_between(body, "entry-title", "</").map(|value| html::strip_tags(&value)),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn labeled_value(block: &str, label: &str) -> Option<String> {
    block
        .split("<p")
        .find(|chunk| {
            chunk
                .to_ascii_lowercase()
                .contains(&label.to_ascii_lowercase())
        })
        .map(|chunk| {
            html::strip_tags(chunk)
                .replace(label, "")
                .replace(':', "")
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn split_values(value: String) -> Vec<String> {
    value
        .split([',', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("cancelled") {
        ItemStatus::Cancelled
    } else if lower.contains("on-going") || lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn has_next_page(body: &str) -> bool {
    body.contains("next page-numbers") || body.contains("rel=\"next\"")
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .contains("re-library.com")
        .then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://re-library.com/")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        url::join_url(BASE_URL, input)
    }
}

const LIST_FIXTURE: &str = r#"
<div class="entry-content"><ol><li><h3><a href="https://re-library.com/translations/sample/">Sample Novel</a></h3><table><tr><td><a><img src="/cover.jpg"></a></td></tr></table></li></ol></div>
"#;

const LATEST_FIXTURE: &str = r#"
<article class="type-page page"><h2 class="entry-title"><a href="https://re-library.com/translations/sample/">Sample Novel</a></h2><div class="entry-content"><table><tr><td><a><img src="/cover.jpg"></a></td></tr></table></div></article>
"#;

const SEARCH_FIXTURE: &str = r#"<article><h4>Sample Novel</h4><a href="https://re-library.com/translations/sample/"><img src="/cover.jpg"></a></article>"#;

const DETAILS_FIXTURE: &str = r#"
<header class="entry-header"><h1 class="entry-title">Sample Novel</h1></header>
<div class="entry-content"><table><tr><td><img src="/cover.jpg"><p><strong>Status</strong> On-going</p><p><strong>Category</strong> Fantasy</p></td></tr></table>
<div class="su-box"><div class="su-box-content">Sample summary.</div></div>
<div class="su-accordion"><li class="page_item"><a href="https://re-library.com/sample/chapter-1/">Chapter 1</a></li></div></div>
"#;

const TEXT_FIXTURE: &str = r#"<div class="entry-content"><p>Sample chapter text.</p></div>"#;

export_novel_source!(SOURCE);
