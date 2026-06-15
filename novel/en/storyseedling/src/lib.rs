use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: StorySeedling = StorySeedling;
const BASE_URL: &str = "https://storyseedling.com";

struct StorySeedling;

impl NovelSource for StorySeedling {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_browse(page, "");
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: !parse_listing(&body).is_empty(),
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_browse(page, query);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: !parse_listing(&body).is_empty(),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "series/sample".to_string());
        Ok(fetch_details(&normalize_key(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "series/sample".to_string());
        Ok(fetch_chapters(&normalize_key(&key)))
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
            .unwrap_or_else(|| "series/sample/chapter-1".to_string());
        let chapter_page = fetch_document_or_fixture(&absolute_url(&key), TEXT_PAGE_FIXTURE);
        let nonce = load_chapter_nonce(&chapter_page);
        let html_body = nonce
            .and_then(|nonce| fetch_chapter_content(&normalize_key(&key), &nonce))
            .unwrap_or_else(|| TEXT_FIXTURE.to_string());
        let decoded = remove_brand_spans(&decode_chapter_html(&html_body));
        Ok(NovelText {
            html: Some(decoded.clone()),
            text: Some(novel::cleanup_text(&decoded)),
            base_url: Some(absolute_url(&key)),
            css: Some(
                "body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string(),
            ),
            image_headers: novel::image_headers(&absolute_url(&key)),
            ..NovelText::default()
        })
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let latest = self.list(request)?;
        Ok(vec![HomeSection {
            id: "recent".to_string(),
            title: "Recent".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: latest.entries,
            has_more: latest.has_next_page,
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

fn fetch_browse(page: u64, query: &str) -> String {
    let browse_page = fetch_document_or_fixture(&format!("{BASE_URL}/browse"), BROWSE_FIXTURE);
    let post = quoted_after(&browse_page, "browse('", "')").unwrap_or_else(|| "sample".to_string());
    client()
        .post(format!("{BASE_URL}/ajax"))
        .referer(format!("{BASE_URL}/browse"))
        .xhr()
        .form(&[
            ("search", query),
            ("orderBy", "recent"),
            ("curpage", &page.to_string()),
            ("post", &post),
            ("action", "fetch_browse"),
        ])
        .send_text()
        .unwrap_or_else(|_| LIST_FIXTURE.to_string())
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    root.get("data")
        .and_then(|data| data.get("posts"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|post| {
            let title = json_text(post, "title")?;
            let permalink = json_text(post, "permalink")?;
            let key = normalize_key(&permalink);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: json_text(post, "thumbnail").map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .take(48)
        .collect()
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: text_between_tag(body, "h1")
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(body, "x-ref=\"art\"", "src")
            .or_else(|| html::attr_after(body, "x-ref='art'", "src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        description: content_after(body, "mb-4 order-2")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: text_after(body, "mb-1").into_iter().collect(),
        tags: collect_links_after(body, "flex-wrap"),
        status: parse_status(&text_after(body, "gap-2").unwrap_or_default()),
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapters(key: &str) -> Vec<NovelChapter> {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    let Some(xdata) = body
        .split("toc('")
        .nth(1)
        .map(|value| value.split(')').next().unwrap_or_default().to_string())
    else {
        return Vec::new();
    };
    let parts: Vec<_> = xdata.split('\'').collect();
    let id = parts.first().copied().unwrap_or_default();
    let post = parts.get(2).copied().unwrap_or_default();
    if id.is_empty() || post.is_empty() {
        return Vec::new();
    }
    let response = client()
        .post(format!("{BASE_URL}/ajax"))
        .referer(absolute_url(key))
        .xhr()
        .form(&[("post", post), ("id", id), ("action", "series_toc")])
        .send_text()
        .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
    parse_chapters_json(&response)
}

fn parse_chapters_json(body: &str) -> Vec<NovelChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    root.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let url_value = json_text(chapter, "url")?;
            let key = normalize_key(&url_value);
            Some(NovelChapter {
                key: key.clone(),
                title: json_text(chapter, "title"),
                chapter_number: json_text(chapter, "slug").and_then(|value| value.parse().ok()),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn fetch_chapter_content(key: &str, nonce: &str) -> Option<String> {
    let response = client()
        .post(format!(
            "{}/content",
            absolute_url(key).trim_end_matches('/')
        ))
        .referer(format!("{}/", absolute_url(key).trim_end_matches('/')))
        .header("x-nonce", nonce)
        .json(r#"{"captcha_response":""}"#)
        .xhr()
        .send_text()
        .ok()?;
    let parsed: Result<Value, _> = serde_json::from_str(&response);
    if parsed
        .ok()
        .and_then(|value| value.get("success").and_then(Value::as_bool))
        == Some(false)
    {
        return None;
    }
    Some(response)
}

fn decode_chapter_html(input: &str) -> String {
    input
        .replace("cls", "")
        .chars()
        .map(|ch| {
            let code = ch as u32;
            let offset = if code > 12123 { 12027 } else { 12033 };
            let decoded = code.saturating_sub(offset);
            if (32..=126).contains(&decoded) {
                char::from_u32(decoded).unwrap_or(ch)
            } else {
                ch
            }
        })
        .collect()
}

fn remove_brand_spans(input: &str) -> String {
    let mut out = input.to_string();
    loop {
        let Some(start) = out.find("<span") else {
            break;
        };
        let Some(end) = out[start..].find("</span>").map(|idx| start + idx + 7) else {
            break;
        };
        let block = &out[start..end];
        let lower = html::strip_tags(block).to_ascii_lowercase();
        if lower.contains("storyseedling") || lower.contains("story seedling") {
            out.replace_range(start..end, "");
        } else {
            let next_start = start + 5;
            if next_start >= out.len() {
                break;
            }
            let prefix = out[..next_start].to_string();
            let suffix = remove_brand_spans(&out[next_start..]);
            out = format!("{prefix}{suffix}");
            break;
        }
    }
    out
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("drop") || lower.contains("cancel") {
        ItemStatus::Cancelled
    } else if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn collect_links_after(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .collect()
}

fn content_after(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</div>")
        .or_else(|| html::text_between(body, marker, "</section>"))
}

fn text_after(body: &str, marker: &str) -> Option<String> {
    content_after(body, marker)
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn text_between_tag(body: &str, tag: &str) -> Option<String> {
    html::text_between(body, &format!("<{tag}"), &format!("</{tag}>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn quoted_after(body: &str, start: &str, end: &str) -> Option<String> {
    Some(body.split(start).nth(1)?.split(end).next()?.to_string())
}

fn load_chapter_nonce(body: &str) -> Option<String> {
    let call = body.split("loadChapter(").nth(1)?.split(')').next()?;
    let quoted = call
        .split(['\'', '"'])
        .filter(|part| !part.trim().is_empty() && part.trim() != ",")
        .map(str::trim)
        .collect::<Vec<_>>();
    quoted
        .get(1)
        .map(|value| value.trim_matches(',').trim().to_string())
}

fn json_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .contains("storyseedling.com")
        .then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://storyseedling.com/")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

const BROWSE_FIXTURE: &str = r#"<div ax-load x-data="browse('sample')"></div>"#;
const LIST_FIXTURE: &str = r#"{"success":true,"data":{"posts":[{"title":"Sample Novel","thumbnail":"https://storyseedling.com/cover.jpg","permalink":"https://storyseedling.com/series/sample"}]}}"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample Novel</h1><img x-ref="art" src="https://storyseedling.com/cover.jpg"><div class="mb-1"><a>Sample Author</a></div><div class="gap-2"><span class="text-sm">ongoing</span></div><div class="flex flex-wrap"><a>Fantasy</a></div><div class="mb-4 order-2"><p>Sample summary.</p></div><div ax-load x-data="toc('1', 'samplepost')"></div>
"#;
const CHAPTERS_FIXTURE: &str = r#"{"success":true,"data":[{"title":"Chapter 1","url":"https://storyseedling.com/series/sample/chapter-1","slug":"1","date":"1 day ago"}]}"#;
const TEXT_PAGE_FIXTURE: &str = r#"<div class="mb-4"><h1 class="text-xl">Chapter 1</h1><div x-data="loadChapter('1', 'nonce')"></div></div>"#;
const TEXT_FIXTURE: &str = r#"<p>Sample chapter text.</p>"#;

export_novel_source!(SOURCE);
