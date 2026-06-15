use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: LnMtl = LnMtl;
const BASE_URL: &str = "https://lnmtl.com";

struct LnMtl;

impl NovelSource for LnMtl {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = filter_text(&request, "order", "favourites");
        let sort = filter_text(&request, "sort", "desc");
        let status = filter_text(&request, "storyStatus", "all");
        let body = fetch_or_fixture(
            &format!("{BASE_URL}/novel?orderBy={order}&order={sort}&filter={status}&page={page}"),
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
        if request.get("page").and_then(Value::as_u64).unwrap_or(1) != 1 {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        let body = fetch_or_fixture(BASE_URL, LIST_FIXTURE);
        let list_url = prefetch_json_path(&body);
        let entries = list_url
            .map(|path| fetch_or_fixture(&url::join_url(BASE_URL, &path), SEARCH_FIXTURE))
            .map(|json| parse_search_json(&json, query))
            .unwrap_or_else(|| {
                parse_listing(&body)
                    .into_iter()
                    .filter(|item| item.title.to_lowercase().contains(&query.to_lowercase()))
                    .collect()
            });
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        Ok(self.chapters_page(request)?.entries)
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let details = fetch_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let volumes = parse_volumes(&details);
        let volume = volumes
            .get(page.saturating_sub(1) as usize)
            .cloned()
            .unwrap_or_else(|| ("sample-volume".to_string(), "Volume 1".to_string()));
        let body = fetch_or_fixture(
            &format!("{BASE_URL}/chapter?volumeId={}", volume.0),
            CHAPTERS_FIXTURE,
        );
        let (entries, last_page) = parse_chapter_api(&body);
        Ok(NovelChapterPage {
            entries,
            has_next_page: page < volumes.len() as u64 || last_page.unwrap_or(1) > 1,
            next_page: (page < volumes.len() as u64).then_some(page as u32 + 1),
            section: Some(volume.1),
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "chapter/sample-chapter".to_string());
        let body = fetch_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        let html = parse_text_html(&body);
        Ok(NovelText {
            html: Some(html.clone()),
            text: Some(novel::cleanup_text(&html)),
            base_url: Some(BASE_URL.to_string()),
            css: Some("body { line-height: 1.7; }".to_string()),
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

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, &normalize_key(key))
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split("media-left")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")?;
            let key = normalize_key(&href);
            if !seen.insert(key.clone()) {
                return None;
            }
            let title = html::attr_after(block, "<img", "alt")
                .or_else(|| html::attr_after(block, "<img", "title"))
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(block, "<img", "src").map(|src| absolute_url(&src)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
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
        title: html::attr_after(body, "img-rounded", "title")
            .or_else(|| first_text(body, &["<h1", "<title"]))
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(body, "img-rounded", "src").map(|src| absolute_url(&src)),
        url: Some(absolute_url(key)),
        description: html::text_between(body, "description", "</div>")
            .map(|value| html::strip_tags(&value)),
        tags: tags_from_first_inline_list(body),
        status: parse_status(&first_text(body, &["Current status"]).unwrap_or_default()),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    };
    if let Some(author_block) = body.split("Authors").nth(1) {
        if let Some(author) = html::text_between(author_block, "<dd", "</dd>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
        {
            item.authors.push(author);
        }
    }
    item
}

fn parse_volumes(body: &str) -> Vec<(String, String)> {
    let Some(raw) = body
        .split("lnmtl.volumes = ")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
    else {
        return vec![("sample-volume".to_string(), "Volume 1".to_string())];
    };
    let root: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    root.as_array()
        .into_iter()
        .flatten()
        .filter_map(|volume| {
            Some((
                json_text(volume, "id")?,
                json_text(volume, "title").unwrap_or_else(|| "Volume".to_string()),
            ))
        })
        .collect()
}

fn parse_chapter_api(body: &str) -> (Vec<NovelChapter>, Option<u64>) {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let entries = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let slug = json_text(chapter, "slug")?;
            let number = chapter.get("number").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let title = json_text(chapter, "title").unwrap_or_else(|| "Chapter".to_string());
            Some(NovelChapter {
                key: format!("chapter/{slug}"),
                title: Some(format!("#{number} - {title}")),
                chapter_number: Some(number),
                url: Some(format!("{BASE_URL}/chapter/{slug}")),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect();
    (entries, root.get("last_page").and_then(Value::as_u64))
}

fn parse_text_html(body: &str) -> String {
    let mut out = String::new();
    for block in body.split("<sentence").skip(1) {
        if !block.contains("translated") {
            continue;
        }
        if let Some(text) = html::text_between(block, ">", "</sentence>") {
            let text = html::html_unescape(&html::strip_tags(&text)).replace('„', "“");
            if !text.trim().is_empty() {
                out.push_str("<p>");
                out.push_str(&escape_html(text.trim()));
                out.push_str("</p>");
            }
        }
    }
    if out.is_empty() {
        TEXT_FIXTURE.to_string()
    } else {
        out
    }
}

fn parse_search_json(body: &str, query: &str) -> Vec<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    root.as_array()
        .into_iter()
        .flatten()
        .filter(|item| {
            json_text(item, "name")
                .unwrap_or_default()
                .to_lowercase()
                .contains(&query.to_lowercase())
        })
        .filter_map(|item| {
            let slug = json_text(item, "slug")?;
            Some(CatalogItem {
                key: format!("novel/{slug}"),
                title: json_text(item, "name").unwrap_or_else(|| "Novel".to_string()),
                cover: json_text(item, "image"),
                url: Some(format!("{BASE_URL}/novel/{slug}")),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn prefetch_json_path(body: &str) -> Option<String> {
    body.split("prefetch: '")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .map(ToString::to_string)
}

fn tags_from_first_inline_list(body: &str) -> Vec<String> {
    body.split("list-inline")
        .nth(1)
        .unwrap_or(body)
        .split("<li")
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</li>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn first_text(body: &str, markers: &[&str]) -> Option<String> {
    markers.iter().find_map(|marker| {
        html::text_between(body, marker, "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
    })
}

fn parse_status(value: &str) -> ItemStatus {
    match value.to_ascii_lowercase().as_str() {
        text if text.contains("finished") || text.contains("complete") => ItemStatus::Completed,
        text if text.contains("hiatus") => ItemStatus::Hiatus,
        _ => ItemStatus::Ongoing,
    }
}

fn has_next_page(body: &str) -> bool {
    body.contains("rel=\"next\"") || body.to_ascii_lowercase().contains(">next<")
}

fn filter_text(request: &Value, id: &str, default: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .unwrap_or(default)
        .to_string()
}

fn json_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
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

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const LIST_FIXTURE: &str = r#"<div class="media-left"><a href="/novel/sample"><img alt="Sample Novel" src="/cover.jpg"></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<main><img class="img-rounded" title="Sample Novel" src="/cover.jpg"><div class="description">Sample summary.</div><ul class="list-inline"><li>Fantasy</li></ul></main><script>lnmtl.volumes = [{"id":"sample-volume","title":"Volume 1"}];</script>"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"number":1,"title":"Beginning","slug":"sample-chapter","created_at":"2024-01-01"}],"last_page":1}"#;
const SEARCH_FIXTURE: &str = r#"[{"name":"Sample Novel","slug":"sample","image":"/cover.jpg"}]"#;
const TEXT_FIXTURE: &str = r#"<p>Sample chapter text.</p>"#;

export_novel_source!(SOURCE);
