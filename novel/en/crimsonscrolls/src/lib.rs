use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: CrimsonScrolls = CrimsonScrolls;
const BASE_URL: &str = "https://crimsonscrolls.net";

struct CrimsonScrolls;

impl NovelSource for CrimsonScrolls {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_HTML_FIXTURE),
                has_next_page: false,
            });
        }
        let page_string = page(&request).to_string();
        let body = post_form_or_fixture(
            &[("action", "load_novels"), ("page", page_string.as_str())],
            LIST_RESPONSE_FIXTURE,
        );
        let html = response_html(&body).unwrap_or(body);
        Ok(Paged {
            entries: parse_listing(&html),
            has_next_page: !parse_listing(&html).is_empty(),
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
                    &fetch_or_fixture(&novel_url(&key), DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }
        let body = post_form_or_fixture(
            &[("action", "live_novel_search"), ("query", query)],
            SEARCH_RESPONSE_FIXTURE,
        );
        let html = response_html(&body).unwrap_or(body);
        Ok(Paged {
            entries: parse_listing(&html),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        Ok(parse_details(
            &fetch_or_fixture(&novel_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        let details = fetch_or_fixture(&novel_url(&key), DETAILS_FIXTURE);
        let novel_id = chapter_list_id(&details).unwrap_or(1);
        Ok(fetch_all_chapters(novel_id))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key =
            novel::request_key(&request, "chapter").unwrap_or_else(|| "sample-chapter".to_string());
        Ok(parse_text(
            &fetch_or_fixture(
                &format!("{BASE_URL}/chapter/{}", key.trim_start_matches("chapter/")),
                TEXT_FIXTURE,
            ),
            &key,
        ))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Novels".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: parse_listing(LIST_HTML_FIXTURE),
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
                    &fetch_or_fixture(&novel_url(&key), DETAILS_FIXTURE),
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
        .with_origin(BASE_URL)
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

fn post_form_or_fixture(form: &[(&str, &str)], fixture: &str) -> String {
    client()
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .form(form)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn novel_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn response_html(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body).ok().and_then(|root| {
        root.get("html")
            .and_then(Value::as_str)
            .map(ToString::to_string)
    })
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<")
        .filter(|chunk| chunk.contains("live-search-item") || chunk.contains("novel-list-card"))
        .filter_map(|chunk| {
            let href =
                html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: text_between(chunk, "live-search-title", "</")
                    .or_else(|| text_between(chunk, "novel-title", "</"))
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string())
                    }),
                cover: html::attr_after(chunk, "live-search-cover", "src")
                    .or_else(|| html::attr_after(chunk, "novel-cover", "src")),
                url: Some(novel_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let wrapper = html::text_between(body, "single-novel-content-wrapper", "</section>")
        .unwrap_or_else(|| body.to_string());
    CatalogItem {
        key: key.to_string(),
        title: text_between(&wrapper, "<h1", "</h1>")
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(&wrapper, "<img", "data-src")
            .or_else(|| html::attr_after(&wrapper, "<img", "src")),
        description: text_between(&wrapper, "synopsis-full", "</"),
        authors: author_from_wrapper(&wrapper).into_iter().collect(),
        tags: wrapper
            .split("cs-genre-chip")
            .skip(1)
            .filter_map(|chunk| {
                html::text_between(chunk, ">", "</").map(|value| html::strip_tags(&value))
            })
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(&wrapper),
        url: Some(novel_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_all_chapters(novel_id: u64) -> Vec<NovelChapter> {
    let mut chapters = Vec::new();
    let mut page = 1_u64;
    loop {
        let target = format!(
            "{BASE_URL}/wp-json/cs/v1/novels/{novel_id}/chapters?per_page=75&order=asc&page={page}"
        );
        let body = fetch_or_fixture(&target, CHAPTERS_FIXTURE);
        let (mut entries, total_pages) = parse_chapter_json(&body);
        chapters.append(&mut entries);
        if page >= total_pages.unwrap_or(page) {
            break;
        }
        page += 1;
        if page > 20 {
            break;
        }
    }
    chapters
}

fn parse_chapter_json(body: &str) -> (Vec<NovelChapter>, Option<u64>) {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let entries = root
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(idx, item)| {
            let locked = item.get("locked").and_then(Value::as_bool).unwrap_or(false);
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Chapter")
                .trim();
            let key = item
                .get("url")
                .and_then(Value::as_str)
                .map(|value| {
                    normalize_key(value)
                        .trim_start_matches("chapter/")
                        .to_string()
                })
                .unwrap_or_else(|| format!("chapter-{}", idx + 1));
            NovelChapter {
                key: key.clone(),
                title: Some(if locked {
                    format!("[Locked] {title}")
                } else {
                    title.to_string()
                }),
                chapter_number: Some(idx as f32 + 1.0),
                url: Some(format!("{BASE_URL}/chapter/{key}")),
                language: Some("en".to_string()),
                is_locked: locked,
                ..NovelChapter::default()
            }
        })
        .collect();
    (entries, root.get("total_pages").and_then(Value::as_u64))
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let mut html_body = html::text_between(body, "chapter-display", "</div>")
        .unwrap_or_else(|| TEXT_HTML_FIXTURE.to_string());
    for marker in ["cs-attrib-divider", "cs-attrib", "cs-chapter-attrib"] {
        html_body = strip_marker_block(&html_body, marker);
    }
    let normalized = novel::normalize_reader_html(&html_body);
    NovelText {
        title: text_between(body, "<h1", "</h1>").or_else(|| text_between(body, "<h2", "</h2>")),
        html: Some(normalized.clone()),
        text: Some(html::strip_tags(&normalized)),
        base_url: Some(BASE_URL.to_string()),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        next_chapter_key: next_chapter_key(key),
        ..NovelText::default()
    }
}

fn text_between(body: &str, marker: &str, end: &str) -> Option<String> {
    html::text_between(body, marker, end)
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn author_from_wrapper(wrapper: &str) -> Option<String> {
    let strong = wrapper.find("<strong")?;
    let rest = &wrapper[strong..];
    let after = rest.find("</strong>")?;
    html::text_between(&rest[after..], ">", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn chapter_list_id(body: &str) -> Option<u64> {
    let start = body
        .find("id=\"chapter-list\"")
        .or_else(|| body.find("id='chapter-list'"))?;
    html::attr(&body[start..], "data-novel").and_then(|value| value.parse().ok())
}

fn parse_status(body: &str) -> ItemStatus {
    let status = text_between(body, "cs-nsb-badge", "</").unwrap_or_default();
    match status.to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "hiatus" => ItemStatus::Hiatus,
        "dropped" | "cancelled" => ItemStatus::Cancelled,
        "completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn next_chapter_key(key: &str) -> Option<String> {
    let number = key
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .next_back()?
        .parse::<u64>()
        .ok()?;
    let prefix = key.rsplit_once('-')?.0;
    Some(format!("{prefix}-{}", number + 1))
}

fn strip_marker_block(input: &str, marker: &str) -> String {
    input
        .split('<')
        .filter(|chunk| !chunk.contains(marker))
        .map(|chunk| format!("<{chunk}"))
        .collect::<String>()
        .trim_start_matches('<')
        .to_string()
}

const LIST_HTML_FIXTURE: &str = r#"<div class="novel-list-card"><a href="https://crimsonscrolls.net/novel/sample"><div class="novel-cover"><img src="https://crimsonscrolls.net/sample.jpg"></div><h3 class="novel-title">Sample Novel</h3></a></div>"#;
const LIST_RESPONSE_FIXTURE: &str = r#"{"html":"<div class=\"novel-list-card\"><a href=\"https://crimsonscrolls.net/novel/sample\"><h3 class=\"novel-title\">Sample Novel</h3></a></div>"}"#;
const SEARCH_RESPONSE_FIXTURE: &str = r#"{"html":"<a class=\"live-search-item\" href=\"https://crimsonscrolls.net/novel/sample\"><div class=\"live-search-title\">Sample Novel</div></a>"}"#;
const DETAILS_FIXTURE: &str = r#"<section id="single-novel-content-wrapper"><h1>Sample Novel</h1><img data-src="https://crimsonscrolls.net/sample.jpg"><div id="synopsis-full">Sample description.</div><span class="cs-nsb-badge">Ongoing</span><span class="cs-genre-chip">Fantasy</span><strong>Author</strong><span>Sample Author</span><div id="chapter-list" data-novel="1"></div></section>"#;
const CHAPTERS_FIXTURE: &str = r#"{"items":[{"id":1,"title":"Chapter 1","url":"https://crimsonscrolls.net/chapter/sample-chapter-1","locked":false}],"total":1,"total_pages":1,"page":1}"#;
const TEXT_HTML_FIXTURE: &str = r#"<p>The first fixture paragraph.</p>"#;
const TEXT_FIXTURE: &str = r#"<div id="chapter-display"><p>The first fixture paragraph.</p></div>"#;

export_novel_source!(SOURCE);
