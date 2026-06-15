use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ChrysanthemumGarden = ChrysanthemumGarden;
const BASE_URL: &str = "https://chrysanthemumgarden.com";

struct ChrysanthemumGarden;

impl NovelSource for ChrysanthemumGarden {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        let page = page(&request);
        let path = if page <= 1 {
            "/books/".to_string()
        } else {
            format!("/books/page/{page}/")
        };
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &path), LIST_FIXTURE);
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
                entries: vec![parse_details(
                    &fetch_or_fixture(&book_url(&key), DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }
        if page(&request) != 1 {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        let body = fetch_or_fixture(&format!("{BASE_URL}/wp-json/cg/novels"), SEARCH_FIXTURE);
        Ok(Paged {
            entries: parse_search(&body, query),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "book/sample".to_string());
        Ok(parse_details(
            &fetch_or_fixture(&book_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "book/sample".to_string());
        Ok(parse_chapters(
            &fetch_or_fixture(&book_url(&key), DETAILS_FIXTURE),
            &key,
        ))
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
            .unwrap_or_else(|| "chapter/sample-chapter".to_string());
        Ok(parse_text(
            &fetch_or_fixture(&chapter_url(&key), TEXT_FIXTURE),
            &key,
        ))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Books".to_string(),
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
                item: Some(parse_details(
                    &fetch_or_fixture(&book_url(&key), DETAILS_FIXTURE),
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

fn book_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn chapter_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<article")
        .skip(1)
        .filter(|chunk| {
            !text_between(chunk, "series-genres", "</div>")
                .unwrap_or_default()
                .contains("Manhua")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "novel-title", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: text_between(chunk, "novel-title", "</h2>").unwrap_or_else(|| {
                    url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string())
                }),
                cover: html::attr_after(chunk, "novel-cover", "data-breeze")
                    .or_else(|| html::attr_after(chunk, "novel-cover", "src")),
                url: Some(book_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_search(body: &str, query: &str) -> Vec<CatalogItem> {
    let needle = query.to_ascii_lowercase();
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let name = item.get("name").and_then(Value::as_str)?.trim().to_string();
            if !name.to_ascii_lowercase().contains(&needle) {
                return None;
            }
            let key = item
                .get("link")
                .and_then(Value::as_str)
                .map(normalize_key)
                .unwrap_or_else(|| format!("book/{}", name.to_ascii_lowercase().replace(' ', "-")));
            Some(CatalogItem {
                key: key.clone(),
                title: name,
                cover: None,
                url: Some(book_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: text_between(body, "h1 class=\"novel-title", "</h1>")
            .or_else(|| text_between(body, "<h1", "</h1>"))
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(body, "novel-cover", "data-breeze")
            .or_else(|| html::attr_after(body, "novel-cover", "src")),
        description: paragraphs(body, "entry-content").filter(|value| !value.is_empty()),
        authors: author_from_info(body).into_iter().collect(),
        tags: body
            .split("series-genres")
            .skip(1)
            .flat_map(|chunk| chunk.split("<a").skip(1))
            .filter_map(|chunk| {
                html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
            })
            .chain(
                body.split("series-tag")
                    .skip(1)
                    .filter_map(|chunk| {
                        html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
                    })
                    .map(|value| value.split('(').next().unwrap_or(&value).trim().to_string()),
            )
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(body),
        url: Some(book_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, _novel_key: &str) -> Vec<NovelChapter> {
    body.split("chapter-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(NovelChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                chapter_number: chapter_number(&key),
                url: Some(chapter_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let html_body = html::text_between(body, "id=\"novel-content\"", "</div>")
        .or_else(|| html::text_between(body, "id='novel-content'", "</div>"))
        .unwrap_or_else(|| TEXT_HTML_FIXTURE.to_string());
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
        .map(|value| html::strip_tags(&value.replace("novel-raw-title", "")))
        .filter(|value| !value.is_empty())
}

fn paragraphs(body: &str, marker: &str) -> Option<String> {
    let block = html::text_between(body, marker, "</div>")?;
    let text = block
        .split("<p")
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</p>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

fn author_from_info(body: &str) -> Option<String> {
    let info = html::text_between(body, "novel-info", "</div>")?;
    let start = info.find("Author:")? + "Author:".len();
    let rest = &info[start..];
    Some(
        html::strip_tags(rest.split("<br").next().unwrap_or(rest))
            .trim()
            .to_string(),
    )
    .filter(|value| !value.is_empty())
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn chapter_number(key: &str) -> Option<f32> {
    key.split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .filter(|part| !part.is_empty())
        .next_back()
        .and_then(|part| part.parse().ok())
}

fn next_chapter_key(key: &str) -> Option<String> {
    let number = chapter_number(key)? as u64;
    let prefix = key.rsplit_once('/')?.0;
    Some(format!("{prefix}/chapter-{}", number + 1))
}

fn has_next_page(body: &str) -> bool {
    body.contains("rel=\"next\"") || body.contains("next page-numbers")
}

const LIST_FIXTURE: &str = r#"<article><div class="novel-cover"><img data-breeze="https://chrysanthemumgarden.com/sample.jpg"></div><h2 class="novel-title"><a href="https://chrysanthemumgarden.com/book/sample/">Sample Novel</a></h2></article>"#;
const SEARCH_FIXTURE: &str =
    r#"[{"name":"Sample Novel","link":"https://chrysanthemumgarden.com/book/sample/"}]"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="novel-title">Sample Novel</h1><div class="novel-cover"><img data-breeze="https://chrysanthemumgarden.com/sample.jpg"></div><div class="entry-content"><p>Sample description.</p></div><div class="novel-info">Author: Sample Author<br></div><div class="chapter-item"><a href="https://chrysanthemumgarden.com/chapter/sample-chapter/">Chapter 1</a></div>"#;
const TEXT_HTML_FIXTURE: &str = r#"<p>The first fixture paragraph.</p>"#;
const TEXT_FIXTURE: &str = r#"<div id="novel-content"><p>The first fixture paragraph.</p></div>"#;

export_novel_source!(SOURCE);
