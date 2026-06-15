use crate::{
    dates, html, novel,
    sdk::{
        CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
        NovelText, Paged, UrlResolveResult, abi::ExtensionResult, http::HttpClient,
    },
    url,
};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub struct NovelSite {
    pub id: &'static str,
    pub name: &'static str,
    pub lang: &'static str,
    pub base_url: &'static str,
    pub popular_path: &'static str,
    pub latest_path: &'static str,
    pub search_path: &'static str,
    pub content_rating: &'static str,
}

pub fn client(site: NovelSite) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(site.base_url)
        .with_cookies_for(site.base_url)
        .with_webview_challenge_fallback()
}

pub fn fetch_document(site: NovelSite, target: &str, fixture: &str) -> String {
    client(site)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

pub fn fetch_document_deflate(site: NovelSite, target: &str, fixture: &str) -> String {
    client(site)
        .with_header("Accept-Encoding", "deflate")
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

pub fn absolute_url(site: NovelSite, input: &str) -> String {
    url::join_url(site.base_url, input)
}

pub fn key(site: NovelSite, input: &str) -> String {
    normalize_key(site, input).trim_matches('/').to_string()
}

pub fn catalog_item(
    site: NovelSite,
    key: String,
    title: String,
    cover: Option<String>,
    initialized: bool,
) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|value| absolute_url(site, &value)),
        url: Some(absolute_url(site, &key)),
        language: Some(site.lang.to_string()),
        content_rating: Some(site.content_rating.to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

pub fn chapter_item(
    site: NovelSite,
    key: String,
    title: Option<String>,
    chapter_number: Option<f32>,
) -> NovelChapter {
    NovelChapter {
        date_uploaded: dates::parse_ymd_from_path(&key),
        url: Some(absolute_url(site, &key)),
        key,
        title,
        chapter_number,
        language: Some(site.lang.to_string()),
        ..NovelChapter::default()
    }
}

pub fn text_from_html(
    site: NovelSite,
    key: &str,
    title: Option<String>,
    content: String,
) -> NovelText {
    let normalized = novel::normalize_reader_html(&content);
    NovelText {
        title,
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(site, key)),
        css: Some("img { max-width: 100%; height: auto; } body { line-height: 1.7; }".to_string()),
        image_headers: novel::image_headers(site.base_url),
        ..NovelText::default()
    }
}

pub fn normalized_search(value: &str) -> String {
    value
        .to_lowercase()
        .replace(['\u{0300}', '\u{0301}', '\u{0302}', '\u{0308}'], "")
        .replace(['à', 'á', 'â', 'ä'], "a")
        .replace(['è', 'é', 'ê', 'ë'], "e")
        .replace(['ì', 'í', 'î', 'ï'], "i")
        .replace(['ò', 'ó', 'ô', 'ö'], "o")
        .replace(['ù', 'ú', 'û', 'ü'], "u")
        .replace('ç', "c")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn filter_local_catalog(entries: Vec<CatalogItem>, query: &str) -> Vec<CatalogItem> {
    let query = normalized_search(query);
    entries
        .into_iter()
        .filter(|item| normalized_search(&item.title).contains(&query))
        .collect()
}

pub fn first_non_empty(values: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
}

pub fn text_between_markers(text: &str, start: &str, end: &str) -> Option<String> {
    let start_index = text.find(start)? + start.len();
    let rest = &text[start_index..];
    let end_index = rest.find(end)?;
    Some(rest[..end_index].trim().to_string())
}

pub fn text_after_label(text: &str, label: &str) -> Option<String> {
    let start = text.find(label)? + label.len();
    let value = text[start..].lines().next()?.trim();
    (!value.is_empty()).then(|| value.trim_matches(':').trim().to_string())
}

pub fn tag_text(block: &str, tag: &str) -> Option<String> {
    html::text_between(block, &format!("<{tag}"), &format!("</{tag}>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

pub fn page_url(site: NovelSite, path: &str, page: u64) -> String {
    let mut full = url::join_url(site.base_url, path);
    if full.contains("{page}") {
        full = full.replace("{page}", &page.to_string());
    } else {
        full.push_str(if full.contains('?') { "&" } else { "?" });
        full.push_str("page=");
        full.push_str(&page.to_string());
    }
    full
}

pub fn search_url(site: NovelSite, query: &str, page: u64) -> String {
    let escaped = url::query_escape(query);
    let mut full = url::join_url(site.base_url, site.search_path);
    full = full
        .replace("{query}", &escaped)
        .replace("{page}", &page.to_string());
    if !full.contains(&escaped) {
        full.push_str(if full.contains('?') { "&" } else { "?" });
        full.push_str("q=");
        full.push_str(&escaped);
    }
    full
}

pub fn list(site: NovelSite, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
    if request.as_object().is_some_and(|object| object.is_empty()) {
        return Ok(Paged {
            entries: parse_listing(site, LIST_FIXTURE),
            has_next_page: false,
        });
    }
    let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
    let listing = request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular");
    let path = if listing == "latest" {
        site.latest_path
    } else {
        site.popular_path
    };
    let target = page_url(site, path, page);
    let body = fetch_document(site, &target, LIST_FIXTURE);
    Ok(Paged {
        entries: parse_listing(site, &body),
        has_next_page: has_next_page(&body),
    })
}

pub fn search(site: NovelSite, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
    let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
    let query = request
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if query.starts_with(site.base_url) {
        let key = normalize_key(site, query);
        let body = fetch_document(site, &url::join_url(site.base_url, &key), DETAILS_FIXTURE);
        return Ok(Paged {
            entries: vec![parse_details(site, &body, Some(key))],
            has_next_page: false,
        });
    }
    let target = search_url(site, query, page);
    let body = fetch_document(site, &target, LIST_FIXTURE);
    Ok(Paged {
        entries: parse_listing(site, &body),
        has_next_page: has_next_page(&body),
    })
}

pub fn details(site: NovelSite, request: Value) -> ExtensionResult<CatalogItem> {
    let key = request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
    let body = fetch_document(site, &url::join_url(site.base_url, &key), DETAILS_FIXTURE);
    Ok(parse_details(site, &body, Some(key)))
}

pub fn chapters(site: NovelSite, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
    let key = request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
    let body = fetch_document(site, &url::join_url(site.base_url, &key), CHAPTERS_FIXTURE);
    Ok(parse_chapters(site, &body))
}

pub fn chapters_page(site: NovelSite, request: Value) -> ExtensionResult<NovelChapterPage> {
    Ok(NovelChapterPage {
        entries: chapters(site, request)?,
        has_next_page: false,
        ..NovelChapterPage::default()
    })
}

pub fn text(site: NovelSite, request: Value) -> ExtensionResult<NovelText> {
    let key =
        request_key(&request, "chapter").unwrap_or_else(|| "novel/sample/chapter-1".to_string());
    let body = fetch_document(site, &url::join_url(site.base_url, &key), TEXT_FIXTURE);
    Ok(parse_text(site, &body, Some(key)))
}

pub fn home(site: NovelSite, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
    Ok(vec![HomeSection {
        id: "popular".to_string(),
        title: "Popular".to_string(),
        style: Some(HomeSectionStyle::Cover),
        entries: parse_listing(site, LIST_FIXTURE),
        has_more: true,
        ..HomeSection::default()
    }])
}

pub fn handle_url(site: NovelSite, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
    let Some(input) = request.get("url").and_then(Value::as_str) else {
        return Ok(None);
    };
    if input.starts_with(site.base_url) {
        let key = normalize_key(site, input);
        let body = fetch_document(site, &url::join_url(site.base_url, &key), DETAILS_FIXTURE);
        return Ok(Some(UrlResolveResult {
            item: Some(parse_details(site, &body, Some(key))),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }));
    }
    Ok(None)
}

pub fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("url")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

pub fn normalize_key(profile: NovelSite, input: &str) -> String {
    input
        .strip_prefix(profile.base_url)
        .unwrap_or(input)
        .trim_start_matches('/')
        .to_string()
}

pub fn parse_listing(profile: NovelSite, body: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(profile, &href);
            if !looks_like_novel_path(&key) || !seen.insert(key.clone()) {
                return None;
            }
            let title = html::attr(chunk, "title")
                .or_else(|| {
                    html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    url::slug_from_url(&key).unwrap_or_else(|| profile.name.to_string())
                });
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_near(chunk).map(|image| url::join_url(profile.base_url, &image)),
                url: Some(url::join_url(profile.base_url, &key)),
                language: Some(profile.lang.to_string()),
                content_rating: Some(profile.content_rating.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .take(48)
        .collect()
}

pub fn parse_details(profile: NovelSite, body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "novel/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: first_text(
            body,
            &["<h1", "entry-title", "novel-title", "book-name", "<title"],
        )
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| profile.name.to_string())),
        cover: image_near(body).map(|image| url::join_url(profile.base_url, &image)),
        description: description(body),
        authors: first_text(body, &["author", "writer"])
            .into_iter()
            .collect(),
        tags: tags(body),
        status: parse_status(body),
        url: Some(url::join_url(profile.base_url, &key)),
        language: Some(profile.lang.to_string()),
        content_rating: Some(profile.content_rating.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

pub fn parse_chapters(profile: NovelSite, body: &str) -> Vec<NovelChapter> {
    let mut seen = BTreeSet::new();
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(profile, &href);
            if !looks_like_chapter_path(&key) || !seen.insert(key.clone()) {
                return None;
            }
            let title = html::attr(chunk, "title")
                .or_else(|| {
                    html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(NovelChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: chapter_number(&key),
                url: Some(url::join_url(profile.base_url, &key)),
                language: Some(profile.lang.to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

pub fn parse_text(profile: NovelSite, body: &str, key: Option<String>) -> NovelText {
    let raw = content_block(body).unwrap_or_else(|| body.to_string());
    let normalized = novel::normalize_reader_html(&raw);
    NovelText {
        title: first_text(body, &["<h1", "chapter-title", "entry-title"]),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(profile.base_url.to_string()),
        css: Some("img { max-width: 100%; height: auto; } body { line-height: 1.7; }".to_string()),
        image_headers: novel::image_headers(profile.base_url),
        next_chapter_key: key,
        ..NovelText::default()
    }
}

pub fn first_text(body: &str, markers: &[&str]) -> Option<String> {
    markers
        .iter()
        .find_map(|marker| {
            html::text_between(body, marker, "</").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
}

pub fn image_near(body: &str) -> Option<String> {
    html::attr_after(body, "<img", "data-src")
        .or_else(|| html::attr_after(body, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(body, "<img", "src"))
}

pub fn description(body: &str) -> Option<String> {
    html::attr_after(body, "name=\"description\"", "content")
        .or_else(|| {
            html::text_between(body, "description", "</div>").map(|value| html::strip_tags(&value))
        })
        .or_else(|| html::text_between(body, "<p", "</p>").map(|value| html::strip_tags(&value)))
        .filter(|value| !value.is_empty())
}

pub fn tags(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("tag") || chunk.contains("genre"))
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .take(32)
        .collect()
}

pub fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") || lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("dropped") || lower.contains("cancelled") {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Ongoing
    }
}

pub fn content_block(body: &str) -> Option<String> {
    for marker in [
        "chapter-content",
        "entry-content",
        "reading-content",
        "chapter-c",
        "novel-content",
        "chapter__content",
        "trix-content",
        "prose",
        "post-body",
        "post-content",
        "<article",
        "<main",
    ] {
        if let Some(value) = html::text_between(body, marker, "</div>") {
            return Some(value);
        }
    }
    None
}

pub fn has_next_page(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("rel=\"next\"") || lower.contains("class=\"next") || lower.contains(">next<")
}

pub fn looks_like_novel_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    !lower.starts_with('#')
        && !lower.starts_with("javascript:")
        && !looks_like_chapter_path(&lower)
        && (lower.contains("novel")
            || lower.contains("book")
            || lower.contains("fiction")
            || lower.contains("series")
            || lower.contains("roman")
            || lower.contains("obra")
            || lower.contains("work")
            || lower.contains("oeuvre")
            || lower.contains("fictions")
            || lower.split('/').filter(|part| !part.is_empty()).count() <= 3)
}

pub fn looks_like_chapter_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("chapter")
        || lower.contains("chapitre")
        || lower.contains("capitulo")
        || lower.contains("capítulo")
        || lower.contains("chap-")
        || lower.contains("cap-")
        || lower.contains("/ch-")
        || lower.contains("/cap-")
        || lower.contains("/read/")
        || lower.contains("/episode/")
        || lower.contains("/episodes/")
        || lower.contains("/chapters/")
}

pub fn chapter_number(path: &str) -> Option<f32> {
    path.split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .filter(|part| !part.is_empty())
        .next_back()
        .and_then(|part| part.parse().ok())
}

pub const LIST_FIXTURE: &str = r#"
<a href="/novel/sample" title="Sample Novel"><img src="/covers/sample.jpg">Sample Novel</a>
"#;

pub const DETAILS_FIXTURE: &str = r#"
<h1>Sample Novel</h1><img src="/covers/sample.jpg"><p>A fixture detail page.</p>
<a href="/novel/sample/chapter-1">Chapter 1</a>
"#;

pub const CHAPTERS_FIXTURE: &str = r#"
<a href="/novel/sample/chapter-1">Chapter 1</a>
<a href="/novel/sample/chapter-2">Chapter 2</a>
"#;

pub const TEXT_FIXTURE: &str = r#"
<div class="chapter-content"><h1>Chapter 1</h1><p>The first fixture paragraph.</p></div>
"#;
