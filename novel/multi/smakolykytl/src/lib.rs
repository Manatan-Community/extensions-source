use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: SmakolykyTl = SmakolykyTl;
const BASE_URL: &str = "https://smakolykytl.site";
const API_URL: &str = "https://api.smakolykytl.site/api/user";

struct SmakolykyTl;

impl NovelSource for SmakolykyTl {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let endpoint = if listing == "latest" {
            "updates"
        } else {
            "projects"
        };
        let body = fetch_json_or_fixture(&format!("{API_URL}/{endpoint}"), LIST_FIXTURE);
        let entries = parse_projects(&body);
        Ok(Paged {
            has_next_page: false,
            entries,
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
        let body = fetch_json_or_fixture(&format!("{API_URL}/projects"), LIST_FIXTURE);
        let folded = query.to_lowercase();
        let entries = parse_projects(&body)
            .into_iter()
            .filter(|item| {
                item.title.to_lowercase().contains(&folded)
                    || item.key.trim_start_matches("titles/") == query
            })
            .collect();
        Ok(Paged {
            has_next_page: false,
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "titles/1".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "titles/1".to_string());
        let id = key.trim_end_matches('/').rsplit('/').next().unwrap_or("1");
        let body = fetch_json_or_fixture(&format!("{API_URL}/projects/{id}/books"), BOOKS_FIXTURE);
        Ok(parse_books(&body))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter").unwrap_or_else(|| "read/1".to_string());
        let id = key.trim_end_matches('/').rsplit('/').next().unwrap_or("1");
        let body = fetch_json_or_fixture(&format!("{API_URL}/chapters/{id}"), TEXT_FIXTURE);
        let value = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
        let title = value
            .pointer("/chapter/title")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let raw_content = value
            .pointer("/chapter/content")
            .and_then(Value::as_str)
            .unwrap_or("[]");
        let content =
            serde_json::from_str::<Value>(raw_content).unwrap_or(Value::Array(Vec::new()));
        let html = json_to_html(&content);
        Ok(text_from_html(&key, title, html))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Projects".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Updates".to_string(),
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
        .with_origin(BASE_URL)
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_cookies_for(API_URL)
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

fn parse_projects(body: &str) -> Vec<CatalogItem> {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    value
        .get("projects")
        .or_else(|| value.get("updates"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(project_item)
        .collect()
}

fn project_item(project: &Value) -> Option<CatalogItem> {
    let id = project.get("id").and_then(Value::as_i64)?;
    let title = project.get("title").and_then(Value::as_str)?.to_string();
    let key = format!("titles/{id}");
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: project
            .pointer("/image/url")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(absolute_url(&key)),
        language: Some("multi".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn fetch_details(key: &str) -> CatalogItem {
    let id = key.trim_end_matches('/').rsplit('/').next().unwrap_or("1");
    let body = fetch_json_or_fixture(&format!("{API_URL}/projects/{id}"), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let project = value.get("project").unwrap_or(&value);
    let key = normalize_key(key);
    let mut tags = Vec::new();
    for field in ["genres", "tags"] {
        if let Some(values) = project.get(field).and_then(Value::as_array) {
            tags.extend(values.iter().filter_map(|tag| {
                tag.get("title")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            }));
        }
    }
    CatalogItem {
        key: key.clone(),
        title: project
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Smakolyky")
            .to_string(),
        cover: project
            .pointer("/image/url")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        description: project
            .get("description")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        authors: project
            .get("author")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .into_iter()
            .collect(),
        tags,
        status: project
            .get("status_translate")
            .or_else(|| project.get("status"))
            .and_then(Value::as_str)
            .map(parse_status)
            .unwrap_or(ItemStatus::Unknown),
        url: Some(absolute_url(&key)),
        language: Some("multi".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_books(body: &str) -> Vec<NovelChapter> {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let mut chapters = Vec::new();
    for book in value
        .get("books")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let volume = book
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default();
        for chapter in book
            .get("chapters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(id) = chapter.get("id").and_then(Value::as_i64) else {
                continue;
            };
            let title = chapter
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Chapter");
            let key = format!("read/{id}");
            chapters.push(NovelChapter {
                key: key.clone(),
                title: Some(format!("{volume} {title}").trim().to_string()),
                chapter_number: Some((chapters.len() + 1) as f32),
                date_uploaded: chapter
                    .get("modifiedAt")
                    .and_then(Value::as_str)
                    .and_then(parse_iso_date),
                url: Some(absolute_url(&key)),
                language: Some("multi".to_string()),
                ..NovelChapter::default()
            });
        }
    }
    chapters
}

fn json_to_html(value: &Value) -> String {
    let mut out = String::new();
    if let Some(array) = value.as_array() {
        for node in array {
            out.push_str(&node_to_html(node));
        }
    }
    out
}

fn node_to_html(node: &Value) -> String {
    match node.get("type").and_then(Value::as_str).unwrap_or_default() {
        "hardBreak" => "<br>".to_string(),
        "horizontalRule" => "<hr>".to_string(),
        "image" => {
            let attrs = node.get("attrs").and_then(Value::as_object);
            let mut parts = Vec::new();
            for name in ["src", "alt", "title"] {
                if let Some(value) = attrs
                    .and_then(|attrs| attrs.get(name))
                    .and_then(Value::as_str)
                {
                    parts.push(format!("{name}=\"{}\"", escape_attr(value)));
                }
            }
            format!("<img {}>", parts.join(" "))
        }
        "paragraph" => {
            let inner = node
                .get("content")
                .map(json_to_html)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "<br>".to_string());
            format!("<p>{inner}</p>")
        }
        "text" => node
            .get("text")
            .and_then(Value::as_str)
            .map(escape_text)
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn text_from_html(key: &str, title: Option<String>, raw: String) -> NovelText {
    let normalized = novel::normalize_reader_html(&raw);
    NovelText {
        title,
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("трива") || lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else if lower.contains("hiatus") || lower.contains("paused") {
        ItemStatus::Hiatus
    } else if lower.contains("cancel") || lower.contains("drop") {
        ItemStatus::Cancelled
    } else if !lower.is_empty() {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let date = value.split('T').next().unwrap_or(value);
    manatan_shared::dates::parse_ymd(date)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .contains("smakolykytl.site")
        .then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    if let Some(object) = request.as_object_mut() {
        object.insert("listingId".to_string(), Value::String(listing.to_string()));
    }
    request
}

fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(value: &str) -> String {
    escape_text(value).replace('"', "&quot;")
}

export_novel_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"projects":[{"id":1,"title":"Sample Novel","image":{"url":"https://smakolykytl.site/cover.jpg"}}]}"#;
const DETAILS_FIXTURE: &str = r#"{"project":{"id":1,"title":"Sample Novel","description":"Sample summary.","author":"Author","status_translate":"Триває","image":{"url":"https://smakolykytl.site/cover.jpg"},"genres":[{"title":"Fantasy"}],"tags":[{"title":"Adventure"}]}}"#;
const BOOKS_FIXTURE: &str = r#"{"books":[{"title":"Volume 1","chapters":[{"id":1,"title":"Chapter 1","modifiedAt":"2024-01-01T00:00:00.000Z"}]}]}"#;
const TEXT_FIXTURE: &str = r#"{"chapter":{"title":"Chapter 1","content":"[{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"Sample chapter text.\"}]}]"}}"#;
