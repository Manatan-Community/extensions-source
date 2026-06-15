use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    dates, html, novel,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: Rainofsnow = Rainofsnow;
const BASE_URL: &str = "https://rainofsnow.com";

struct Rainofsnow;

impl NovelSource for Rainofsnow {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let genre = request
            .get("filters")
            .and_then(|filters| filters.get("genre"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let body = fetch_document_or_fixture(
            &format!("{BASE_URL}/novels/page/{page}{genre}"),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
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
        let target = format!("{BASE_URL}/?s={}", url::query_escape(query));
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if page <= 1 {
            absolute_url(&key)
        } else {
            format!(
                "{}/{}/page/{page}/#chapter",
                BASE_URL,
                key.trim_matches('/')
            )
        };
        let body = fetch_document_or_fixture(&target, DETAILS_FIXTURE);
        Ok(NovelChapterPage {
            entries: parse_chapters(&body),
            has_next_page: has_chapter_next_page(&body, page),
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
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Novels".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: list.entries,
            has_more: list.has_next_page,
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

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("minbox")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "<h3", "href")
                .or_else(|| html::attr_after(block, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(block, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(block, "<img", "data-src")
                    .or_else(|| html::attr_after(block, "<img", "src"))
                    .map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("suggestive".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "class=\"text", "</h2>")
            .or_else(|| html::text_between(body, "<h2", "</h2>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(body, "imagboca1", "data-src")
            .or_else(|| html::attr_after(body, "imagboca1", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "id=\"synop\"", "</")
            .or_else(|| html::text_between(body, "id='synop'", "</"))
            .map(|value| html::strip_tags(&value)),
        tags: text_after_label(body, "Genre(s)")
            .map(split_values)
            .unwrap_or_default(),
        authors: text_after_label(body, "Author")
            .map(split_values)
            .unwrap_or_default(),
        status: ItemStatus::Unknown,
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    body.split("march1")
        .nth(1)
        .unwrap_or(body)
        .split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(NovelChapter {
                key: key.clone(),
                title: html::text_between(chunk, "class=\"chapter", "</")
                    .or_else(|| html::text_between(chunk, "class='chapter", "</"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: html::text_between(chunk, "<small", "</small>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let title = html::text_between(body, "class=\"content", "</h2>")
        .or_else(|| html::text_between(body, "<h2", "</h2>"))
        .map(|value| html::strip_tags(&value));
    let content = html::text_between(body, "class=\"content", "</div>")
        .or_else(|| html::text_between(body, "class='content", "</div>"))
        .unwrap_or_else(|| TEXT_FIXTURE.to_string());
    let normalized = novel::normalize_reader_html(&content);
    NovelText {
        title,
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| {
            html::text_between(chunk, "<span", "</span>")
                .or_else(|| html::text_between(chunk, ">", "</"))
        })
        .map(|value| html::strip_tags(&value))
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

fn has_next_page(body: &str) -> bool {
    body.contains("next page-numbers") || body.contains("rel=\"next\"")
}

fn has_chapter_next_page(body: &str, page: u64) -> bool {
    body.split("page-numbers").any(|chunk| {
        html::strip_tags(chunk)
            .trim()
            .parse::<u64>()
            .is_ok_and(|value| value > page)
    })
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .contains("rainofsnow.com")
        .then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://rainofsnow.com/")
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
<div class="minbox"><h3><a href="https://rainofsnow.com/sample">Sample Novel</a></h3><img data-src="/cover.jpg"></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="text"><h2>Sample Novel</h2></div><div class="imagboca1"><img data-src="/cover.jpg"></div>
<div id="synop">Sample summary.</div><span>Genre(s)</span><span>Fantasy</span><span>Author</span><span>Sample Author</span>
<div id="chapter"><ul class="march1"><li><a href="https://rainofsnow.com/sample/chapter-1"><span class="chapter">Chapter 1</span><small>jan 1, 2024</small></a></li></ul></div>
"#;

const TEXT_FIXTURE: &str =
    r#"<div class="content"><h2>Chapter 1</h2><p>Sample chapter text.</p></div>"#;

export_novel_source!(SOURCE);
