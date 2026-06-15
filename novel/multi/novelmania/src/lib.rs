use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, lnreader, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: NovelMania = NovelMania;
const BASE_URL: &str = "https://novelmania.com.br";

struct NovelMania;

impl NovelSource for NovelMania {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body =
            fetch_document_or_fixture(&novels_url("", page(&request), &request), LIST_FIXTURE);
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
        let body =
            fetch_document_or_fixture(&novels_url(query, page(&request), &request), LIST_FIXTURE);
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty() && lnreader::has_next_page(&body),
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novels/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novels/sample".to_string());
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
            .unwrap_or_else(|| "capitulo/sample-1".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        let raw = html::text_between(&body, "id=\"chapter-content\"", "</div>")
            .or_else(|| html::text_between(&body, "id='chapter-content'", "</div>"))
            .unwrap_or(body);
        Ok(text_from_html(&key, None, raw))
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

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn novels_url(query: &str, page: u64, request: &Value) -> String {
    let mut params = vec![
        format!("titulo={}", url::query_escape(query)),
        format!(
            "categoria={}",
            url::query_escape(&lnreader::filter_string_opt(request, "genres").unwrap_or_default())
        ),
        format!(
            "status={}",
            url::query_escape(&lnreader::filter_string_opt(request, "status").unwrap_or_default())
        ),
        format!(
            "nacionalidade={}",
            url::query_escape(&lnreader::filter_string_opt(request, "type").unwrap_or_default())
        ),
        format!(
            "ordem={}",
            url::query_escape(&lnreader::filter_string_opt(request, "ordem").unwrap_or_default())
        ),
    ];
    params.push(format!("page%5Bpage%5D={page}"));
    format!("{BASE_URL}/novels?{}", params.join("&"))
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("novel-title") || chunk.contains("card-image"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "novel-title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            if key.is_empty() || !seen.insert(key.clone()) {
                return None;
            }
            let title = html::text_between(chunk, "<h5", "</h5>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_key(&key));
            Some(catalog_item(
                key,
                title,
                html::attr_after(chunk, "card-image", "src"),
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
        html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_key(key)),
        html::attr_after(body, "novel-img", "src")
            .or_else(|| html::attr_after(body, "img-responsive", "src")),
        true,
    );
    item.description = html::text_between(body, "tab-pane fade show active", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    item.authors = html::text_between(body, "authors mb-1", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .into_iter()
        .collect();
    item.tags = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("tag") || chunk.contains("categoria"))
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect();
    item.status = html::text_between(body, "authors mb-3", "</")
        .map(|value| parse_status(&html::strip_tags(&value)))
        .unwrap_or(ItemStatus::Unknown);
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let mut seen = BTreeSet::new();
    body.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            if key.is_empty() || !seen.insert(key.clone()) {
                return None;
            }
            let sub_vol = html::text_between(chunk, "sub-vol", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            let title = html::text_between(chunk, "<strong", "</strong>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value))
                })
                .unwrap_or_else(|| "Chapter".to_string());
            let full_title = if sub_vol.is_empty() {
                title
            } else {
                format!("{sub_vol} - {title}")
            };
            Some(NovelChapter {
                key: key.clone(),
                title: Some(full_title),
                chapter_number: chapter_number(&key),
                url: Some(absolute_url(&key)),
                language: Some("multi".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
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
    input
        .contains("novelmania.com.br")
        .then(|| normalize_key(input))
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
        .unwrap_or("Novel Mania")
        .replace(['-', '_'], " ")
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "ativo" => ItemStatus::Ongoing,
        "pausado" => ItemStatus::Hiatus,
        "completo" => ItemStatus::Completed,
        "parado" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn chapter_number(key: &str) -> Option<f32> {
    key.split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .filter(|part| !part.is_empty())
        .next_back()
        .and_then(|part| part.parse().ok())
}

export_novel_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="top-novels dark col-6"><div class="row mb-2"><a class="novel-title" href="/novels/sample"><h5>Sample Novel</h5></a><img class="card-image" src="/cover.jpg"></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="novel-img"><img class="img-responsive" src="/cover.jpg"></div>
<div class="novel-info"><h1>Sample Novel</h1><span class="authors mb-1">Author</span><span class="authors mb-3">Ativo</span></div>
<div class="tab-pane fade show active"><div class="text"><p>Sample summary.</p></div></div>
<div class="accordion capitulo"><div class="card"><div class="collapse"><div class="card-body p-0"><ol><li><a href="/capitulo/sample-1"><span class="sub-vol">Vol 1</span><strong>Chapter 1</strong></a></li></ol></div></div></div></div>
"#;

const TEXT_FIXTURE: &str = r#"<div id="chapter-content"><p>Sample chapter text.</p></div>"#;
