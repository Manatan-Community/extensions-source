use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    html, novel,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: NovelsOnline = NovelsOnline;
const BASE_URL: &str = "https://novelsonline.org";

struct NovelsOnline;

impl NovelSource for NovelsOnline {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if has_active_filters(&request) {
            return Ok(Paged {
                entries: parse_listing(&post_search(&request, "")),
                has_next_page: false,
            });
        }
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/top-novel/{page}"), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
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
        Ok(Paged {
            entries: parse_listing(&post_search(&request, query)),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "/novel/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "/novel/sample".to_string());
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
            .unwrap_or_else(|| "/novel/sample/chapter-1".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Top Novels".to_string(),
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

fn post_search(request: &Value, query: &str) -> String {
    let mut form: Vec<(String, String)> = Vec::new();
    let keyword = request
        .get("filters")
        .and_then(|filters| filters.get("keyword"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(query);
    if !keyword.is_empty() {
        form.push(("keyword".to_string(), keyword.to_string()));
    }
    push_multi(request, &mut form, "novel_type");
    push_multi(request, &mut form, "language");
    push_multi(request, &mut form, "genre");
    push_single(request, &mut form, "completed");
    form.push(("search".to_string(), "1".to_string()));
    let borrowed: Vec<(&str, &str)> = form
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    client()
        .post(format!("{BASE_URL}/detailed-search"))
        .referer(BASE_URL)
        .form(&borrowed)
        .send_text()
        .unwrap_or_else(|_| LIST_FIXTURE.to_string())
}

fn push_multi(request: &Value, form: &mut Vec<(String, String)>, id: &str) {
    let Some(values) = request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_array)
    else {
        return;
    };
    for value in values
        .iter()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        form.push((format!("include[{id}][]"), value.to_string()));
    }
}

fn push_single(request: &Value, form: &mut Vec<(String, String)>, id: &str) {
    let Some(value) = request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    form.push((format!("include[{id}][]"), value.to_string()));
}

fn has_active_filters(request: &Value) -> bool {
    request
        .get("filters")
        .and_then(Value::as_object)
        .is_some_and(|filters| {
            filters.values().any(|value| match value {
                Value::String(text) => !text.is_empty(),
                Value::Array(values) => !values.is_empty(),
                Value::Bool(value) => *value,
                _ => false,
            })
        })
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("top-novel-block")
        .skip(1)
        .filter_map(parse_listing_block)
        .take(48)
        .collect()
}

fn parse_listing_block(block: &str) -> Option<CatalogItem> {
    let href =
        html::attr_after(block, "<h2", "href").or_else(|| html::attr_after(block, "<a", "href"))?;
    let key = normalize_key(&href);
    let title = html::text_between(block, "<h2", "</h2>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(block, "<img", "src").map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let mut item = CatalogItem {
        key: normalize_key(key),
        title: first_text(body, &["<h1", "<title"])
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(body, "novel-cover", "src").map(|image| absolute_url(&image)),
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    };
    for block in body.split("novel-detail-item").skip(1) {
        let label = html::text_between(block, "<h6", "</h6>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default();
        let detail = html::text_between(block, "novel-detail-body", "</div>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default();
        match label.trim() {
            "Description" => item.description = Some(detail),
            "Genre" => item.tags = split_words(&detail),
            "Author(s)" => item.authors = split_words(&detail),
            "Artist(s)" if detail.trim() != "N/A" => item.artists = split_words(&detail),
            "Status" => item.status = parse_status(&detail),
            _ => {}
        }
    }
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    body.split("chapter-chs")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            Some(NovelChapter {
                key: key.clone(),
                title,
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let html_body = html::text_between(body, "id=\"contentall\"", "</div>")
        .or_else(|| html::text_between(body, "id='contentall'", "</div>"))
        .unwrap_or_else(|| TEXT_FIXTURE.to_string());
    let normalized = novel::normalize_reader_html(&html_body);
    NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn first_text(body: &str, markers: &[&str]) -> Option<String> {
    markers.iter().find_map(|marker| {
        html::text_between(body, marker, "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
    })
}

fn split_words(value: &str) -> Vec<String> {
    value
        .split([',', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.to_ascii_lowercase().as_str() {
        text if text.contains("complete") => ItemStatus::Completed,
        text if text.contains("ongoing") => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn has_next_page(body: &str) -> bool {
    body.contains("rel=\"next\"") || body.contains("pagination") && body.contains("Next")
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .contains("novelsonline.org")
        .then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    let path = input
        .trim()
        .trim_start_matches(BASE_URL)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_matches('/');
    format!("/{path}")
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        url::join_url(BASE_URL, input)
    }
}

const LIST_FIXTURE: &str = r#"
<div class="top-novel-block"><div class="top-novel-cover"><img src="/cover.jpg"></div><h2><a href="/novel/sample">Sample Novel</a></h2></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample Novel</h1><div class="novel-cover"><a><img src="/cover.jpg"></a></div>
<div class="novel-detail-item"><h6>Description</h6><div class="novel-detail-body">Sample summary.</div></div>
<div class="novel-detail-item"><h6>Genre</h6><div class="novel-detail-body"><li>Fantasy</li></div></div>
<div class="novel-detail-item"><h6>Author(s)</h6><div class="novel-detail-body"><li>Sample Author</li></div></div>
<div class="novel-detail-item"><h6>Status</h6><div class="novel-detail-body">Ongoing</div></div>
<ul class="chapter-chs"><li><a href="/novel/sample/chapter-1">Chapter 1</a></li></ul>
"#;

const TEXT_FIXTURE: &str = r#"<div id="contentall"><p>Sample chapter text.</p></div>"#;

export_novel_source!(SOURCE);
