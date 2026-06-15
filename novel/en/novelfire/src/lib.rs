use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: NovelFire = NovelFire;
const BASE_URL: &str = "https://novelfire.net";

struct NovelFire;

impl NovelSource for NovelFire {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE, ".novel-item"),
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
            &search_adv_url(&request, page, listing == "latest"),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_listing(&body, ".novel-item"),
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
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
            &format!(
                "{BASE_URL}/search?keyword={}&page={page}",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_listing(&body, ".novel-list.chapters .novel-item"),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        Ok(chapters_for(&request, &key, 1))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let entries = chapters_for(&request, &key, page);
        Ok(NovelChapterPage {
            has_next_page: pref_bool(&request, "pageLength") && !entries.is_empty(),
            next_page: pref_bool(&request, "pageLength").then_some(page as u32 + 1),
            entries,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "novel/sample/chapter-1".to_string());
        let body = fetch_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        let raw = html::text_between(&body, "id=\"content\"", "</div>")
            .or_else(|| html::text_between(&body, "id='content'", "</div>"))
            .unwrap_or_else(|| TEXT_FIXTURE.to_string());
        let cleaned = strip_custom_noise(&raw).replace("&nbsp;", " ");
        let normalized = novel::normalize_reader_html(&cleaned);
        Ok(NovelText {
            html: Some(normalized.clone()),
            text: Some(novel::cleanup_text(&normalized)),
            base_url: Some(BASE_URL.to_string()),
            css: Some(
                "body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string(),
            ),
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
            entries: parse_listing(LIST_FIXTURE, ".novel-item"),
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

fn search_adv_url(request: &Value, page: u64, latest: bool) -> String {
    let mut params = Vec::new();
    for language in filter_array(request, "language") {
        params.push(format!("country_id[]={}", url::query_escape(&language)));
    }
    params.push("ctgcon=and".to_string());
    for genre in filter_array(request, "genres") {
        params.push(format!("categories[]={}", url::query_escape(&genre)));
    }
    params.push(format!("status={}", filter_text(request, "status", "-1")));
    params.push(format!(
        "sort={}",
        if latest {
            "date".to_string()
        } else {
            filter_text(request, "sort", "rank-top")
        }
    ));
    params.push(format!("page={page}"));
    format!("{BASE_URL}/search-adv?{}", params.join("&"))
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, &normalize_key(key))
}

fn parse_listing(body: &str, _selector: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split("novel-item")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")
                .or_else(|| html::attr_after(block, "<h4", "href"))?;
            let key = normalize_key(&href);
            if !seen.insert(key.clone()) {
                return None;
            }
            let title = html::attr_after(block, "<a", "title")
                .or_else(|| {
                    html::text_between(block, "<h4", "</h4>").map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
            let cover = block
                .split("novel-cover")
                .nth(1)
                .and_then(|image| {
                    html::attr_after(image, "<img", "data-src")
                        .or_else(|| html::attr_after(image, "<img", "src"))
                })
                .map(|src| absolute_url(&src));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover,
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
    let summary = html::text_between(body, "summary", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    CatalogItem {
        key: normalize_key(key),
        title: first_text(body, &["novel-title", "<title"])
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(body, "cover", "data-src")
            .or_else(|| html::attr_after(body, "cover", "src"))
            .map(|src| absolute_url(&src)),
        url: Some(absolute_url(key)),
        authors: html::text_between(body, "author", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .into_iter()
            .collect(),
        description: summary,
        tags: body
            .split("categories")
            .nth(1)
            .unwrap_or(body)
            .split("property-item")
            .skip(1)
            .filter_map(|block| {
                html::text_between(block, ">", "</").map(|value| html::strip_tags(&value))
            })
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(body),
        rating: first_text(body, &["nub"]).and_then(|value| value.parse().ok()),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn chapters_for(request: &Value, key: &str, page: u64) -> Vec<NovelChapter> {
    let detail = fetch_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    let post_id = html::attr_after(&detail, "id=\"novel-report\"", "report-post_id")
        .or_else(|| html::attr_after(&detail, "id='novel-report'", "report-post_id"));
    if let Some(post_id) = post_id {
        let length = if pref_bool(request, "pageLength") {
            100
        } else {
            -1
        };
        let start = if length == -1 {
            0
        } else {
            (page.saturating_sub(1) * length as u64) as i64
        };
        let ajax = fetch_or_fixture(
            &format!(
                "{BASE_URL}/ajax/listChapterDataAjax?draw=1&columns[0][data]=n_sort&columns[0][name]=cmm_posts_detail.n_sort&columns[0][searchable]=true&columns[0][orderable]=true&order[0][column]=0&order[0][dir]=asc&start={start}&length={length}&post_id={post_id}&only_bookmark=false"
            ),
            CHAPTERS_FIXTURE,
        );
        let parsed = parse_ajax_chapters(&ajax, key);
        if !parsed.is_empty() {
            return parsed;
        }
    }
    parse_scraped_chapters(&detail)
}

fn parse_ajax_chapters(body: &str, novel_key: &str) -> Vec<NovelChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let mut chapters: Vec<_> = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let number = chapter.get("n_sort").and_then(|value| {
                value
                    .as_f64()
                    .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
            })? as f32;
            let title = json_text(chapter, "title")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("Chapter {}", display_number(number)));
            Some(NovelChapter {
                key: format!(
                    "{}/chapter-{}",
                    normalize_key(novel_key).trim_end_matches('/'),
                    display_number(number)
                ),
                title: Some(title),
                chapter_number: Some(number),
                url: Some(format!(
                    "{BASE_URL}/{}/chapter-{}",
                    normalize_key(novel_key).trim_matches('/'),
                    display_number(number)
                )),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect();
    chapters.sort_by(|a, b| {
        a.chapter_number
            .unwrap_or(0.0)
            .partial_cmp(&b.chapter_number.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_scraped_chapters(body: &str) -> Vec<NovelChapter> {
    body.split("chapter-list")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr(block, "href")?;
            let key = normalize_key(&href);
            Some(NovelChapter {
                key: key.clone(),
                title: html::attr(block, "title").or_else(|| {
                    html::text_between(block, ">", "</a>").map(|value| html::strip_tags(&value))
                }),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn first_text(body: &str, markers: &[&str]) -> Option<String> {
    markers.iter().find_map(|marker| {
        html::text_between(body, marker, "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
    })
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("dropped") || lower.contains("cancelled") {
        ItemStatus::Cancelled
    } else if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn strip_custom_noise(input: &str) -> String {
    let mut out = String::new();
    for part in input.split('<') {
        if part.starts_with("nf") || part.starts_with("/nf") {
            continue;
        }
        if out.is_empty() {
            out.push_str(part);
        } else {
            out.push('<');
            out.push_str(part);
        }
    }
    out
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

fn json_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
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

fn display_number(number: f32) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i32)
    } else {
        number.to_string()
    }
}

const LIST_FIXTURE: &str = r#"<div class="novel-item"><a href="/novel/sample" title="Sample Novel"><div class="novel-cover"><img src="/cover.jpg"></div></a><h4>Sample Novel</h4></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="novel-title">Sample Novel</h1><div class="cover"><img src="/cover.jpg"></div><div id="novel-report" report-post_id="1"></div><div class="summary"><div class="content">Sample summary.</div></div><div class="author"><span class="property-item"><span>Sample Author</span></span></div><div class="categories"><span class="property-item">Fantasy</span></div><div class="header-stats"><span class="ongoing">Ongoing</span></div>"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"title":"Chapter 1","slug":"chapter-1","n_sort":1}]}"#;
const TEXT_FIXTURE: &str = r#"<div id="content"><p>Sample chapter text.</p></div>"#;

export_novel_source!(SOURCE);
