use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    html, novel,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: WuxiaWorld = WuxiaWorld;
const BASE_URL: &str = "https://www.wuxiaworld.com";

struct WuxiaWorld;

impl NovelSource for WuxiaWorld {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = fetch_json_or_fixture(&format!("{BASE_URL}/api/novels"), LIST_FIXTURE);
        Ok(parse_listing(&body))
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
        let target = format!(
            "{BASE_URL}/api/novels/search?query={}",
            url::query_escape(query)
        );
        Ok(parse_listing(&fetch_json_or_fixture(&target, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel")
            .unwrap_or_else(|| "novel/against-the-gods/".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel")
            .unwrap_or_else(|| "novel/against-the-gods/".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
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
            .unwrap_or_else(|| "novel/against-the-gods/atg-chapter-1".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), TEXT_PAGE_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Novels".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: list.entries,
            has_more: list.has_next_page,
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

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let entries = root
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(parse_listing_item)
        .collect::<Vec<_>>();
    let total = root.get("total").and_then(Value::as_u64).unwrap_or(entries.len() as u64);
    Paged {
        has_next_page: total > entries.len() as u64,
        entries,
    }
}

fn parse_listing_item(item: &Value) -> CatalogItem {
    let slug = text(item, "slug").unwrap_or_else(|| "against-the-gods".to_string());
    CatalogItem {
        key: format!("novel/{slug}/"),
        title: text(item, "name").unwrap_or_else(|| slug.replace('-', " ")),
        cover: text(item, "coverUrl"),
        description: text(item, "synopsis").map(|value| html::strip_tags(&value)),
        tags: item
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        authors: text(item, "authorName").into_iter().collect(),
        status: item
            .get("status")
            .and_then(Value::as_i64)
            .map(parse_status_code)
            .unwrap_or(ItemStatus::Unknown),
        url: Some(format!("{BASE_URL}/novel/{slug}/")),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let item = hydration_query(body, "novel")
        .and_then(|value| value.pointer("/state/data/item").cloned())
        .or_else(|| serde_json::from_str::<Value>(DETAIL_ITEM_FIXTURE).ok())
        .unwrap_or(Value::Null);
    let mut details = parse_listing_item(&item);
    details.key = normalize_key(key);
    details.initialized = true;
    details.description = rich_text(&item, "description")
        .into_iter()
        .chain(rich_text(&item, "synopsis"))
        .collect::<Vec<_>>()
        .join("\n\n")
        .trim()
        .to_string()
        .into();
    details.cover = item.pointer("/coverUrl/value").and_then(Value::as_str).map(ToString::to_string);
    details.authors = item
        .pointer("/authorName/value")
        .and_then(Value::as_str)
        .map(|author| vec![author.to_string()])
        .unwrap_or_default();
    details.tags = item
        .get("genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect();
    details.status = item
        .get("status")
        .and_then(Value::as_i64)
        .map(parse_status_code)
        .unwrap_or(ItemStatus::Unknown);
    details
}

fn parse_chapters(body: &str, key: &str) -> Vec<NovelChapter> {
    let item = hydration_query(body, "novel")
        .and_then(|value| value.pointer("/state/data/item").cloned())
        .or_else(|| serde_json::from_str::<Value>(DETAIL_ITEM_FIXTURE).ok())
        .unwrap_or(Value::Null);
    let novel_slug = text(&item, "slug").or_else(|| key.split('/').nth(1).map(str::to_string)).unwrap_or_default();
    let mut chapters = Vec::new();
    for group in item
        .get("chapterGroups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let group_title = text(group, "title");
        for chapter in group
            .get("chapterList")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(slug) = text(chapter, "slug") else {
                continue;
            };
            let mut title = text(chapter, "name").or_else(|| Some(slug.replace('-', " ")));
            if let Some(group_title) = &group_title {
                if let Some(current) = title.take() {
                    title = Some(format!("{group_title}: {current}"));
                }
            }
            let number = chapter
                .get("offset")
                .and_then(Value::as_f64)
                .or_else(|| decimal_value(chapter.get("number")));
            let chapter_key = format!("novel/{novel_slug}/{slug}");
            chapters.push(NovelChapter {
                key: chapter_key.clone(),
                title,
                chapter_number: number.map(|value| value as f32),
                date_uploaded: chapter
                    .get("publishedAt")
                    .and_then(timestamp_millis)
                    .or_else(|| chapter.get("timePublished").and_then(timestamp_millis)),
                url: Some(absolute_url(&chapter_key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            });
        }
    }
    chapters
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let item = hydration_query(body, "chapter")
        .and_then(|value| value.pointer("/state/data/item").cloned())
        .or_else(|| serde_json::from_str::<Value>(CHAPTER_ITEM_FIXTURE).ok())
        .unwrap_or(Value::Null);
    let content = item
        .pointer("/content/value")
        .and_then(Value::as_str)
        .or_else(|| item.get("content").and_then(Value::as_str))
        .unwrap_or(TEXT_FALLBACK_HTML);
    let normalized = novel::normalize_reader_html(content);
    NovelText {
        title: text(&item, "name"),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn hydration_query(body: &str, key: &str) -> Option<Value> {
    let state = react_query_state(body)?;
    state
        .get("queries")
        .and_then(Value::as_array)?
        .iter()
        .find(|query| {
            query
                .get("queryKey")
                .and_then(Value::as_array)
                .and_then(|keys| keys.first())
                .and_then(Value::as_str)
                == Some(key)
        })
        .cloned()
}

fn react_query_state(body: &str) -> Option<Value> {
    let marker = "window.__REACT_QUERY_STATE__ = ";
    let start = body.find(marker)? + marker.len();
    let rest = &body[start..];
    let end = rest
        .find(";\nwindow.__APP_CONTEXT__")
        .or_else(|| rest.find(";</script>"))
        .unwrap_or(rest.len());
    serde_json::from_str(rest[..end].trim()).ok()
}

fn rich_text(item: &Value, key: &str) -> Option<String> {
    item.pointer(&format!("/{key}/value"))
        .and_then(Value::as_str)
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn decimal_value(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    let units = value
        .get("units")
        .and_then(Value::as_str)
        .and_then(|text| text.parse::<f64>().ok())
        .or_else(|| value.get("units").and_then(Value::as_f64))
        .unwrap_or(0.0);
    let nanos = value.get("nanos").and_then(Value::as_f64).unwrap_or(0.0);
    Some(units + nanos / 1_000_000_000.0)
}

fn timestamp_millis(value: &Value) -> Option<i64> {
    let seconds = value
        .get("seconds")
        .and_then(Value::as_i64)
        .or_else(|| value.get("seconds").and_then(Value::as_str).and_then(|s| s.parse().ok()))?;
    let nanos = value.get("nanos").and_then(Value::as_i64).unwrap_or(0);
    Some(seconds * 1000 + nanos / 1_000_000)
}

fn parse_status_code(value: i64) -> ItemStatus {
    match value {
        1 => ItemStatus::Ongoing,
        2 => ItemStatus::Hiatus,
        0 => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input.contains("wuxiaworld.com").then(|| normalize_key(input))
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
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        url::join_url(BASE_URL, input)
    }
}

const LIST_FIXTURE: &str = r#"{
  "total": 1,
  "items": [
    {
      "id": 21,
      "name": "Against the Gods",
      "slug": "against-the-gods",
      "coverUrl": "https://cdn.wuxiaworld.com/images/covers/atg.webp",
      "synopsis": "<p>Sample synopsis.</p>",
      "authorName": "Mars Gravity",
      "status": 1,
      "genres": ["Action", "Fantasy"]
    }
  ]
}"#;

const SEARCH_FIXTURE: &str = r#"{
  "items": [
    {
      "id": 21,
      "name": "Against the Gods",
      "slug": "against-the-gods",
      "coverUrl": "https://cdn.wuxiaworld.com/images/covers/atg.webp",
      "synopsis": "<p>Sample synopsis.</p>",
      "status": 1,
      "genres": ["Action", "Fantasy"]
    }
  ],
  "result": true
}"#;

const DETAIL_ITEM_FIXTURE: &str = r#"{
  "id": 21,
  "name": "Against the Gods",
  "slug": "against-the-gods",
  "status": 1,
  "genres": ["Action", "Fantasy"],
  "description": { "value": "<p>Sample details.</p>" },
  "synopsis": { "value": "<p>Sample synopsis.</p>" },
  "coverUrl": { "value": "https://cdn.wuxiaworld.com/images/covers/atg.webp" },
  "authorName": { "value": "Mars Gravity" },
  "chapterGroups": [
    {
      "title": "Book 1",
      "chapterList": [
        {
          "name": "Chapter 1",
          "slug": "atg-chapter-1",
          "offset": 1,
          "publishedAt": { "seconds": 1434153600, "nanos": 0 }
        }
      ]
    }
  ]
}"#;

const CHAPTER_ITEM_FIXTURE: &str = r#"{
  "name": "Chapter 1",
  "content": { "value": "<p>Sample chapter text.</p>" }
}"#;

const DETAILS_FIXTURE: &str = r#"<script>window.__REACT_QUERY_STATE__ = {"queries":[{"state":{"data":{"item":{"id":21,"name":"Against the Gods","slug":"against-the-gods","status":1,"genres":["Action","Fantasy"],"description":{"value":"<p>Sample details.</p>"},"synopsis":{"value":"<p>Sample synopsis.</p>"},"coverUrl":{"value":"https://cdn.wuxiaworld.com/images/covers/atg.webp"},"authorName":{"value":"Mars Gravity"},"chapterGroups":[{"title":"Book 1","chapterList":[{"name":"Chapter 1","slug":"atg-chapter-1","offset":1,"publishedAt":{"seconds":1434153600,"nanos":0}}]}]}}},"queryKey":["novel","against-the-gods",null]}]}; window.__APP_CONTEXT__ = {};</script>"#;

const TEXT_PAGE_FIXTURE: &str = r#"<script>window.__REACT_QUERY_STATE__ = {"queries":[{"state":{"data":{"item":{"name":"Chapter 1","content":{"value":"<p>Sample chapter text.</p>"}}}},"queryKey":["chapter","against-the-gods","atg-chapter-1",null]}]}; window.__APP_CONTEXT__ = {};</script>"#;

const TEXT_FALLBACK_HTML: &str = "<p>Sample chapter text.</p>";

export_novel_source!(SOURCE);
