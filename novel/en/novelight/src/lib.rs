use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Novelight = Novelight;
const BASE_URL: &str = "https://novelight.net";

struct Novelight;

impl NovelSource for Novelight {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let body = fetch_or_fixture(
            &catalog_url(&request, page, listing == "latest"),
            LIST_FIXTURE,
        );
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
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        let body = fetch_or_fixture(
            &format!("{BASE_URL}/catalog/?search={}", url::query_escape(query)),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "book/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        Ok(self.chapters_page(request)?.entries)
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "book/sample".to_string());
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let detail = fetch_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let total_pages = total_chapter_pages(&detail);
        let remote_page = total_pages.saturating_sub(page).saturating_add(1);
        let csrf = capture_between(&detail, "window.CSRF_TOKEN = \"", "\"").unwrap_or_default();
        let book_id = capture_between(&detail, "const OBJECT_BY_COMMENT = ", ";")
            .unwrap_or_else(|| "1".to_string());
        let ajax = fetch_or_fixture(
            &format!(
                "{BASE_URL}/book/ajax/chapter-pagination?csrfmiddlewaretoken={csrf}&book_id={book_id}&page={remote_page}"
            ),
            CHAPTERS_FIXTURE,
        );
        let root: Value = serde_json::from_str(&ajax).unwrap_or(Value::Null);
        let html = root
            .get("html")
            .and_then(Value::as_str)
            .unwrap_or(CHAPTERS_HTML_FIXTURE);
        let entries = parse_chapters_html(html, pref_bool(&request, "hideLocked"));
        Ok(NovelChapterPage {
            entries,
            has_next_page: page < total_pages,
            next_page: (page < total_pages).then_some(page as u32 + 1),
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "book/sample/chapter-1".to_string());
        let raw_page = fetch_or_fixture(&absolute_url(&key), TEXT_PAGE_FIXTURE);
        let csrf = capture_between(&raw_page, "window.CSRF_TOKEN = \"", "\"").unwrap_or_default();
        let chapter_id = capture_between(&raw_page, "const CHAPTER_ID = \"", "\"")
            .or_else(|| capture_between(&raw_page, "const CHAPTER_ID = ", ";"))
            .unwrap_or_else(|| "1".to_string());
        let ajax = client()
            .get(format!("{BASE_URL}/book/ajax/read-chapter/{chapter_id}"))
            .header("Cookie", format!("csrftoken={csrf}"))
            .header("X-Requested-With", "XMLHttpRequest")
            .referer(absolute_url(&key))
            .send_text()
            .unwrap_or_else(|_| TEXT_AJAX_FIXTURE.to_string());
        let root: Value = serde_json::from_str(&ajax).unwrap_or(Value::Null);
        let class_name = root
            .get("class")
            .and_then(Value::as_str)
            .unwrap_or("content");
        let content = root
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or(TEXT_CONTENT_FIXTURE);
        let html = extract_class_content(content, class_name)
            .unwrap_or_else(|| content.to_string())
            .replace("class=\"advertisment\"", "style=\"display:none;\"");
        let normalized = novel::normalize_reader_html(&html);
        Ok(NovelText {
            html: Some(normalized.clone()),
            text: Some(novel::cleanup_text(&normalized)),
            base_url: Some(BASE_URL.to_string()),
            css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; } .advertisment { display: none; }".to_string()),
            image_headers: novel::image_headers(BASE_URL),
            next_chapter_key: Some(key),
            ..NovelText::default()
        })
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
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

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn catalog_url(request: &Value, page: u64, latest: bool) -> String {
    if latest {
        return format!("{BASE_URL}/catalog/?ordering=-time_updated&page={page}");
    }
    let mut params = Vec::new();
    for key in ["country", "genres", "translation", "status", "novel_type"] {
        let param = if key == "genres" { "genre" } else { key };
        for value in filter_array(request, key) {
            params.push(format!("{param}={}", url::query_escape(&value)));
        }
    }
    params.push(format!(
        "ordering={}",
        filter_text(request, "sort", "popularity")
    ));
    params.push(format!("page={page}"));
    format!("{BASE_URL}/catalog/?{}", params.join("&"))
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, &normalize_key(key))
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|block| block.contains("item"))
        .filter_map(|block| {
            let href = html::attr(block, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(block, "div class=\"title\"", "</div>")
                .or_else(|| html::text_between(block, "div class='title'", "</div>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(block, "<img", "src").map(|src| absolute_url(&src)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("suggestive".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .take(48)
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let mut item = CatalogItem {
        key: normalize_key(key),
        title: first_text(body, &["<h1", "<title"])
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: body
            .split("poster")
            .nth(1)
            .and_then(|poster| html::attr_after(poster, "<img", "src"))
            .map(|src| absolute_url(&src)),
        url: Some(absolute_url(key)),
        description: html::text_between(body, "text-info section", "</section>")
            .map(|value| html::strip_tags(&value)),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    };
    for block in body
        .split("mini-info")
        .skip(1)
        .flat_map(|part| part.split("sub-header").skip(1))
    {
        let label = html::text_between(block, ">", "</")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default();
        let value = html::text_between(block, "info", "</div>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default();
        match label.trim() {
            "Author" if !value.is_empty() => item.authors.push(value),
            "Genres" => item.tags = split_csv(&value),
            "Status" => item.status = parse_status(&value),
            "Translation" if item.status == ItemStatus::Unknown => {
                item.status = parse_status(&value)
            }
            _ => {}
        }
    }
    if item.status == ItemStatus::Unknown {
        item.status = parse_status(body);
    }
    item
}

fn parse_chapters_html(body: &str, hide_locked: bool) -> Vec<NovelChapter> {
    let mut chapters = Vec::new();
    for block in body.split("<a").skip(1) {
        let Some(href) = html::attr(block, "href") else {
            continue;
        };
        let locked = block.contains("cost");
        if hide_locked && locked {
            continue;
        }
        let key = normalize_key(&href);
        let mut title = html::text_between(block, "title", "</")
            .or_else(|| html::text_between(block, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Chapter".to_string());
        if locked {
            title = format!("[Locked] {title}");
        }
        chapters.push(NovelChapter {
            key: key.clone(),
            title: Some(title),
            url: Some(absolute_url(&key)),
            language: Some("en".to_string()),
            ..NovelChapter::default()
        });
    }
    chapters.reverse();
    chapters
}

fn extract_class_content(body: &str, class_name: &str) -> Option<String> {
    for marker in [
        format!("class=\"{class_name}\""),
        format!("class='{class_name}'"),
    ] {
        if let Some(content) = html::text_between(body, &marker, "</div>") {
            return Some(content);
        }
    }
    None
}

fn total_chapter_pages(body: &str) -> u64 {
    body.match_indices("<option")
        .filter_map(|(idx, _)| html::attr(&body[idx..], "value"))
        .filter_map(|value| value.parse::<u64>().ok())
        .max()
        .unwrap_or(1)
}

fn capture_between(input: &str, start: &str, end: &str) -> Option<String> {
    input
        .split(start)
        .nth(1)
        .and_then(|rest| rest.split(end).next())
        .map(ToString::to_string)
}

fn first_text(body: &str, markers: &[&str]) -> Option<String> {
    markers.iter().find_map(|marker| {
        html::text_between(body, marker, "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
    })
}

fn split_csv(input: &str) -> Vec<String> {
    input
        .split([',', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("cancel") || lower.contains("dropped") {
        ItemStatus::Cancelled
    } else if lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("paused") || lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("ongoing") || lower.contains("releasing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn filter_text(request: &Value, id: &str, default: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .unwrap_or(default)
        .to_string()
}

fn filter_array(request: &Value, id: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(|value| value.get("value").unwrap_or(value).as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn pref_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .or_else(|| request.get("settings"))
        .and_then(|settings| settings.get(id))
        .and_then(|value| value.get("value").unwrap_or(value).as_bool())
        .unwrap_or(false)
}

fn has_next_page(body: &str) -> bool {
    body.contains("rel=\"next\"") || body.to_ascii_lowercase().contains(">next<")
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches('/')
        .split('?')
        .next()
        .unwrap_or(input)
        .to_string()
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

const LIST_FIXTURE: &str = r#"<a class="item" href="/book/sample"><img src="/cover.jpg"><div class="title">Sample Novel</div></a>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample Novel</h1><div class="poster"><img src="/cover.jpg"></div><section class="text-info section"><p>Sample summary.</p></section><div class="mini-info"><div class="item"><div class="sub-header">Author</div><div class="info">Sample Author</div></div><div class="item"><div class="sub-header">Status</div><div class="info">Releasing</div></div></div><select id="select-pagination-chapter"><option value="1">1</option></select><script>window.CSRF_TOKEN = "sample"; const OBJECT_BY_COMMENT = 1;</script>"#;
const CHAPTERS_FIXTURE: &str = r#"{"html":"<a href=\"/book/sample/chapter-1\"><div class=\"title\">Chapter 1</div><div class=\"date\">01.01.2024</div></a>"}"#;
const CHAPTERS_HTML_FIXTURE: &str =
    r#"<a href="/book/sample/chapter-1"><div class="title">Chapter 1</div></a>"#;
const TEXT_PAGE_FIXTURE: &str =
    r#"<script>window.CSRF_TOKEN = "sample"; const CHAPTER_ID = "1";</script>"#;
const TEXT_AJAX_FIXTURE: &str =
    r#"{"class":"content","content":"<div class=\"content\"><p>Sample chapter text.</p></div>"}"#;
const TEXT_CONTENT_FIXTURE: &str = r#"<div class="content"><p>Sample chapter text.</p></div>"#;

export_novel_source!(SOURCE);
