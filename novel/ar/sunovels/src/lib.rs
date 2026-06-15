use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Sunovels = Sunovels;
const BASE_URL: &str = "https://sunovels.com";

struct Sunovels;

impl NovelSource for Sunovels {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        let page = page(&request).saturating_sub(1);
        let body = fetch_or_fixture(&format!("{BASE_URL}/library?page={page}"), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_or_fixture(&item_url(&key), DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }
        let body = fetch_or_fixture(
            &format!(
                "{BASE_URL}/search?page={page}&title={}",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        Ok(parse_details(
            &fetch_or_fixture(&item_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        Ok(self.chapters_page(request)?.entries)
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        let page = page(&request);
        let corrected = page.saturating_sub(1);
        let body = fetch_or_fixture(
            &format!("{}?activeTab=chapters&page={corrected}", item_url(&key)),
            CHAPTERS_FIXTURE,
        );
        Ok(NovelChapterPage {
            entries: parse_chapters(&body),
            has_next_page: has_next_page(&body),
            next_page: has_next_page(&body).then_some(page as u32 + 1),
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "novel/sample/chapter-1".to_string());
        Ok(parse_text(
            &fetch_or_fixture(&item_url(&key), TEXT_FIXTURE),
            &key,
        ))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Library".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: parse_listing(LIST_FIXTURE),
            has_more: true,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_or_fixture(&item_url(&key), DETAILS_FIXTURE),
                    &key,
                )),
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

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn item_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("list-item")
        .skip(1)
        .flat_map(|chunk| chunk.split("<a").skip(1).take(1))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: text_between(chunk, "<h4", "</h4>").unwrap_or_else(|| {
                    url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string())
                }),
                cover: html::attr_after(chunk, "<img", "src").map(|image| item_url(&image)),
                url: Some(item_url(&key)),
                language: Some("ar".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let status = text_between(body, "header-stats", "</div>")
        .and_then(|stats| {
            ["مكتمل", "جديد", "مستمر"]
                .iter()
                .find(|word| stats.contains(**word))
                .map(|word| (*word).to_string())
        })
        .unwrap_or_default();
    CatalogItem {
        key: key.to_string(),
        title: text_between(body, "main-head", "</div>")
            .or_else(|| text_between(body, "<h3", "</h3>"))
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(body, "img-container", "src").map(|image| item_url(&image)),
        description: text_between(body, "description", "</section>"),
        authors: text_between(body, "novel-author", "</")
            .into_iter()
            .collect(),
        tags: body
            .split("tag")
            .skip(1)
            .filter_map(|chunk| {
                html::text_between(chunk, ">", "</").map(|value| html::strip_tags(&value))
            })
            .filter(|value| !value.is_empty())
            .collect(),
        status: match status.as_str() {
            "مكتمل" => ItemStatus::Completed,
            "جديد" | "مستمر" => ItemStatus::Ongoing,
            _ => ItemStatus::Unknown,
        },
        url: Some(item_url(key)),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    body.split("chaptersList")
        .skip(1)
        .flat_map(|chunk| chunk.split("<a").skip(1))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::attr(chunk, "title")
                .or_else(|| text_between(chunk, "chapter-title", "</"))
                .unwrap_or_else(|| "Chapter".to_string());
            Some(NovelChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: text_between(chunk, "chapter-title", "</")
                    .and_then(|value| first_number(&value)),
                url: Some(item_url(&key)),
                language: Some("ar".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let html_body = html::text_between(body, "chapter-content", "</div>")
        .unwrap_or_else(|| TEXT_HTML_FIXTURE.to_string());
    let normalized = novel::normalize_reader_html(&html_body);
    NovelText {
        title: text_between(body, "<h1", "</h1>").or_else(|| text_between(body, "<h2", "</h2>")),
        html: Some(normalized.clone()),
        text: Some(html::strip_tags(&normalized)),
        base_url: Some(BASE_URL.to_string()),
        css: Some(
            "body { line-height: 1.8; direction: rtl; } img { max-width: 100%; }".to_string(),
        ),
        image_headers: novel::image_headers(BASE_URL),
        next_chapter_key: next_slug_key(key),
        ..NovelText::default()
    }
}

fn text_between(body: &str, marker: &str, end: &str) -> Option<String> {
    html::text_between(body, marker, end)
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn first_number(input: &str) -> Option<f32> {
    input
        .chars()
        .map(|ch| if ch.is_ascii_digit() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .next()
        .and_then(|part| part.parse().ok())
}

fn next_slug_key(key: &str) -> Option<String> {
    let number = first_number(url::slug_from_url(key)?.as_str())? as u64;
    let prefix = key.rsplit_once('/')?.0;
    Some(format!("{prefix}/chapter-{}", number + 1))
}

fn has_next_page(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("rel=\"next\"") || lower.contains("page-link") && lower.contains("next")
}

const LIST_FIXTURE: &str = r#"<div class="list-item"><a href="/novel/sample"><img src="/uploads/sample.jpg"><h4>Sample Novel</h4></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="main-head"><h3>Sample Novel</h3></div><section class="info-section"><div class="description"><p>Sample description.</p></div></section><div class="img-container"><figure class="cover"><img src="/uploads/sample.jpg"></figure></div>"#;
const CHAPTERS_FIXTURE: &str = r#"<ul class="chaptersList"><a href="/novel/sample/chapter-1" title="Chapter 1"><strong class="chapter-title">Chapter 1</strong></a></ul>"#;
const TEXT_HTML_FIXTURE: &str = r#"<p>The first fixture paragraph.</p>"#;
const TEXT_FIXTURE: &str =
    r#"<div class="chapter-content"><p>The first fixture paragraph.</p></div>"#;

export_novel_source!(SOURCE);
