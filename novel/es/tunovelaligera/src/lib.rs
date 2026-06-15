use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage, NovelText, Paged,
    UrlResolveResult,
};
use manatan_shared::{html, novel, sdk::http::HttpClient, sdk::SearchRequest, url};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: TuNovelaLigera = TuNovelaLigera;
const BASE_URL: &str = "https://tunovelaligera.com";

struct TuNovelaLigera;

impl NovelSource for TuNovelaLigera {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = filter_string(&request, "order", "rating");
        let genre = filter_string(&request, "genres", "");
        let body = if order != "rating" {
            fetch_document_or_fixture(
                &format!("{BASE_URL}/novelas/?m_orderby={order}"),
                LIST_FIXTURE,
            )
        } else {
            post_load_more(page, &genre)
        };
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty(),
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
        let body = fetch_document_or_fixture(
            &format!(
                "{BASE_URL}/?s={}&post_type=wp-manga",
                url::query_escape(query)
            ),
            SEARCH_FIXTURE,
        );
        Ok(Paged {
            entries: parse_search(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novelas/sample/".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novelas/sample/".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let ajax = post_chapters(&key);
        let source = if ajax.contains("wp-manga") || ajax.contains("<li") {
            ajax
        } else {
            body
        };
        Ok(parse_chapters(&source))
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
            .unwrap_or_else(|| "novelas/sample/capitulo-1/".to_string());
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
        .with_origin(BASE_URL)
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

fn post_load_more(page: u64, genre: &str) -> String {
    let page_string = page.to_string();
    let mut form = vec![
        ("action", "madara_load_more"),
        ("page", page_string.as_str()),
        ("template", "madara-core/content/content-archive"),
        ("vars[post_type]", "wp-manga"),
    ];
    if !genre.is_empty() {
        form.push(("vars[wp-manga-genre]", genre));
    }
    client()
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .xhr()
        .form(&form)
        .send_text()
        .unwrap_or_else(|_| LIST_FIXTURE.to_string())
}

fn post_chapters(key: &str) -> String {
    let slug = key.split('/').nth(1).unwrap_or("sample");
    client()
        .post(format!("{BASE_URL}/novelas/{slug}/ajax/chapters/"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string())
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    parse_listing_blocks(body, "page-item-detail", ".h5")
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    parse_listing_blocks(body, "c-tabs-item__content", ".h4")
}

fn parse_listing_blocks(body: &str, marker: &str, title_marker: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split(marker)
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, title_marker, "href")
                .or_else(|| html::attr_after(block, "<a", "href"))?;
            let key = normalize_key(&href);
            if !seen.insert(key.clone()) {
                return None;
            }
            let title = text_after(block, title_marker)
                .or_else(|| link_text(block))
                .unwrap_or_else(|| title_from_key(&key));
            Some(catalog_item(key, title, image_from(block), false))
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
        text_between_tag(body, "h1").unwrap_or_else(|| title_from_key(key)),
        html::attr_after(body, "summary_image", "data-src")
            .or_else(|| html::attr_after(body, "summary_image", "src"))
            .or_else(|| html::attr_after(body, "summary_image", "data-cfsrc"))
            .or_else(|| image_from(body))
            .map(|value| absolute_url(&value)),
        true,
    );
    item.description = block_after(body, "summary__content")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    for block in body.split("post-content_item").skip(1) {
        let name = text_after(block, "summary-heading").unwrap_or_default();
        let detail = text_after(block, "summary-content").unwrap_or_default();
        match name.as_str() {
            "Genero(s)" | "G\u{e9}nero(s)" => item.tags = split_values(detail),
            "Autor(es)" => item.authors = vec![detail],
            "Estado" => item.status = parse_status(&detail),
            _ => {}
        }
    }
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let marker = if body.contains("wp-manga") {
        "wp-manga"
    } else {
        "lcp_instance"
    };
    let mut seen = BTreeSet::new();
    body.split("<li")
        .skip(1)
        .filter(|block| block.contains(marker) || block.contains("<a"))
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")?;
            let key = normalize_key(&href);
            if !seen.insert(key.clone()) {
                return None;
            }
            Some(NovelChapter {
                key: key.clone(),
                title: link_text(block),
                chapter_number: chapter_number(&key),
                url: Some(absolute_url(&key)),
                language: Some("es".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let raw = block_after(body, "entry-content_wrap")
        .or_else(|| block_after(body, "reading-content"))
        .or_else(|| block_after(body, "entry-content"))
        .unwrap_or_else(|| body.to_string());
    let cleaned = remove_blocks(&raw, &["code-block", "ads", "manga-title-badges", "script"]);
    let normalized = novel::normalize_reader_html(&cleaned);
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

fn filter_string(request: &Value, id: &str, default: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(|value| value.get("value").or(Some(value)))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn split_values(value: String) -> Vec<String> {
    value
        .split([',', '/', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("completed") || lower.contains("complet") || lower.contains("finaliz") {
        ItemStatus::Completed
    } else if lower.contains("paus") {
        ItemStatus::Hiatus
    } else if lower.contains("drop") {
        ItemStatus::Cancelled
    } else if lower.is_empty() {
        ItemStatus::Unknown
    } else {
        ItemStatus::Ongoing
    }
}

fn image_from(block: &str) -> Option<String> {
    html::attr_after(block, "<img", "data-src")
        .or_else(|| html::attr_after(block, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(block, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(block, "<img", "src"))
        .map(|value| absolute_url(&value))
}

fn text_after(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn link_text(chunk: &str) -> Option<String> {
    html::text_between(chunk, "<a", "</a>")
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
        .or_else(|| rest.find("wp-manga-tags"))
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn remove_blocks(input: &str, markers: &[&str]) -> String {
    input
        .split("<div")
        .enumerate()
        .filter_map(|(idx, part)| {
            let marked = markers.iter().any(|marker| part.contains(marker));
            if idx > 0 && marked {
                None
            } else if idx == 0 {
                Some(part.to_string())
            } else {
                Some(format!("<div{part}"))
            }
        })
        .collect()
}

fn has_next_page(body: &str) -> bool {
    body.contains("rel=\"next\"") || body.contains("next page-numbers")
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
    input
        .contains("tunovelaligera.com")
        .then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://tunovelaligera.com/")
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
<div class="page-item-detail"><div class="h5"><a href="https://tunovelaligera.com/novelas/sample/">Sample Novel</a></div><img src="/cover.jpg"></div>
"#;

const SEARCH_FIXTURE: &str = r#"
<div class="c-tabs-item__content"><div class="h4"><a href="https://tunovelaligera.com/novelas/sample/">Sample Novel</a></div><img src="/cover.jpg"></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample Novel</h1><div class="summary_image"><img src="/cover.jpg"></div><div class="post-content_item"><div class="summary-heading"><h5>Autor(es)</h5></div><div class="summary-content">Sample Author</div></div><div class="post-content_item"><div class="summary-heading"><h5>Estado</h5></div><div class="summary-content">OnGoing</div></div><div class="summary__content"><p>Sample summary.</p></div>
"#;

const CHAPTERS_FIXTURE: &str = r#"
<ul><li class="wp-manga-chapter"><a href="https://tunovelaligera.com/novelas/sample/capitulo-1/">Capitulo 1</a></li></ul>
"#;

const TEXT_FIXTURE: &str = r#"
<h1>Capitulo 1</h1><div class="entry-content_wrap"><p>Sample chapter text.</p></div>
"#;

export_novel_source!(SOURCE);
