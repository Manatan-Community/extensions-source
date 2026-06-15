use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: FictionZone = FictionZone;
const BASE_URL: &str = "https://fictionzone.net";
const CDN_URL: &str = "https://cdn.fictionzone.net/insecure/rs:fill:165:250";

struct FictionZone;

impl NovelSource for FictionZone {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(FICTION_LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            == Some("latest")
        {
            "created_at"
        } else {
            "bookmark_count"
        };
        let path = format!(
            "/platform/browse?page={page}&page_size=20&sort_by={sort}&sort_order=desc&include_genres=true"
        );
        Ok(parse_listing(&api_or_fixture(&path, FICTION_LIST_FIXTURE)))
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
            let body = details_body(&key);
            return Ok(Paged {
                entries: vec![parse_details(&body, &key)],
                has_next_page: false,
            });
        }
        let path = format!(
            "/platform/browse?search={}&page={page}&page_size=20&search_in_synopsis=true&sort_by=bookmark_count&sort_order=desc&include_genres=true",
            url::query_escape(query)
        );
        Ok(parse_listing(&api_or_fixture(&path, FICTION_LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        Ok(parse_details(&details_body(&key), &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        let id = details_id(&details_body(&key)).unwrap_or_else(|| "sample-id".to_string());
        Ok(parse_chapters(
            &api_or_fixture(
                &format!("/platform/chapter-lists?novel_id={id}"),
                CHAPTERS_FIXTURE,
            ),
            &key,
            &id,
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
        let key = novel::request_key(&request, "chapter").unwrap_or_else(|| {
            "novel/sample/chapter-1|/platform/chapter-content?novel_id=sample-id&chapter_id=chapter-1"
                .to_string()
        });
        let api_path = key.split('|').nth(1).unwrap_or(&key);
        Ok(parse_text(&api_or_fixture(api_path, TEXT_FIXTURE), &key))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(FICTION_LIST_FIXTURE);
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: popular.entries,
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
            let body = details_body(&key);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, &key)),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_or_fixture(path: &str, fixture: &str) -> String {
    let payload = serde_json::to_string(&json!({
        "path": path,
        "headers": [
            ["content-type", "application/json"],
            ["x-request-time", "1970-01-01T00:00:00.000Z"]
        ],
        "method": "GET"
    }))
    .unwrap_or_default();
    client()
        .post(format!("{BASE_URL}/api/__api_party/fictionzone"))
        .json(payload)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn details_body(key: &str) -> String {
    let slug = key.trim_start_matches("novel/").trim_matches('/');
    api_or_fixture(
        &format!("/platform/novel-details?slug={slug}"),
        DETAILS_FIXTURE,
    )
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let novels = root
        .get("data")
        .and_then(|data| data.get("novels"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    Paged {
        entries: novels.map(parse_listing_item).collect(),
        has_next_page: false,
    }
}

fn parse_listing_item(item: &Value) -> CatalogItem {
    let slug = text(item, "slug").unwrap_or_else(|| "sample".to_string());
    CatalogItem {
        key: format!("novel/{slug}"),
        title: text(item, "title").unwrap_or_else(|| "Novel".to_string()),
        cover: text(item, "image").map(cover_url),
        url: Some(format!("{BASE_URL}/novel/{slug}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let data = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|root| root.get("data").cloned())
        .unwrap_or(Value::Null);
    CatalogItem {
        key: key.to_string(),
        title: text(&data, "title").unwrap_or_else(|| "Novel".to_string()),
        cover: text(&data, "image").map(cover_url),
        description: text(&data, "synopsis"),
        tags: data
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .chain(
                data.get("tags")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten(),
            )
            .filter_map(|value| text(value, "name"))
            .collect(),
        authors: data
            .get("contributors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|value| text(value, "role").as_deref() == Some("author"))
            .filter_map(|value| text(value, "display_name"))
            .collect(),
        status: match data.get("status").and_then(Value::as_i64) {
            Some(0) => ItemStatus::Completed,
            Some(1) => ItemStatus::Ongoing,
            _ => ItemStatus::Unknown,
        },
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, novel_key: &str, novel_id: &str) -> Vec<NovelChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    root.get("data")
        .and_then(|data| data.get("chapters"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|chapter| {
            let chapter_id = text(chapter, "chapter_id").unwrap_or_else(|| "chapter-1".to_string());
            NovelChapter {
                key: format!(
                    "{novel_key}/{chapter_id}|/platform/chapter-content?novel_id={novel_id}&chapter_id={chapter_id}"
                ),
                title: text(chapter, "title"),
                chapter_number: chapter
                    .get("chapter_number")
                    .and_then(Value::as_f64)
                    .map(|value| value as f32),
                date_uploaded: None,
                url: Some(format!("{BASE_URL}/{novel_key}/{chapter_id}")),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            }
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let content = root
        .get("data")
        .and_then(|data| data.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("Fixture chapter text.");
    let html = content
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .map(|line| format!("<p>{}</p>", escape_html(line.trim())))
        .collect::<String>();
    NovelText {
        html: Some(html.clone()),
        text: Some(content.to_string()),
        base_url: Some(BASE_URL.to_string()),
        image_headers: novel::image_headers(BASE_URL),
        next_chapter_key: Some(key.to_string()),
        ..NovelText::default()
    }
}

fn details_id(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .get("data")?
        .get("id")?
        .as_str()
        .map(ToString::to_string)
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn cover_url(image: String) -> String {
    format!("{CDN_URL}/{image}.webp")
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .to_string()
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

const FICTION_LIST_FIXTURE: &str = r#"{"data":{"novels":[{"title":"Sample Fiction","image":"covers/sample","slug":"sample-fiction"}]}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"id":"sample-id","title":"Sample Fiction","image":"covers/sample","genres":[{"name":"Fantasy"}],"tags":[{"name":"Adventure"}],"status":1,"contributors":[{"role":"author","display_name":"Example Author"}],"synopsis":"A fixture fiction."}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"chapters":[{"title":"Chapter 1","chapter_number":1,"chapter_id":"chapter-1","published_date":"2024-01-01"}]}}"#;
const TEXT_FIXTURE: &str = r#"{"data":{"content":"The first paragraph.\nThe second paragraph."}}"#;

export_novel_source!(SOURCE);
