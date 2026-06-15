use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage, NovelText, Paged,
    UrlResolveResult,
};
use manatan_shared::{html, novel, sdk::http::HttpClient, sdk::SearchRequest, url};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: YukiTls = YukiTls;
const BASE_URL: &str = "https://yuukitls.com/";

struct YukiTls;

impl NovelSource for YukiTls {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = fetch_document_or_fixture(BASE_URL, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_menu_listing(&body, "quadmenu-navbar-collapse"),
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
        let needle = query.to_ascii_lowercase();
        let body = fetch_document_or_fixture(BASE_URL, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_menu_listing(&body, "menu-item-2869")
                .into_iter()
                .filter(|item| item.title.to_ascii_lowercase().contains(&needle))
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novela/sample/".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novela/sample/".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
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
            .unwrap_or_else(|| "novela/sample/capitulo-1/".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: popular.entries,
            has_more: false,
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

fn parse_menu_listing(body: &str, marker: &str) -> Vec<CatalogItem> {
    let menu = body
        .find(marker)
        .map(|start| &body[start..])
        .unwrap_or(body);
    let mut seen = BTreeSet::new();
    menu.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.starts_with(BASE_URL) {
                return None;
            }
            let key = normalize_key(&href);
            let title = link_text(chunk).unwrap_or_else(|| title_from_key(&key));
            if title.is_empty() || !seen.insert(key.clone()) {
                return None;
            }
            Some(catalog_item(key, title, image_from(chunk), false))
        })
        .collect()
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let content = block_after(body, "entry-content").unwrap_or_else(|| body.to_string());
    let mut item = catalog_item(
        normalize_key(key),
        text_between_tag(body, "h1").unwrap_or_else(|| title_from_key(key)),
        html::attr_after(body, "loading=\"lazy\"", "src")
            .or_else(|| image_from(body))
            .map(|value| absolute_url(&value)),
        true,
    );
    item.authors = detail_value(&content, "Escritor:").into_iter().collect();
    item.tags = detail_value(&content, "G\u{e9}nero:")
        .or_else(|| detail_value(&content, "Genero:"))
        .map(split_values)
        .unwrap_or_default();
    item.description = content
        .split("Sinopsis:")
        .nth(1)
        .map(html::strip_tags)
        .filter(|value| !value.is_empty());
    item
}

fn parse_chapters(body: &str, novel_key: &str) -> Vec<NovelChapter> {
    let content = block_after(body, "entry-content").unwrap_or_else(|| body.to_string());
    let mut seen = BTreeSet::new();
    content
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.starts_with(BASE_URL) {
                return None;
            }
            let key = normalize_key(&href);
            if key == normalize_key(novel_key) || !seen.insert(key.clone()) {
                return None;
            }
            Some(NovelChapter {
                key: key.clone(),
                title: link_text(chunk),
                chapter_number: chapter_number(&key),
                url: Some(absolute_url(&key)),
                language: Some("es".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let raw = block_after(body, "entry-content").unwrap_or_else(|| body.to_string());
    let normalized = novel::normalize_reader_html(&raw);
    NovelText {
        title: text_between_tag(body, "h1"),
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
        cover,
        url: Some(absolute_url(&key)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized,
        ..CatalogItem::default()
    }
}

fn detail_value(body: &str, label: &str) -> Option<String> {
    html::strip_tags(body).split(['\n', '|']).find_map(|line| {
        let idx = line.find(label)?;
        let rest = line[idx + label.len()..].trim();
        (!rest.is_empty()).then(|| rest.to_string())
    })
}

fn split_values(value: String) -> Vec<String> {
    value
        .split([',', '/', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn image_from(block: &str) -> Option<String> {
    html::attr_after(block, "<img", "data-src")
        .or_else(|| html::attr_after(block, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(block, "<img", "src"))
        .map(|value| absolute_url(&value))
}

fn link_text(chunk: &str) -> Option<String> {
    html::text_between(chunk, ">", "</a>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn text_between_tag(body: &str, tag: &str) -> Option<String> {
    html::text_between(body, &format!("<{tag}"), &format!("</{tag}>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn block_after(body: &str, marker: &str) -> Option<String> {
    let start = body.find(marker)?;
    let rest = &body[start..];
    let end = rest
        .find("</article>")
        .or_else(|| rest.find("</main>"))
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn chapter_number(key: &str) -> Option<f32> {
    key.split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .filter(|part| !part.is_empty())
        .next_back()
        .and_then(|part| part.parse().ok())
}

fn title_from_key(key: &str) -> String {
    url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())
}

fn key_from_url(input: &str) -> Option<String> {
    input.contains("yuukitls.com").then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://yuukitls.com/")
        .trim_start_matches('/')
        .to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

const LIST_FIXTURE: &str = r#"
<div class="quadmenu-navbar-collapse"><ul><li><a href="https://yuukitls.com/novela/sample/"><img src="/cover.jpg">Sample Novel</a></li></ul></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample Novel</h1><div class="entry-content"><div>Escritor: Sample Author</div><div>Genero: Fantasia</div><div>Sinopsis:</div><p>Sample summary.</p><li><a href="https://yuukitls.com/novela/sample/capitulo-1/">Capitulo 1</a></li></div>
"#;

const TEXT_FIXTURE: &str = r#"
<h1 class="entry-title">Capitulo 1</h1><div class="entry-content"><p>Sample chapter text.</p></div>
"#;

export_novel_source!(SOURCE);
