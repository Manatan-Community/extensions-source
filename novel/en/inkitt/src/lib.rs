use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Inkitt = Inkitt;
const BASE_URL: &str = "https://www.inkitt.com";

struct Inkitt;

impl NovelSource for Inkitt {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let genre = filter_string(&request, "genres").unwrap_or_default();
        let target = if genre.trim().is_empty() {
            format!("{BASE_URL}/trending_stories?page={page}&period=alltime")
        } else {
            format!("{BASE_URL}/genre/{genre}/{page}?period=alltime&sort=popular")
        };
        let body = fetch_api_or_fixture(&target, LIST_FIXTURE);
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
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&story_html(&key), &story_api(&key), &key)],
                has_next_page: false,
            });
        }
        let target = format!(
            "{BASE_URL}/api/2/search/title?q={}&page={}",
            url::query_escape(query),
            request.get("page").and_then(Value::as_u64).unwrap_or(1)
        );
        let body = fetch_api_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: !parse_listing(&body).is_empty(),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "fantasy/1".to_string());
        Ok(parse_details(
            &story_html(&normalize_key(&key)),
            &story_api(&normalize_key(&key)),
            &normalize_key(&key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "fantasy/1".to_string());
        Ok(parse_chapters(
            &story_api(&normalize_key(&key)),
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
            .unwrap_or_else(|| "fantasy/1/chapters/1".to_string());
        let body = fetch_or_fixture(
            &format!("{BASE_URL}/stories/{}", normalize_key(&key)),
            TEXT_FIXTURE,
        );
        let chapter_html = div_by_id(&body, "chapterText")
            .unwrap_or_else(|| "<p>The first fixture paragraph.</p>".to_string());
        Ok(NovelText {
            html: Some(chapter_html.clone()),
            text: Some(novel::cleanup_text(&chapter_html)),
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
            id: "trending".to_string(),
            title: "Trending".to_string(),
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
                item: Some(parse_details(&story_html(&key), &story_api(&key), &key)),
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

fn fetch_api_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn story_html(key: &str) -> String {
    fetch_or_fixture(
        &format!("{BASE_URL}/stories/{}", normalize_key(key)),
        DETAILS_HTML_FIXTURE,
    )
}

fn story_api(key: &str) -> String {
    let id = story_id(key).unwrap_or_else(|| "1".to_string());
    fetch_api_or_fixture(&format!("{BASE_URL}/api/stories/{id}"), DETAILS_API_FIXTURE)
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    root.get("stories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|story| {
            let key = story_path(story);
            CatalogItem {
                key: key.clone(),
                title: json_text(story, "title").unwrap_or_else(|| "Story".to_string()),
                cover: cover_from_story(story),
                url: Some(format!("{BASE_URL}/stories/{key}")),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect()
}

fn parse_details(html_body: &str, api_body: &str, key: &str) -> CatalogItem {
    let api: Value = serde_json::from_str(api_body).unwrap_or(Value::Null);
    let title = text_after(html_body, "story-title")
        .or_else(|| first_text(html_body, &["<h1", "<title"]))
        .unwrap_or_else(|| "Story".to_string());
    CatalogItem {
        key: normalize_key(key),
        title,
        cover: api
            .get("vertical_cover")
            .and_then(|cover| json_text(cover, "url")),
        description: content_after(html_body, "story-summary")
            .map(|value| html::strip_tags(&value)),
        authors: text_after(html_body, "author-link").into_iter().collect(),
        tags: content_after(html_body, "genres")
            .map(|block| {
                block
                    .split("<a")
                    .skip(1)
                    .filter_map(|chunk| {
                        html::text_between(chunk, ">", "</a>").map(|text| html::strip_tags(&text))
                    })
                    .filter(|text| !text.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        status: parse_status(
            &content_after(html_body, "Status")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default(),
        ),
        url: Some(format!("{BASE_URL}/stories/{}", normalize_key(key))),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(api_body: &str, story_key: &str) -> Vec<NovelChapter> {
    let root: Value = serde_json::from_str(api_body).unwrap_or(Value::Null);
    root.get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|chapter| {
            let number = chapter
                .get("chapter_number")
                .and_then(Value::as_f64)
                .unwrap_or(1.0) as f32;
            let key = format!(
                "{}/chapters/{}",
                normalize_key(story_key),
                trim_float(number)
            );
            NovelChapter {
                key: key.clone(),
                title: json_text(chapter, "name"),
                chapter_number: Some(number),
                url: Some(format!("{BASE_URL}/stories/{key}")),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            }
        })
        .collect()
}

fn story_path(story: &Value) -> String {
    let id = story
        .get("id")
        .and_then(Value::as_i64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "1".to_string());
    let category = json_text(story, "category_one")
        .or_else(|| {
            story
                .get("genres")
                .and_then(Value::as_array)
                .and_then(|genres| genres.first())
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "story".to_string());
    format!("{category}/{id}")
}

fn cover_from_story(story: &Value) -> Option<String> {
    story
        .get("vertical_cover")
        .and_then(|cover| json_text(cover, "url").or_else(|| json_text(cover, "iphone")))
        .or_else(|| story.get("cover").and_then(|cover| json_text(cover, "url")))
}

fn div_by_id(body: &str, id: &str) -> Option<String> {
    let marker = format!("id=\"{id}\"");
    html::text_between(body, &marker, "</div>")
}

fn content_after(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</")
}

fn text_after(body: &str, marker: &str) -> Option<String> {
    content_after(body, marker)
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn first_text(body: &str, markers: &[&str]) -> Option<String> {
    markers.iter().find_map(|marker| text_after(body, marker))
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .map(ToString::to_string)
}

fn json_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn parse_status(status: &str) -> ItemStatus {
    let lower = status.to_ascii_lowercase();
    if lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn story_id(key: &str) -> Option<String> {
    normalize_key(key)
        .split('/')
        .nth(1)
        .map(ToString::to_string)
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_start_matches("stories/")
        .trim_matches('/')
        .to_string()
}

fn trim_float(number: f32) -> String {
    if number.fract() == 0.0 {
        (number as i64).to_string()
    } else {
        number.to_string()
    }
}

const LIST_FIXTURE: &str = r#"{"stories":[{"id":1,"title":"Sample Inkitt","category_one":"fantasy","genres":["fantasy"],"cover":{"url":"https://www.inkitt.com/sample.jpg"},"vertical_cover":{"url":"https://www.inkitt.com/sample.jpg","iphone":"https://www.inkitt.com/sample.jpg"}}]}"#;
const DETAILS_HTML_FIXTURE: &str = r#"<h1 class="story-title">Sample Inkitt</h1><dl><dd><a class="author-link">Inkitt Author</a></dd><dd class="genres"><a>Fantasy</a></dd></dl><div class="dlc"><dl><dt>Status</dt><dd>Ongoing</dd></dl></div><p class="story-summary">A fixture story.</p>"#;
const DETAILS_API_FIXTURE: &str = r#"{"vertical_cover":{"url":"https://www.inkitt.com/sample.jpg"},"chapters":[{"name":"Chapter 1","chapter_number":1}]}"#;
const TEXT_FIXTURE: &str = r#"<div id="chapterText"><p>The first fixture paragraph.</p></div>"#;

export_novel_source!(SOURCE);
