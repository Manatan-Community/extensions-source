use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, NovelChapter, NovelChapterPage, NovelText, Paged, UrlResolveResult,
};
use manatan_shared::{html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: DivineDaoLibrary = DivineDaoLibrary;
const BASE_URL: &str = "https://www.divinedaolibrary.com";

struct DivineDaoLibrary;

impl NovelSource for DivineDaoLibrary {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if page != 1 {
            return Ok(Paged::default());
        }
        let latest = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .is_some_and(|listing| listing == "latest");
        let keys = if latest {
            latest_novel_paths()
        } else {
            all_novel_paths(&request)
        };
        Ok(Paged {
            entries: keys.into_iter().map(|key| fetch_details(&key)).collect(),
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
        if page != 1 {
            return Ok(Paged::default());
        }
        let lower = query.to_ascii_lowercase();
        let entries = cached_novel_rows()
            .into_iter()
            .filter(|(_, title, _)| title.to_ascii_lowercase().contains(&lower))
            .map(|(_, _, key)| fetch_details(&key))
            .collect();
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        Ok(fetch_chapters(&key))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key =
            novel::request_key(&request, "chapter").unwrap_or_else(|| "sample-chapter".to_string());
        let api = format!(
            "{BASE_URL}/wp-json/wp/v2/posts?slug={}",
            url::query_escape(&key)
        );
        let json = lnreader::fetch_json(BASE_URL, &api, CHAPTER_JSON_FIXTURE);
        let chapter = json.get(0).unwrap_or(&Value::Null);
        let title = text_at(chapter, &["title", "rendered"]);
        let content =
            text_at(chapter, &["content", "rendered"]).unwrap_or_else(|| TEXT_FIXTURE.to_string());
        let html_body = format!(
            "{}{}",
            title
                .as_ref()
                .map(|title| format!("<h1>{title}</h1>"))
                .unwrap_or_default(),
            content
        );
        let normalized = novel::normalize_reader_html(&html_body);
        Ok(NovelText {
            title,
            html: Some(normalized.clone()),
            text: Some(novel::cleanup_text(&normalized)),
            base_url: Some(absolute_url(&key)),
            css: Some(
                "body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string(),
            ),
            image_headers: novel::image_headers(BASE_URL),
            ..NovelText::default()
        })
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "novels".to_string(),
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

fn all_novel_paths(request: &Value) -> Vec<String> {
    let selected = lnreader::filter_array(request, "category");
    cached_novel_rows()
        .into_iter()
        .filter(|(category, _, _)| {
            selected.is_empty() || selected.iter().any(|value| value == category)
        })
        .map(|(_, _, key)| key)
        .collect()
}

fn latest_novel_paths() -> Vec<String> {
    let body = lnreader::fetch_document(BASE_URL, BASE_URL, HOME_FIXTURE);
    let mut seen = BTreeSet::new();
    body.split("rel=\"category tag\"")
        .skip(1)
        .filter_map(|chunk| {
            html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))
        })
        .map(|href| lnreader::normalize_key(BASE_URL, &href))
        .filter(|key| seen.insert(key.clone()))
        .collect()
}

fn cached_novel_rows() -> Vec<(String, String, String)> {
    let body = lnreader::fetch_document(BASE_URL, &format!("{BASE_URL}/novels"), LIST_FIXTURE);
    let mut rows = Vec::new();
    let mut current_category = String::new();
    for segment in body.split("<").skip(1) {
        if segment.starts_with("h") {
            let text = html::strip_tags(&format!("<{segment}"));
            if !text.is_empty() {
                current_category = text;
            }
        }
        if segment.starts_with("a") {
            let Some(href) = html::attr(segment, "href") else {
                continue;
            };
            let key = lnreader::normalize_key(BASE_URL, &href);
            if key.is_empty() {
                continue;
            }
            let title = html::text_between(segment, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
            rows.push((current_category.clone(), title, key));
        }
    }
    if rows.is_empty() {
        vec![(
            "Completed".to_string(),
            "Sample Novel".to_string(),
            "sample".to_string(),
        )]
    } else {
        rows
    }
}

fn fetch_details(key: &str) -> CatalogItem {
    let api = format!(
        "{BASE_URL}/wp-json/wp/v2/pages?slug={}",
        url::query_escape(&lnreader::normalize_key(BASE_URL, key))
    );
    let json = lnreader::fetch_json(BASE_URL, &api, PAGE_JSON_FIXTURE);
    let page = json.get(0).unwrap_or(&Value::Null);
    let content = text_at(page, &["content", "rendered"]).unwrap_or_default();
    let excerpt = text_at(page, &["excerpt", "rendered"]).unwrap_or_default();
    CatalogItem {
        key: lnreader::normalize_key(BASE_URL, key),
        title: text_at(page, &["title", "rendered"])
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(&content, "<img", "data-lazy-src")
            .or_else(|| html::attr_after(&content, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(&excerpt, "<p", "</p>")
            .map(|value| html::strip_tags(&value)),
        authors: content
            .split("<h3")
            .nth(1)
            .and_then(|chunk| html::text_between(chunk, ">", "</h3>"))
            .map(|value| {
                html::strip_tags(&value)
                    .replace("Author:", "")
                    .trim()
                    .to_string()
            })
            .filter(|value| !value.is_empty())
            .into_iter()
            .collect(),
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapters(key: &str) -> Vec<NovelChapter> {
    let api = format!(
        "{BASE_URL}/wp-json/wp/v2/pages?slug={}",
        url::query_escape(&lnreader::normalize_key(BASE_URL, key))
    );
    let json = lnreader::fetch_json(BASE_URL, &api, PAGE_JSON_FIXTURE);
    let page = json.get(0).unwrap_or(&Value::Null);
    let content = text_at(page, &["content", "rendered"]).unwrap_or_default();
    let mut chapters: Vec<_> = content
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let chapter_key = lnreader::normalize_key(BASE_URL, &href);
            if chapter_key.is_empty() {
                return None;
            }
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            Some(NovelChapter {
                key: chapter_key.clone(),
                title,
                url: Some(absolute_url(&chapter_key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect();
    if let Some(last_key) = latest_published_chapter(key) {
        if let Some(index) = chapters.iter().position(|chapter| chapter.key == last_key) {
            chapters.truncate(index + 1);
        }
    }
    chapters
}

fn latest_published_chapter(key: &str) -> Option<String> {
    let category_api = format!(
        "{BASE_URL}/wp-json/wp/v2/categories?slug={}",
        url::query_escape(&lnreader::normalize_key(BASE_URL, key))
    );
    let categories = lnreader::fetch_json(BASE_URL, &category_api, "[]");
    let id = categories.get(0)?.get("id")?.as_u64()?;
    let posts_api = format!("{BASE_URL}/wp-json/wp/v2/posts?categories={id}&per_page=1");
    let posts = lnreader::fetch_json(BASE_URL, &posts_api, "[]");
    posts
        .get(0)
        .and_then(|post| post.get("slug"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn text_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str().map(ToString::to_string)
}

fn absolute_url(input: &str) -> String {
    lnreader::absolute_url(BASE_URL, input)
}

const HOME_FIXTURE: &str = r#"<main id="main"><a rel="category tag" href="https://www.divinedaolibrary.com/sample">Sample Novel</a></main>"#;
const LIST_FIXTURE: &str = r#"<h2>Completed</h2><ul><li><a href="https://www.divinedaolibrary.com/sample">Sample Novel</a></li></ul>"#;
const PAGE_JSON_FIXTURE: &str = r#"[{"title":{"rendered":"Sample Novel"},"content":{"rendered":"<h3>Author: Sample Author</h3><img src=\"/cover.jpg\"><li><span><a href=\"https://www.divinedaolibrary.com/sample-chapter\">Chapter 1</a></span></li>"},"excerpt":{"rendered":"<p>Sample summary.</p>"}}]"#;
const CHAPTER_JSON_FIXTURE: &str =
    r#"[{"title":{"rendered":"Chapter 1"},"content":{"rendered":"<p>Sample chapter text.</p>"}}]"#;
const TEXT_FIXTURE: &str = r#"<p>Sample chapter text.</p>"#;

export_novel_source!(SOURCE);
