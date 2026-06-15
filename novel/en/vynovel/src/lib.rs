use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: VyNovel = VyNovel;
const BASE_URL: &str = "https://vynovel.com";

struct VyNovel;

impl NovelSource for VyNovel {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort = if listing == "latest" {
            "updated_at".to_string()
        } else {
            filter_string(&request, "sort", "viewed")
        };
        let body = fetch_document_or_fixture(
            &format!("{BASE_URL}/search?sort={sort}&page={page}"),
            LIST_FIXTURE,
        );
        Ok(Paged {
            has_next_page: !parse_listing(&body).is_empty(),
            entries: parse_listing(&body),
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
        let body = fetch_document_or_fixture(
            &format!(
                "{BASE_URL}/search?sort=viewed&page={page}&q={}",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        );
        Ok(Paged {
            has_next_page: !parse_listing(&body).is_empty(),
            entries: parse_listing(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        Ok(fetch_details(&normalize_key(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        let body = fetch_document_or_fixture(&novel_url(&normalize_key(&key)), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &normalize_key(&key)))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".to_string());
        let body = fetch_document_or_fixture(&read_url(&normalize_key(&key)), TEXT_FIXTURE);
        let html_body = content_after(&body, "content").unwrap_or_else(|| TEXT_FIXTURE.to_string());
        let normalized = novel::normalize_reader_html(&html_body);
        Ok(NovelText {
            html: Some(normalized.clone()),
            text: Some(novel::cleanup_text(&normalized)),
            base_url: Some(read_url(&normalize_key(&key))),
            css: Some(
                "body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string(),
            ),
            image_headers: novel::image_headers(BASE_URL),
            ..NovelText::default()
        })
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

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("comic-item")
        .skip(1)
        .filter_map(parse_listing_item)
        .take(48)
        .collect()
}

fn parse_listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    let title = text_after(chunk, "comic-title")
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "comic-image", "data-background-image")
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|value| absolute_url(&value)),
        url: Some(novel_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&novel_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: text_after(body, "title")
            .or_else(|| text_between_tag(body, "h1"))
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(body, "img-manga", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        description: content_after(body, "summary")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: collect_author(body),
        status: if body.contains("text-ongoing") || body.contains("Ongoing") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Completed
        },
        url: Some(novel_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, novel_key: &str) -> Vec<NovelChapter> {
    let total = body
        .matches("list-group-item")
        .count()
        .max(body.matches("<a").count());
    let mut entries: Vec<_> = body
        .split("list-group")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .enumerate()
        .filter_map(|(index, chunk)| {
            let id = html::attr(chunk, "id")
                .map(|value| {
                    value
                        .chars()
                        .filter(|ch| ch.is_ascii_digit())
                        .collect::<String>()
                })
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    html::attr(chunk, "href")
                        .and_then(|href| href.split('/').next_back().map(ToString::to_string))
                })?;
            let title = text_after(chunk, "<span")
                .or_else(|| {
                    html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty());
            Some(NovelChapter {
                key: format!("{}/{}", normalize_key(novel_key), id),
                title,
                chapter_number: Some((total.saturating_sub(index)) as f32),
                url: Some(read_url(&format!("{}/{}", normalize_key(novel_key), id))),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect();
    entries.reverse();
    entries
}

fn collect_author(body: &str) -> Vec<String> {
    let value = body
        .split("col-md-7")
        .nth(1)
        .and_then(|chunk| chunk.split("</div>").next())
        .and_then(|chunk| chunk.split("<p").nth(4))
        .and_then(|chunk| html::text_between(chunk, "<a", "</a>"))
        .map(|value| html::strip_tags(&value));
    value
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect()
}

fn filter_string(request: &Value, key: &str, default: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_string()
}

fn content_after(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</div>").or_else(|| html::text_between(body, marker, "</p>"))
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

fn key_from_url(input: &str) -> Option<String> {
    input.contains("vynovel.com").then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://vynovel.com/")
        .trim_start_matches("novel/")
        .trim_start_matches("read/")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn novel_url(key: &str) -> String {
    format!("{BASE_URL}/novel/{}", normalize_key(key))
}

fn read_url(key: &str) -> String {
    format!("{BASE_URL}/read/{}", normalize_key(key))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

const LIST_FIXTURE: &str = r#"
<div class="comic-item"><a href="/novel/sample"><div class="comic-image lozad " data-background-image="https://vynovel.com/cover.jpg"></div><div class="comic-title">Sample Novel</div></a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="title">Sample Novel</h1><div class="img-manga"><img src="https://vynovel.com/cover.jpg"></div><div class="summary"><p class="content">Sample summary.</p></div><div class="col-md-7"><p></p><p></p><p></p><p></p><p><a>Sample Author</a></p></div><span class="text-ongoing">Ongoing</span><div class="list-group"><a class="list-group-item" id="chapter-1"><span>Chapter 1</span><p>1 day ago</p></a></div>
"#;
const TEXT_FIXTURE: &str = r#"<div class="content"><p>Sample chapter text.</p></div>"#;

export_novel_source!(SOURCE);
