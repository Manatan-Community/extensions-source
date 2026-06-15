use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage, NovelText, Paged,
    UrlResolveResult,
};
use manatan_shared::{html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: NovelHall = NovelHall;
const BASE_URL: &str = "https://novelhall.com";

struct NovelHall;

impl NovelSource for NovelHall {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch(&format!("{BASE_URL}/all2022-{page}.html"), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: !parse_listing(&body).is_empty(),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = lnreader::key_from_url(BASE_URL, query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        let body = fetch(
            &format!(
                "{BASE_URL}/index.php?s=so&module=book&keyword={}",
                url::query_escape(query)
            ),
            SEARCH_FIXTURE,
        );
        Ok(Paged {
            entries: parse_search(&body),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "sample.html".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "sample.html".to_string());
        let body = fetch(&absolute_url(&key), DETAILS_FIXTURE);
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
            .unwrap_or_else(|| "sample/chapter-1.html".to_string());
        let body = fetch(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "all".to_string(),
            title: "All Novels".to_string(),
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
        if let Some(key) = lnreader::key_from_url(BASE_URL, input) {
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

fn fetch(target: &str, fixture: &str) -> String {
    lnreader::fetch_document(BASE_URL, target, fixture)
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("li class=\"btm")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")?;
            let key = lnreader::normalize_key(BASE_URL, &href);
            let title = html::text_between(block, ">", "</li>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
            Some(item(key, title, None))
        })
        .collect()
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    body.split("<tr")
        .skip(1)
        .filter_map(|row| {
            let href = html::attr_after(row, "<a", "href")?;
            let key = lnreader::normalize_key(BASE_URL, &href);
            let title = html::text_between(row, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
            Some(item(key, title, None))
        })
        .collect()
}

fn item(key: String, title: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch(&absolute_url(key), DETAILS_FIXTURE);
    let mut item = CatalogItem {
        key: lnreader::normalize_key(BASE_URL, key),
        title: lnreader::text_after_marker(&body, "book-info", "</h1>")
            .or_else(|| lnreader::text_between_tag(&body, "h1"))
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(&body, "property=\"og:image\"", "content")
            .or_else(|| html::attr_after(&body, "property='og:image'", "content"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(&body, "class=\"intro", "</")
            .or_else(|| html::text_between(&body, "class='intro", "</"))
            .map(|value| html::strip_tags(&value)),
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    };
    let total = body.split("class=\"total").nth(1).unwrap_or(&body);
    if let Some(author) = label_value(total, "Author") {
        item.authors = vec![author];
    }
    item.status = label_value(total, "Status")
        .map(|status| status.replace("Active", "Ongoing"))
        .map(|status| parse_status(&status))
        .unwrap_or(ItemStatus::Unknown);
    item.tags = total
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect();
    item
}

fn label_value(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| {
            html::text_between(chunk, ">", "</span>").or_else(|| Some(chunk.to_string()))
        })
        .map(|value| html::strip_tags(&value).replace(['：', ':'], ""))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    body.split("id=\"morelist")
        .nth(1)
        .unwrap_or(body)
        .split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = lnreader::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            Some(NovelChapter {
                key: key.clone(),
                title,
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let content = lnreader::html_after_marker(body, "id=\"htmlContent\"", "</div>")
        .or_else(|| lnreader::html_after_marker(body, "id='htmlContent'", "</div>"))
        .unwrap_or_else(|| TEXT_FIXTURE.to_string());
    let normalized = novel::normalize_reader_html(&content);
    NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else if lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn absolute_url(input: &str) -> String {
    lnreader::absolute_url(BASE_URL, input)
}

const LIST_FIXTURE: &str = r#"<li class="btm"><a href="sample.html">Sample Novel</a></li>"#;
const SEARCH_FIXTURE: &str =
    r#"<table><tr><td></td><td><a href="sample.html">Sample Novel</a></td></tr></table>"#;
const DETAILS_FIXTURE: &str = r#"
<div class="book-info"><h1>Sample Novel</h1></div><meta property="og:image" content="/cover.jpg"><div class="intro">Sample summary.</div><div class="total"><span>Author：Sample Author</span><span>Status：Active</span><a>Fantasy</a></div><div id="morelist"><ul><li><a href="sample/chapter-1.html">Chapter 1</a></li></ul></div>
"#;
const TEXT_FIXTURE: &str = r#"<div id="htmlContent"><p>Sample text.</p></div>"#;

export_novel_source!(SOURCE);
