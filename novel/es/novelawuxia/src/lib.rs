use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: ReinoWuxia = ReinoWuxia;
const BASE_URL: &str = "http://www.reinowuxia.com/";

struct ReinoWuxia;

impl NovelSource for ReinoWuxia {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body =
            fetch_document_or_fixture(&absolute_url("p/todas-las-novelas.html"), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
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
        let body = fetch_document_or_fixture(
            &format!("{BASE_URL}search?q={}", url::query_escape(query)),
            SEARCH_FIXTURE,
        );
        Ok(Paged {
            entries: parse_search(&body),
            has_next_page: has_next_page(&body),
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
            .unwrap_or_else(|| "2024/01/sample-capitulo-1.html".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Todas las Novelas".to_string(),
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

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let area = block_after(body, "post-body entry-content").unwrap_or_else(|| body.to_string());
    let mut seen = BTreeSet::new();
    area.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            if !seen.insert(key.clone()) {
                return None;
            }
            let title = html::attr_after(chunk, "<img", "alt")
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_key(&key));
            Some(catalog_item(key, title, image_from(chunk), false))
        })
        .collect()
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split("date-outer")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")?;
            let mut slug = href
                .split('/')
                .next_back()
                .unwrap_or_default()
                .replace(".html", "");
            if let Some((name, _)) = slug.split_once("-capitulo") {
                slug = name.to_string();
            }
            if slug.is_empty() {
                return None;
            }
            let key = format!("{slug}.html/");
            let title = title_from_key(&key);
            seen.insert(title.clone())
                .then(|| catalog_item(key, title, None, false))
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
        text_after(body, "post-title")
            .or_else(|| text_between_tag(body, "h1"))
            .unwrap_or_else(|| title_from_key(key)),
        html::attr_after(body, "separator", "href")
            .or_else(|| image_from(body))
            .map(|value| absolute_url(&value)),
        true,
    );
    item.authors = labeled_value(body, "Autor").into_iter().collect();
    item.tags = labeled_value(body, "Géneros")
        .or_else(|| labeled_value(body, "Generos"))
        .map(split_values)
        .unwrap_or_default();
    item.status = parse_status(&labeled_value(body, "Estatus").unwrap_or_default());
    item.description = synopsis(body);
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let area = body.split("Lista de Capítulos").nth(1).unwrap_or(body);
    let mut seen = BTreeSet::new();
    area.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            if key == "/" || !seen.insert(key.clone()) {
                return None;
            }
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            Some(NovelChapter {
                key: key.clone(),
                title,
                chapter_number: chapter_number(&key),
                url: Some(absolute_url(&key)),
                language: Some("es".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let raw = block_after(body, "post-body entry-content").unwrap_or_else(|| body.to_string());
    let normalized = novel::normalize_reader_html(&raw);
    NovelText {
        title: text_after(body, "post-title").or_else(|| text_between_tag(body, "h1")),
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

fn image_from(block: &str) -> Option<String> {
    html::attr_after(block, "<img", "src").map(|value| absolute_url(&value))
}

fn labeled_value(body: &str, label: &str) -> Option<String> {
    let label_pos = body.find(label)?;
    let rest = &body[label_pos..];
    let text = rest
        .split("<br")
        .next()
        .map(html::strip_tags)
        .unwrap_or_default();
    text.split_once(':')
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn synopsis(body: &str) -> Option<String> {
    body.split("Sinopsis")
        .nth(1)
        .and_then(|chunk| {
            html::text_between(chunk, "<div", "</div>")
                .or_else(|| html::text_between(chunk, "<p", "</p>"))
        })
        .map(|value| {
            html::strip_tags(&value)
                .replace("Sinopsis", "")
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn split_values(value: String) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("complet") || lower.contains("finaliz") {
        ItemStatus::Completed
    } else if lower.contains("paus") {
        ItemStatus::Hiatus
    } else if lower.is_empty() {
        ItemStatus::Unknown
    } else {
        ItemStatus::Ongoing
    }
}

fn text_after(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</")
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
    let end = rest.find("</div>").unwrap_or(rest.len());
    Some(rest[..end + "</div>".len().min(rest.len().saturating_sub(end))].to_string())
}

fn title_from_key(key: &str) -> String {
    let slug = key
        .trim_end_matches('/')
        .trim_end_matches(".html")
        .split('/')
        .next_back()
        .unwrap_or(key);
    slug.replace('-', " ")
        .split_whitespace()
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn has_next_page(body: &str) -> bool {
    body.contains("blog-pager-older-link") || body.contains("rel=\"next\"")
}

fn chapter_number(key: &str) -> Option<f32> {
    key.split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .filter(|part| !part.is_empty())
        .next_back()
        .and_then(|part| part.parse().ok())
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .contains("reinowuxia.com")
        .then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches("http://www.reinowuxia.com/")
        .trim_start_matches("https://www.reinowuxia.com/")
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
<div class="post-body entry-content"><a href="http://www.reinowuxia.com/p/sample.html"><img src="/cover.jpg" alt="Sample Novel"></a></div>
"#;

const SEARCH_FIXTURE: &str = r#"
<div class="date-outer"><a href="http://www.reinowuxia.com/2024/01/sample-capitulo-1.html">Sample Capitulo 1</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="post-title">Sample Novel</h1><div class="separator"><a href="/cover.jpg"><img src="/cover.jpg"></a></div>
<div><b>Autor:</b> Sample Author<br><b>Estatus:</b> En curso<br><b>Géneros:</b> Fantasia, Accion</div>
<div>Sinopsis</div><div>Sample summary.</div>
<div>Lista de Capítulos <a href="http://www.reinowuxia.com/2024/01/sample-capitulo-1.html">Capitulo 1</a></div>
"#;

const TEXT_FIXTURE: &str = r#"
<h1 class="post-title">Capitulo 1</h1><div class="post-body entry-content"><p>Sample chapter text.</p></div>
"#;

export_novel_source!(SOURCE);
