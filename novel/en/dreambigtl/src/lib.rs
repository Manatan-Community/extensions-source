use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage, NovelText, Paged,
    UrlResolveResult,
};
use manatan_shared::{html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: DreamBigTl = DreamBigTl;
const BASE_URL: &str = "https://www.dreambigtl.com";

struct DreamBigTl;

impl NovelSource for DreamBigTl {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if page > 1 {
            return Ok(Paged::default());
        }
        let latest = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .is_some_and(|listing| listing == "latest");
        let body =
            lnreader::fetch_document(BASE_URL, &absolute_url("p/disclaimer.html"), LIST_FIXTURE);
        let mut entries = parse_menu_novels(&body);
        if entries.is_empty() {
            entries = parse_fallback_novels(&body);
        }
        if latest {
            entries.sort_by(|a, b| b.key.cmp(&a.key));
        }
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
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
        let body = lnreader::fetch_document(
            BASE_URL,
            &format!("{BASE_URL}/search?q={}", url::query_escape(query)),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_search_results(&body),
            has_next_page: page == 1 && lnreader::has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "p/sample.html".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "p/sample.html".to_string());
        let body = lnreader::fetch_document(BASE_URL, &absolute_url(&key), DETAILS_FIXTURE);
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
            .unwrap_or_else(|| "2024/01/sample-chapter.html".to_string());
        let body = lnreader::fetch_document(BASE_URL, &absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Novels".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: list.entries,
            has_more: false,
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

fn parse_menu_novels(body: &str) -> Vec<CatalogItem> {
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    for category in ["New Novels", "Ongoing Novels", "Completed Novels"] {
        let Some(after_category) = body.split(category).nth(1) else {
            continue;
        };
        let menu = after_category
            .split("</ul>")
            .next()
            .unwrap_or(after_category);
        for chunk in menu.split("<a").skip(1) {
            let Some(href) = html::attr(chunk, "href") else {
                continue;
            };
            let key = lnreader::normalize_key(BASE_URL, &href);
            if key.contains("disclaimer") || !seen.insert(key.clone()) {
                continue;
            }
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
            entries.push(list_item(key, title, None));
        }
    }
    entries
}

fn parse_fallback_novels(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/p/") || href.contains("disclaimer") {
                return None;
            }
            let key = lnreader::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(list_item(key, title, None))
        })
        .collect()
}

fn parse_search_results(body: &str) -> Vec<CatalogItem> {
    body.split("blog-post")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "entry-title", "href")
                .or_else(|| html::attr_after(block, "<a", "href"))?;
            let key = lnreader::normalize_key(BASE_URL, &href);
            let title = lnreader::text_after_marker(block, "entry-title", "</")
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
            let cover = html::attr_after(block, "entry-image", "data-image");
            Some(list_item(key, title, cover))
        })
        .collect()
}

fn list_item(key: String, title: String, cover: Option<String>) -> CatalogItem {
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
    let body = lnreader::fetch_document(BASE_URL, &absolute_url(key), DETAILS_FIXTURE);
    let mut item = CatalogItem {
        key: lnreader::normalize_key(BASE_URL, key),
        title: lnreader::text_after_marker(&body, "entry-title", "</")
            .or_else(|| lnreader::text_between_tag(&body, "h1"))
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(&body, "post-body", "src").map(|image| absolute_url(&image)),
        description: html::text_between(&body, "<p", "</p>").map(|value| html::strip_tags(&value)),
        status: if key.to_ascii_lowercase().contains("completed") {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    };
    if let Some(author) = body
        .split("Author:")
        .nth(1)
        .map(|value| html::strip_tags(value).trim().to_string())
        .filter(|value| !value.is_empty())
    {
        item.authors = vec![author];
    }
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let mut chapters = Vec::new();
    for marker in ["chapter-panel", "List of Chapters"] {
        let Some(section) = body.split(marker).nth(1) else {
            continue;
        };
        for chunk in section.split("<a").skip(1) {
            let Some(href) = html::attr(chunk, "href") else {
                continue;
            };
            let key = lnreader::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            chapters.push(NovelChapter {
                key: key.clone(),
                title,
                chapter_number: chapter_number(&key),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            });
        }
        if !chapters.is_empty() {
            break;
        }
    }
    chapters.reverse();
    chapters
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let title = lnreader::text_after_marker(body, "entry-title", "</");
    let content = lnreader::html_after_marker(body, "post-body", "</div>")
        .unwrap_or_else(|| TEXT_FIXTURE.to_string());
    let normalized = novel::normalize_reader_html(&content);
    NovelText {
        title,
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn absolute_url(input: &str) -> String {
    lnreader::absolute_url(BASE_URL, input)
}

fn chapter_number(path: &str) -> Option<f32> {
    path.split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .filter(|part| !part.is_empty())
        .next_back()
        .and_then(|part| part.parse().ok())
}

const LIST_FIXTURE: &str = r#"
<ul class="sub-menu m-sub"><li><a href="https://www.dreambigtl.com/p/sample.html">Sample Novel</a></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample Novel</h1><div class="post-body"><img src="/cover.jpg"><p>Author: Sample Author</p><p>Sample summary.</p><h2>List of Chapters</h2><ul><li><a href="https://www.dreambigtl.com/2024/01/sample-chapter.html">Chapter 1</a></li></ul></div>
"#;

const TEXT_FIXTURE: &str =
    r#"<h1 class="entry-title">Chapter 1</h1><div class="post-body"><p>Sample text.</p></div>"#;

export_novel_source!(SOURCE);
