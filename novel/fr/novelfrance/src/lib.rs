use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    novel,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: NovelFrance = NovelFrance;
const BASE_URL: &str = "https://novelfrance.fr";
const PAGE_SIZE: u64 = 24;

struct NovelFrance;

impl NovelSource for NovelFrance {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            let offset = (page.saturating_sub(1)) * PAGE_SIZE;
            let target =
                format!("{BASE_URL}/api/chapters/latest-home?offset={offset}&limit={PAGE_SIZE}");
            let body = fetch_json_or_fixture(&target, LATEST_FIXTURE);
            return Ok(parse_latest(&body));
        }

        let mut target = format!(
            "{BASE_URL}/api/search?skip={}&take={PAGE_SIZE}",
            (page.saturating_sub(1)) * PAGE_SIZE
        );
        if let Some(genre) = filter_string(&request, "genre") {
            target.push_str("&genres=");
            target.push_str(&url::query_escape(&genre));
        }
        if let Some(status) = filter_string(&request, "status") {
            target.push_str("&status=");
            target.push_str(&url::query_escape(&status));
        }
        Ok(parse_listing(&fetch_json_or_fixture(&target, LIST_FIXTURE)))
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
        let target = format!(
            "{BASE_URL}/api/search?q={}&skip={}&take={PAGE_SIZE}",
            url::query_escape(query),
            (page.saturating_sub(1)) * PAGE_SIZE
        );
        Ok(parse_listing(&fetch_json_or_fixture(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        Ok(fetch_chapters(&slug_from_key(&key)))
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
            .unwrap_or_else(|| "sample/chapter-1".to_string());
        let body = fetch_json_or_fixture(&format!("{BASE_URL}/api/chapters/{key}"), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Populaire".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Derniers".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
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
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(key: &str) -> CatalogItem {
    let slug = slug_from_key(key);
    let body = fetch_json_or_fixture(&format!("{BASE_URL}/api/novels/{slug}"), DETAILS_FIXTURE);
    let data = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
    parse_details(&data, &slug)
}

fn fetch_chapters(slug: &str) -> Vec<NovelChapter> {
    let mut chapters = Vec::new();
    let take = 100;
    for i in 0..100 {
        let skip = i * take;
        let target = format!("{BASE_URL}/api/chapters/{slug}?skip={skip}&take={take}&order=asc");
        let body = fetch_json_or_fixture(&target, CHAPTERS_FIXTURE);
        let root = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
        let list = root
            .get("chapters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let count = list.len();
        chapters.extend(list.iter().map(|chapter| parse_chapter_item(slug, chapter)));
        let has_more = root
            .get("hasMore")
            .and_then(Value::as_bool)
            .unwrap_or(count >= take);
        if !has_more || count < take {
            break;
        }
    }
    chapters.sort_by(|a, b| {
        a.chapter_number
            .partial_cmp(&b.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let entries = root
        .get("novels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(parse_catalog_item)
        .collect::<Vec<_>>();
    let has_next_page = entries.len() >= PAGE_SIZE as usize;
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let entries = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(parse_catalog_item)
        .collect::<Vec<_>>();
    let has_next_page = entries.len() >= PAGE_SIZE as usize;
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_catalog_item(item: &Value) -> CatalogItem {
    let slug = text(item, "slug").unwrap_or_else(|| "sample".to_string());
    CatalogItem {
        key: slug.clone(),
        title: text(item, "title").unwrap_or_else(|| title_from_key(&slug)),
        cover: text(item, "coverImage").map(|cover| absolute_url(&cover)),
        url: Some(novel_url(&slug)),
        language: Some("fr".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(data: &Value, slug: &str) -> CatalogItem {
    CatalogItem {
        key: slug.to_string(),
        title: text(data, "title").unwrap_or_else(|| title_from_key(slug)),
        cover: text(data, "coverImage").map(|cover| absolute_url(&cover)),
        description: text(data, "description"),
        authors: text(data, "author").into_iter().collect(),
        artists: text(data, "translatorName").into_iter().collect(),
        tags: data
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| text(genre, "name").or_else(|| genre.as_str().map(str::to_string)))
            .collect(),
        status: parse_status(text(data, "status").as_deref()),
        url: Some(novel_url(slug)),
        language: Some("fr".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapter_item(slug: &str, chapter: &Value) -> NovelChapter {
    let number = chapter
        .get("chapterNumber")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let chapter_slug = text(chapter, "slug").unwrap_or_else(|| format!("chapter-{number}"));
    let title = text(chapter, "title");
    let display = if number.fract() == 0.0 {
        format!("{}", number as i64)
    } else {
        number.to_string()
    };
    let name = title
        .filter(|value| !value.is_empty())
        .map(|value| format!("Chapitre {display} - {value}"))
        .unwrap_or_else(|| format!("Chapitre {display}"));
    NovelChapter {
        key: format!("{slug}/{chapter_slug}"),
        title: Some(name),
        chapter_number: Some(number as f32),
        url: Some(format!("{}/novel/{slug}/{chapter_slug}", BASE_URL)),
        language: Some("fr".to_string()),
        ..NovelChapter::default()
    }
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let mut parts = Vec::new();
    if let Some(title) = text(&root, "title") {
        parts.push(format!("<h1>{}</h1>", escape_html(&title)));
    }
    for paragraph in root
        .get("paragraphs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if paragraph.get("index").and_then(Value::as_u64) == Some(0) && !parts.is_empty() {
            continue;
        }
        let Some(content) = text(paragraph, "content") else {
            continue;
        };
        let content = content.trim();
        if !content.is_empty() {
            parts.push(format!("<p>{}</p>", escape_html(content)));
        }
    }
    let html = if parts.is_empty() {
        "<p>Chapter content could not be loaded.</p>".to_string()
    } else {
        parts.join("\n")
    };
    let normalized = novel::normalize_reader_html(&html);
    NovelText {
        title: text(&root, "title"),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(format!("{BASE_URL}/novel/{key}")),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .filter(|value| !value.is_empty())
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.unwrap_or_default() {
        "ONGOING" => ItemStatus::Ongoing,
        "COMPLETED" => ItemStatus::Completed,
        "HIATUS" => ItemStatus::Hiatus,
        "DROPPED" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn filter_string(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(|value| value.get("value").or(Some(value)))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn key_from_url(input: &str) -> Option<String> {
    if !input.contains("novelfrance.fr") {
        return None;
    }
    let path = input
        .split("novelfrance.fr")
        .nth(1)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim();
    let key = path
        .strip_prefix("novel/")
        .unwrap_or(path)
        .trim_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();
    (!key.is_empty()).then(|| key.to_string())
}

fn slug_from_key(key: &str) -> String {
    key.trim_matches('/')
        .strip_prefix("novel/")
        .unwrap_or(key.trim_matches('/'))
        .split('/')
        .next()
        .unwrap_or("sample")
        .to_string()
}

fn novel_url(slug: &str) -> String {
    format!("{BASE_URL}/novel/{slug}")
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn title_from_key(key: &str) -> String {
    url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    if !request.is_object() {
        request = serde_json::json!({});
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    request
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const LIST_FIXTURE: &str =
    r#"{"novels":[{"title":"Sample Novel","slug":"sample","coverImage":"/cover.jpg"}]}"#;

const LATEST_FIXTURE: &str =
    r#"{"data":[{"title":"Sample Novel","slug":"sample","coverImage":"/cover.jpg"}]}"#;

const DETAILS_FIXTURE: &str = r#"{"title":"Sample Novel","slug":"sample","description":"Sample summary.","coverImage":"/cover.jpg","author":"Sample Author","translatorName":"Sample Translator","status":"ONGOING","genres":[{"name":"Action"}]}"#;

const CHAPTERS_FIXTURE: &str = r#"{"chapters":[{"chapterNumber":1,"title":"Debut","slug":"chapter-1","createdAt":"2024-01-01"}],"hasMore":false}"#;

const TEXT_FIXTURE: &str = r#"{"title":"Chapter 1","paragraphs":[{"index":0,"content":"Chapter 1"},{"index":1,"content":"Sample chapter text."}]}"#;

export_novel_source!(SOURCE);
