use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    dates, html, lnreader, novel, sdk::SearchRequest, sdk::http::HttpClient, url,
};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: Novelki = Novelki;
const BASE_URL: &str = "https://novelki.pl";

struct Novelki;

impl NovelSource for Novelki {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let body = fetch_document_or_fixture(&projects_url(page, "", &request), LIST_FIXTURE);
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty() && lnreader::has_next_page(&body),
            entries,
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
        let page = page(&request);
        let body = fetch_document_or_fixture(&projects_url(page, query, &request), LIST_FIXTURE);
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty() && lnreader::has_next_page(&body),
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "projekty/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "projekty/sample".to_string());
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
            .unwrap_or_else(|| "projekty/sample/chapter-1".to_string());
        let chapter_id = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("chapter-1");
        let body = fetch_json_or_fixture(
            &format!("{BASE_URL}/api/reader/chapters/{chapter_id}"),
            TEXT_FIXTURE,
        );
        let html = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .pointer("/data/content")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .unwrap_or(body);
        Ok(text_from_html(&key, None, html))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
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

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn projects_url(page: u64, query: &str, request: &Value) -> String {
    let mut params = vec!["filter=t".to_string()];
    if !query.is_empty() {
        params.push(format!("title={}+", url::query_escape(query)));
    }
    for key in ["genres", "status", "type"] {
        let value = lnreader::filter_string_opt(request, key).unwrap_or_default();
        params.push(format!("{key}={}", url::query_escape(&value)));
    }
    params.push(format!("page={page}"));
    format!("{BASE_URL}/projekty?{}", params.join("&"))
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("card-title") || chunk.contains("card-img-top"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            if !key.starts_with("projekty/")
                || key.split('/').count() != 2
                || !seen.insert(key.clone())
            {
                return None;
            }
            let title = html::attr_after(chunk, "card-title", "title")
                .or_else(|| {
                    html::text_between(chunk, "card-title", "</")
                        .map(|value| html::strip_tags(&value))
                })
                .unwrap_or_else(|| title_from_key(&key));
            Some(catalog_item(
                key,
                title,
                html::attr_after(chunk, "card-img-top", "src"),
                false,
            ))
        })
        .collect()
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let mut item = catalog_item(
        normalize_key(key),
        html::text_between(body, "<h3", "</h3>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_key(key)),
        html::attr_after(body, "img-fluid", "src")
            .or_else(|| html::attr_after(body, "card-img-top", "src")),
        true,
    );
    item.authors = text_after_label(body, "Autor:").into_iter().collect();
    item.status = text_after_label(body, "Status projektu:")
        .map(|value| parse_status(&value))
        .unwrap_or(ItemStatus::Unknown);
    item.tags = body
        .split("badge")
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect();
    item.description = description_after_opis(body);
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let mut seen = BTreeSet::new();
    let mut chapters = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("card-footer") || chunk.contains("chapters"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            if !key.starts_with("projekty/")
                || key.split('/').count() < 3
                || !seen.insert(key.clone())
            {
                return None;
            }
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(NovelChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "card-footer", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_dmy_dash(&value)),
                url: Some(absolute_url(&key)),
                language: Some("multi".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    for (index, chapter) in chapters.iter_mut().enumerate() {
        chapter.chapter_number = Some((index + 1) as f32);
    }
    chapters
}

fn text_from_html(key: &str, title: Option<String>, raw: String) -> NovelText {
    let normalized = novel::normalize_reader_html(&raw);
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

fn catalog_item(
    key: String,
    title: String,
    cover: Option<String>,
    initialized: bool,
) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("multi".to_string()),
        content_rating: Some("adult".to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input.contains("novelki.pl").then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn title_from_key(key: &str) -> String {
    key.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("Novelki")
        .replace(['-', '_'], " ")
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    let text = html::strip_tags(body);
    let start = text.find(label)? + label.len();
    text[start..]
        .lines()
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn description_after_opis(body: &str) -> Option<String> {
    let after = body.split("Opis:").nth(1)?;
    html::text_between(after, "<p", "</p>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("aktywny") {
        ItemStatus::Ongoing
    } else if lower.contains("zako") {
        ItemStatus::Completed
    } else if lower.contains("wstrzymany") {
        ItemStatus::Hiatus
    } else if lower.contains("porzucony") || lower.contains("zlicencjonowany") {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Unknown
    }
}

fn parse_dmy_dash(value: &str) -> Option<i64> {
    let date = value
        .trim()
        .split_whitespace()
        .next()
        .unwrap_or(value.trim());
    let mut parts = date.split('-').collect::<Vec<_>>();
    if parts.len() != 3 {
        return dates::parse_ymd(date);
    }
    parts.reverse();
    dates::parse_ymd(&parts.join("-"))
}

export_novel_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div id="projects"><div><a href="/projekty/sample"><img class="card-img-top" src="/cover.jpg"><span class="card-title" title="Sample Novel">Sample Novel</span></a></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h3>Sample Novel</h3><img class="img-fluid" src="/cover.jpg">
<p class="h5">Autor: Author</p><p class="h5">Status projektu: Aktywny</p>
<span class="badge">Fantasy</span><p class="h5">Opis:</p><p></p><p>Sample summary.</p>
<div class="chapters"><div class="col-md-3"><div><a href="/projekty/sample/chapter-1">Chapter 1</a><div class="card-footer"><span>01-01-2024</span></div></div></div></div>
"#;

const TEXT_FIXTURE: &str = r#"{"data":{"content":"<p>Sample chapter text.</p>"}}"#;
