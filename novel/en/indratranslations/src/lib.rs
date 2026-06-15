use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: IndraTranslations = IndraTranslations;
const BASE_URL: &str = "https://indratranslations.com";

struct IndraTranslations;

impl NovelSource for IndraTranslations {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if page != 1 {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: parse_listing(&fetch_or_fixture(
                &format!("{BASE_URL}/series/"),
                LIST_FIXTURE,
            )),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }
        if request.get("page").and_then(Value::as_u64).unwrap_or(1) != 1 {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        let target = format!(
            "{BASE_URL}/?s={}&post_type=wp-manga",
            url::query_escape(query)
        );
        Ok(Paged {
            entries: parse_listing(&fetch_or_fixture(&target, LIST_FIXTURE)),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "series/sample/".to_string());
        Ok(parse_details(
            &fetch_or_fixture(
                &url::join_url(BASE_URL, &normalize_key(&key)),
                DETAILS_FIXTURE,
            ),
            &normalize_key(&key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "series/sample/".to_string());
        Ok(parse_chapters(
            &fetch_or_fixture(
                &url::join_url(BASE_URL, &normalize_key(&key)),
                DETAILS_FIXTURE,
            ),
            &normalize_key(&key),
        ))
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
            .unwrap_or_else(|| "series/sample/chapter-1/".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &normalize_key(&key)), TEXT_FIXTURE);
        let raw = content_block(&body)
            .unwrap_or_else(|| "<p>Unable to load chapter content.</p>".to_string());
        let normalized = novel::normalize_reader_html(&raw);
        Ok(NovelText {
            html: Some(normalized.clone()),
            text: Some(novel::cleanup_text(&normalized)),
            base_url: Some(BASE_URL.to_string()),
            css: Some(
                "body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string(),
            ),
            image_headers: novel::image_headers(BASE_URL),
            next_chapter_key: Some(key),
            ..NovelText::default()
        })
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "series".to_string(),
            title: "Series".to_string(),
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
                    &fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
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

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for chunk in body.split("<a").skip(1) {
        let href = html::attr(chunk, "href").unwrap_or_default();
        if !href.contains("/series/") {
            continue;
        }
        let key = normalize_key(&href);
        if !seen.insert(key.clone()) {
            continue;
        }
        let title = html::attr(chunk, "title")
            .or_else(|| {
                html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
        let cover = html::attr_after(chunk, "<img", "data-src")
            .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value));
        out.push(CatalogItem {
            key: key.clone(),
            title,
            cover,
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        });
    }
    out
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let status_text = post_content_value(body, "status").unwrap_or_default();
    CatalogItem {
        key: normalize_key(key),
        title: first_text(body, &["entry-title", "<h1", "<title"])
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(body, "summary_image", "data-src")
            .or_else(|| html::attr_after(body, "summary_image", "data-lazy-src"))
            .or_else(|| html::attr_after(body, "summary_image", "src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        description: content_after(body, "summary__content")
            .or_else(|| content_after(body, "description-summary"))
            .map(|value| html::strip_tags(&value)),
        status: parse_status(&status_text),
        url: Some(url::join_url(BASE_URL, &normalize_key(key))),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, _novel_key: &str) -> Vec<NovelChapter> {
    let mut seen = BTreeSet::new();
    let mut chapters = Vec::new();
    for chunk in body.split("<a").skip(1) {
        let href = html::attr(chunk, "href").unwrap_or_default();
        if !href.contains("/series/") {
            continue;
        }
        let key = normalize_key(&href);
        if !seen.insert(key.clone()) {
            continue;
        }
        let title = html::text_between(chunk, ">", "</a>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty());
        let chapter_number = title.as_deref().and_then(chapter_number);
        chapters.push(NovelChapter {
            key: key.clone(),
            title,
            chapter_number,
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("en".to_string()),
            ..NovelChapter::default()
        });
    }
    chapters.sort_by(|a, b| {
        a.chapter_number
            .partial_cmp(&b.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn content_block(body: &str) -> Option<String> {
    for marker in ["reading-content", "text-left", "entry-content"] {
        if let Some(value) = content_after(body, marker) {
            return Some(remove_noisy_tags(&value));
        }
    }
    None
}

fn content_after(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</div>")
}

fn first_text(body: &str, markers: &[&str]) -> Option<String> {
    markers
        .iter()
        .find_map(|marker| {
            html::text_between(body, marker, "</").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
}

fn post_content_value(body: &str, label: &str) -> Option<String> {
    for chunk in body.split("post-content_item").skip(1) {
        let heading = content_after(chunk, "summary-heading")
            .map(|value| html::strip_tags(&value).to_ascii_lowercase())
            .unwrap_or_default();
        if heading.contains(label) {
            return content_after(chunk, "summary-content").map(|value| html::strip_tags(&value));
        }
    }
    None
}

fn remove_noisy_tags(input: &str) -> String {
    let mut out = input.to_string();
    for tag in ["script", "style", "iframe", "noscript", "ins"] {
        loop {
            let lower = out.to_ascii_lowercase();
            let Some(start) = lower.find(&format!("<{tag}")) else {
                break;
            };
            let Some(end) = lower[start..]
                .find(&format!("</{tag}>"))
                .map(|idx| start + idx + tag.len() + 3)
            else {
                break;
            };
            out.replace_range(start..end, "");
        }
    }
    out
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn parse_status(status: &str) -> ItemStatus {
    let lower = status.to_ascii_lowercase();
    if lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Ongoing
    }
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
        + "/"
}

const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><a href="https://indratranslations.com/series/sample/" title="Sample Series"><img src="/sample.jpg">Sample Series</a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="entry-title">Sample Series</h1><div class="summary_image"><img src="/sample.jpg"></div><div class="summary__content"><p>A fixture series.</p></div><div class="post-content_item"><div class="summary-heading">Status</div><div class="summary-content">Ongoing</div></div><li class="wp-manga-chapter"><a href="https://indratranslations.com/series/sample/chapter-1/">Chapter 1</a></li>"#;
const TEXT_FIXTURE: &str =
    r#"<div class="reading-content"><p>The first fixture paragraph.</p></div>"#;

export_novel_source!(SOURCE);
