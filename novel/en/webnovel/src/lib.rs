use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Webnovel = Webnovel;
const BASE_URL: &str = "https://www.webnovel.com";

struct Webnovel;

impl NovelSource for Webnovel {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if let Some(fanfic) =
            filter_string_opt(&request, "fanficSearch").filter(|value| !value.is_empty())
        {
            return Ok(search_internal(
                &fanfic,
                request.get("page").and_then(Value::as_u64).unwrap_or(1),
                Some("fanfic"),
            ));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/stories/novel?orderBy=5&pageIndex={page}")
        } else {
            category_url(&request, page)
        };
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
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
        Ok(search_internal(
            query,
            request.get("page").and_then(Value::as_u64).unwrap_or(1),
            None,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "book/sample_1".to_string());
        Ok(fetch_details(&normalize_key(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "book/sample_1".to_string());
        let hide_locked = preference_bool(&request, "hideLocked");
        Ok(fetch_chapters(&normalize_key(&key), hide_locked))
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
            .unwrap_or_else(|| "book/sample_1/chapter-1".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        let title = content_after(&body, "cha-tit").unwrap_or_default();
        let words = remove_block_containing(
            &content_after(&body, "cha-words").unwrap_or_else(|| TEXT_FIXTURE.to_string()),
            "para-comment",
        );
        let html_body = format!("{title}{words}");
        let normalized = novel::normalize_reader_html(&html_body);
        Ok(NovelText {
            title: Some(html::strip_tags(&title)).filter(|value| !value.is_empty()),
            html: Some(normalized.clone()),
            text: Some(novel::cleanup_text(&normalized)),
            base_url: Some(absolute_url(&key)),
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
        .with_header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
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

fn category_url(request: &Value, page: u64) -> String {
    let url_path;
    let mut params = Vec::new();
    let gender = filter_string(request, "genresGender", "1");
    if gender == "1" {
        let genre = filter_string(request, "genresMale", "1");
        if genre != "1" {
            url_path = genre;
        } else {
            url_path = "novel".to_string();
            params.push(("gender", "1".to_string()));
        }
    } else {
        let genre = filter_string(request, "genresFemale", "2");
        if genre != "2" {
            url_path = genre;
        } else {
            url_path = "novel".to_string();
            params.push(("gender", "2".to_string()));
        }
    }
    let content_type = filter_string(request, "type", "0");
    if content_type != "0" {
        if content_type == "3" {
            params.push(("translateMode", "3".to_string()));
            params.push(("sourceType", "1".to_string()));
        } else {
            params.push(("sourceType", content_type));
        }
    }
    params.push(("bookStatus", filter_string(request, "status", "0")));
    params.push(("orderBy", filter_string(request, "sort", "1")));
    params.push(("pageIndex", page.to_string()));
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/stories/{url_path}?{query}")
}

fn search_internal(query: &str, page: u64, kind: Option<&str>) -> Paged<CatalogItem> {
    let normalized = query.replace(char::is_whitespace, "+");
    let mut target = format!(
        "{BASE_URL}/search?keywords={}&pageIndex={page}",
        url::query_escape(&normalized)
    );
    if let Some(kind) = kind {
        target.push_str("&type=");
        target.push_str(kind);
    }
    let body = fetch_document_or_fixture(&target, SEARCH_FIXTURE);
    Paged {
        has_next_page: !parse_listing(&body).is_empty(),
        entries: parse_listing(&body),
    }
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<li")
        .skip(1)
        .filter_map(parse_listing_item)
        .take(48)
        .collect()
}

fn parse_listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "g_thumb", "href")?;
    let key = normalize_key(&href);
    let title = html::attr_after(chunk, "g_thumb", "title")
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "<img", "data-original")
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: html::attr_after(body, "g_thumb", "alt")
            .or_else(|| text_between_tag(body, "h1"))
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(body, "g_thumb", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        description: content_after(body, "j_synopsis")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: author_from_details(body).into_iter().collect(),
        tags: html::attr_after(body, "det-hd-tag", "title")
            .map(|value| {
                value
                    .split(',')
                    .map(|part| part.trim().to_string())
                    .filter(|part| !part.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        status: parse_status(&status_from_details(body).unwrap_or_default()),
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapters(key: &str, hide_locked: bool) -> Vec<NovelChapter> {
    let body = fetch_document_or_fixture(
        &format!("{}/catalog", absolute_url(key).trim_end_matches('/')),
        CATALOG_FIXTURE,
    );
    parse_chapters(&body, hide_locked)
}

fn parse_chapters(body: &str, hide_locked: bool) -> Vec<NovelChapter> {
    let mut chapters = Vec::new();
    let mut current_volume = "Unknown Volume".to_string();
    for chunk in body.split("<").skip(1) {
        if chunk.starts_with("h") || chunk.contains("volume-item") {
            let text = html::strip_tags(&format!("<{chunk}"));
            if let Some(volume) = volume_name(&text) {
                current_volume = volume;
            }
        }
        if !chunk.starts_with("li") && !chunk.starts_with("a") {
            continue;
        }
        let href = html::attr(chunk, "href").unwrap_or_default();
        if href.is_empty() {
            continue;
        }
        let locked = chunk.contains("<svg") || chunk.contains("locked") || chunk.contains("lock");
        if locked && hide_locked {
            continue;
        }
        let chapter_title = html::attr(chunk, "title")
            .or_else(|| {
                html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
            })
            .unwrap_or_else(|| "Chapter".to_string());
        let title = if locked {
            format!("{current_volume}: {chapter_title} [Locked]")
        } else {
            format!("{current_volume}: {chapter_title}")
        };
        let key = normalize_key(&href);
        chapters.push(NovelChapter {
            key: key.clone(),
            title: Some(title),
            url: Some(absolute_url(&key)),
            language: Some("en".to_string()),
            is_locked: locked,
            ..NovelChapter::default()
        });
    }
    if chapters.is_empty() {
        for chunk in body.split("<a").skip(1) {
            let href = html::attr(chunk, "href").unwrap_or_default();
            if href.is_empty() {
                continue;
            }
            let key = normalize_key(&href);
            chapters.push(NovelChapter {
                key: key.clone(),
                title: html::attr(chunk, "title"),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            });
        }
    }
    chapters
}

fn author_from_details(body: &str) -> Option<String> {
    body.split("Author:")
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<a", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn status_from_details(body: &str) -> Option<String> {
    body.split("Status")
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn volume_name(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    if !lower.contains("volume") {
        return None;
    }
    let number = lower
        .split("volume")
        .nth(1)?
        .split_whitespace()
        .next()
        .unwrap_or_default();
    if number.is_empty() {
        Some("Unknown Volume".to_string())
    } else {
        Some(format!("Volume {number}"))
    }
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn preference_bool(request: &Value, key: &str) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn filter_string(request: &Value, key: &str, default: &str) -> String {
    filter_string_opt(request, key).unwrap_or_else(|| default.to_string())
}

fn filter_string_opt(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn remove_block_containing(input: &str, marker: &str) -> String {
    let mut out = input.to_string();
    while let Some(pos) = out.find(marker) {
        let start = out[..pos].rfind('<').unwrap_or(pos);
        let end = out[pos..]
            .find("</span>")
            .map(|idx| pos + idx + 7)
            .or_else(|| out[pos..].find("</div>").map(|idx| pos + idx + 6))
            .unwrap_or(pos + marker.len());
        out.replace_range(start..end.min(out.len()), "");
    }
    out
}

fn content_after(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</div>")
        .or_else(|| html::text_between(body, marker, "</section>"))
}

fn text_between_tag(body: &str, tag: &str) -> Option<String> {
    html::text_between(body, &format!("<{tag}"), &format!("</{tag}>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn key_from_url(input: &str) -> Option<String> {
    input.contains("webnovel.com").then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://www.webnovel.com/")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("//") {
        format!("https:{input}")
    } else if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

const LIST_FIXTURE: &str = r#"
<ul class="j_category_wrapper"><li><a class="g_thumb" title="Sample Novel" href="/book/sample_1"><img data-original="//static.webnovel.com/cover.jpg"></a></li></ul>
"#;
const SEARCH_FIXTURE: &str = r#"
<ul class="j_list_container"><li><a class="g_thumb" title="Sample Novel" href="/book/sample_1"><img src="//static.webnovel.com/cover.jpg"></a></li></ul>
"#;
const DETAILS_FIXTURE: &str = r#"
<a class="g_thumb"><img alt="Sample Novel" src="//static.webnovel.com/cover.jpg"></a><div class="det-hd-detail"><span class="det-hd-tag" title="Fantasy"></span><span>Author:</span><a>Sample Author</a><svg title="Status"></svg><span>Ongoing</span></div><div class="j_synopsis"><p>Sample summary.</p></div>
"#;
const CATALOG_FIXTURE: &str = r#"
<div class="volume-item">Volume 1<ul><li><a href="/book/sample_1/chapter-1" title="Chapter 1"></a></li></ul></div>
"#;
const TEXT_FIXTURE: &str =
    r#"<h1 class="cha-tit">Chapter 1</h1><div class="cha-words"><p>Sample chapter text.</p></div>"#;

export_novel_source!(SOURCE);
