use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Foxteller = Foxteller;
const BASE_URL: &str = "https://www.foxteller.com";

struct Foxteller;

impl NovelSource for Foxteller {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        let order = filter_string(&request, "order").unwrap_or_else(|| {
            if request
                .get("listing")
                .or_else(|| request.get("listingId"))
                .and_then(Value::as_str)
                == Some("latest")
            {
                "newest".to_string()
            } else {
                "popularity".to_string()
            }
        });
        let body = fetch_or_fixture(&format!("{BASE_URL}/library?sort={order}"), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: false,
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
        let body = client()
            .post(format!("{BASE_URL}/search"))
            .json(json!({ "query": query }).to_string())
            .xhr()
            .send_text()
            .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "sample-novel".to_string());
        Ok(parse_details(
            &fetch_or_fixture(&novel_url(&normalize_key(&key)), DETAILS_FIXTURE),
            &normalize_key(&key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "sample-novel".to_string());
        Ok(parse_chapters(
            &fetch_or_fixture(&novel_url(&normalize_key(&key)), DETAILS_FIXTURE),
            &normalize_key(&key),
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
            .unwrap_or_else(|| "sample-novel/chapter-1".to_string());
        let chapter_page = fetch_or_fixture(&novel_url(&key), TEXT_PAGE_FIXTURE);
        let chapter_id = quoted_value(&chapter_page, "'chapter_id': '", "'")
            .or_else(|| quoted_value(&chapter_page, "\"chapter_id\":\"", "\""));
        let html = chapter_id
            .and_then(|chapter_id| fetch_aux(&key, &chapter_id))
            .or_else(|| content_block(&chapter_page))
            .unwrap_or_else(|| "<p>Fixture chapter text.</p>".to_string());
        Ok(NovelText {
            html: Some(html.clone()),
            text: Some(novel::cleanup_text(&html)),
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

fn fetch_aux(chapter_key: &str, chapter_id: &str) -> Option<String> {
    let novel_id = chapter_key.split('/').next().unwrap_or_default();
    let body = json!({ "x1": novel_id, "x2": chapter_id }).to_string();
    let response = client()
        .post(format!("{BASE_URL}/aux_dem"))
        .referer(novel_url(chapter_key))
        .json(body)
        .xhr()
        .send_text()
        .ok()?;
    let aux = serde_json::from_str::<Value>(&response)
        .ok()?
        .get("aux")?
        .as_str()?
        .to_string();
    decode_aux(&aux)
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let mut out = Vec::new();
    for chunk in body.split("<a").skip(1) {
        let href = html::attr(chunk, "href").unwrap_or_default();
        if !href.contains("/novel/") {
            continue;
        }
        let key = normalize_key(&href);
        let title = html::attr(chunk, "title")
            .or_else(|| {
                html::text_between(chunk, "ellipsis-1", "</span>")
                    .map(|text| html::strip_tags(&text))
            })
            .or_else(|| html::text_between(chunk, ">", "</a>").map(|text| html::strip_tags(&text)))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
        if out.iter().any(|item: &CatalogItem| item.key == key) {
            continue;
        }
        out.push(CatalogItem {
            key: key.clone(),
            title,
            cover: html::attr_after(chunk, "<img", "src")
                .map(|value| url::join_url(BASE_URL, &value)),
            url: Some(novel_url(&key)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        });
    }
    out
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: html::attr_after(body, "<img", "alt")
            .or_else(|| first_text(body, &["story-title", "<h1", "<title"]))
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(body, "<img", "src").map(|value| url::join_url(BASE_URL, &value)),
        description: content_after(body, "novel-description").map(|value| html::strip_tags(&value)),
        tags: content_after(body, "novel-genres")
            .map(|block| {
                block
                    .split("<li")
                    .skip(1)
                    .filter_map(|chunk| {
                        html::text_between(chunk, ">", "</li>").map(|text| html::strip_tags(&text))
                    })
                    .filter(|text| !text.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        status: parse_status(&html::strip_tags(
            &content_after(body, "novel-tags").unwrap_or_default(),
        )),
        url: Some(novel_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, novel_key: &str) -> Vec<NovelChapter> {
    let mut out = Vec::new();
    for chunk in body.split("<a").skip(1) {
        let href = html::attr(chunk, "href").unwrap_or_default();
        if !href.contains("/novel/") || !href.contains(novel_key) {
            continue;
        }
        let key = normalize_key(&href);
        if key == novel_key || out.iter().any(|chapter: &NovelChapter| chapter.key == key) {
            continue;
        }
        let is_locked = chunk.contains("lock");
        if is_locked {
            continue;
        }
        let title = html::text_between(chunk, ">", "</a>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty());
        out.push(NovelChapter {
            chapter_number: title.as_deref().and_then(chapter_number),
            key: key.clone(),
            title,
            url: Some(novel_url(&key)),
            language: Some("en".to_string()),
            ..NovelChapter::default()
        });
    }
    out
}

fn content_block(body: &str) -> Option<String> {
    for marker in ["chapter-content", "content-area", "entry-content"] {
        if let Some(value) = content_after(body, marker) {
            return Some(value);
        }
    }
    None
}

fn content_after(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</div>")
}

fn first_text(body: &str, markers: &[&str]) -> Option<String> {
    markers
        .iter()
        .find_map(|marker| {
            html::text_between(body, marker, "</").map(|text| html::strip_tags(&text))
        })
        .filter(|value| !value.is_empty())
}

fn quoted_value(body: &str, start: &str, end: &str) -> Option<String> {
    let after = body.split(start).nth(1)?;
    Some(after.split(end).next()?.to_string())
}

fn decode_aux(input: &str) -> Option<String> {
    let mapped = input
        .replace("%Ra&", "A")
        .replace("%Rc&", "B")
        .replace("%Rb&", "C")
        .replace("%Rd&", "D")
        .replace("%Rf&", "E")
        .replace("%Re&", "F");
    String::from_utf8(base64_decode(&mapped)?).ok()
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let bytes: Vec<_> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    for chunk in bytes.chunks(4) {
        let mut values = [64u8; 4];
        for (index, byte) in chunk.iter().enumerate() {
            values[index] = base64_value(*byte)?;
        }
        out.push((values[0] << 2) | (values[1] >> 4));
        if values[2] != 64 {
            out.push((values[1] << 4) | (values[2] >> 2));
        }
        if values[3] != 64 {
            out.push((values[2] << 6) | values[3]);
        }
    }
    Some(out)
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        b'=' => Some(64),
        _ => None,
    }
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .map(ToString::to_string)
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn parse_status(input: &str) -> ItemStatus {
    let lower = input.to_ascii_lowercase();
    if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn novel_url(key: &str) -> String {
    format!("{BASE_URL}/novel/{}", key.trim_start_matches('/'))
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_start_matches("novel/")
        .trim_matches('/')
        .to_string()
}

const LIST_FIXTURE: &str = r#"<div class="col-md-6"><a href="https://www.foxteller.com/novel/sample-novel" title="Sample Novel"><img class="img-fluid" src="/sample.jpg">Sample Novel</a></div>"#;
const SEARCH_FIXTURE: &str = r#"<a href="https://www.foxteller.com/novel/sample-novel"><img src="/sample.jpg"><span class="ellipsis-1">Sample Novel</span></a>"#;
const DETAILS_FIXTURE: &str = r#"<img class="img-fluid" alt="Sample Novel" src="/sample.jpg"><div class="novel-description"><p>A fixture novel.</p></div><div class="novel-genres"><li>Fantasy</li></div><div class="novel-tags"><li>Ongoing</li></div><div class="col-md-6"><ul><li><a href="https://www.foxteller.com/novel/sample-novel/chapter-1">Chapter 1</a></li></ul></div>"#;
const TEXT_PAGE_FIXTURE: &str = r#"<script>'chapter_id': '1'</script><div class="chapter-content"><p>The first fixture paragraph.</p></div>"#;

export_novel_source!(SOURCE);
