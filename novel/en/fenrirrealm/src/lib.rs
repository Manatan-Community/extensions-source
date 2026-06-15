use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: FenrirRealm = FenrirRealm;
const BASE_URL: &str = "https://fenrirealm.com";

struct FenrirRealm;

impl NovelSource for FenrirRealm {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        let page = page(&request);
        let latest = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            == Some("latest");
        let status = filter_string(&request, "status").unwrap_or_else(|| "any".to_string());
        let order = if latest {
            "latest".to_string()
        } else {
            filter_string(&request, "sort").unwrap_or_else(|| "popular".to_string())
        };
        let mut query =
            format!("/api/series/filter?page={page}&per_page=20&status={status}&order={order}");
        for genre in filter_array(&request, "genres") {
            query.push_str("&genres[]=");
            query.push_str(&url::query_escape(&genre));
        }
        let body = fetch_api_or_fixture(&query, LIST_FIXTURE);
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
                entries: vec![parse_details(
                    &fetch_api_or_fixture(&format!("/api/new/v2/series/{key}"), DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }
        let path = format!(
            "/api/series/filter?page={}&per_page=20&search={}",
            page(&request),
            url::query_escape(query)
        );
        let body = fetch_api_or_fixture(&path, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: !parse_listing(&body).is_empty(),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "sample-series".to_string());
        Ok(parse_details(
            &fetch_api_or_fixture(
                &format!("/api/new/v2/series/{}", normalize_key(&key)),
                DETAILS_FIXTURE,
            ),
            &normalize_key(&key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "sample-series".to_string());
        let hide_locked = bool_setting(&request, "hideLocked");
        Ok(parse_chapters(
            &fetch_api_or_fixture(
                &format!("/api/new/v2/series/{}/chapters", normalize_key(&key)),
                CHAPTERS_FIXTURE,
            ),
            &normalize_key(&key),
            hide_locked,
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
            .unwrap_or_else(|| "sample-series/chapter-1~~1".to_string());
        let body = key
            .split("~~")
            .nth(1)
            .map(|id| fetch_api_or_fixture(&format!("/api/new/v2/chapters/{id}"), TEXT_FIXTURE))
            .unwrap_or_else(|| {
                fetch_or_fixture(
                    &format!(
                        "{BASE_URL}/series/{}",
                        key.split("~~").next().unwrap_or(&key)
                    ),
                    TEXT_FIXTURE,
                )
            });
        Ok(parse_text(&body, &key))
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
                    &fetch_api_or_fixture(&format!("/api/new/v2/series/{key}"), DETAILS_FIXTURE),
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

fn fetch_api_or_fixture(path: &str, fixture: &str) -> String {
    client()
        .get(url::join_url(BASE_URL, path))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    root.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            let key = text(item, "slug").unwrap_or_else(|| "sample-series".to_string());
            CatalogItem {
                key: key.clone(),
                title: text(item, "title").unwrap_or_else(|| "Novel".to_string()),
                cover: text(item, "cover").map(abs_url),
                description: text(item, "description").map(|value| html::strip_tags(&value)),
                tags: item
                    .get("genres")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|genre| text(genre, "name"))
                    .collect(),
                status: parse_status(item.get("status").and_then(Value::as_str)),
                url: Some(format!("{BASE_URL}/series/{key}")),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let user = root.get("user").unwrap_or(&Value::Null);
    CatalogItem {
        key: key.to_string(),
        title: text(&root, "title").unwrap_or_else(|| "Novel".to_string()),
        cover: text(&root, "cover").map(abs_url),
        description: text(&root, "description").map(|value| html::strip_tags(&value)),
        authors: text(user, "name")
            .or_else(|| text(user, "username"))
            .into_iter()
            .collect(),
        tags: root
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| text(genre, "name"))
            .collect(),
        status: parse_status(root.get("status").and_then(Value::as_str)),
        url: Some(format!("{BASE_URL}/series/{key}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, novel_key: &str, hide_locked: bool) -> Vec<NovelChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let mut chapters: Vec<_> = root
        .as_array()
        .into_iter()
        .flatten()
        .filter(|chapter| !hide_locked || !is_locked(chapter))
        .map(|chapter| {
            let number = chapter.get("number").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let id = chapter
                .get("id")
                .and_then(Value::as_i64)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "1".to_string());
            let number_label = trim_float(number);
            let slug = text(chapter, "slug").unwrap_or_else(|| format!("chapter-{number_label}"));
            let mut path = novel_key.to_string();
            if let Some(group_slug) = chapter
                .get("group")
                .and_then(|group| group.get("slug"))
                .and_then(Value::as_str)
            {
                path.push('/');
                path.push_str(group_slug);
            }
            path.push('/');
            path.push_str(&slug);
            NovelChapter {
                key: format!("{path}~~{id}"),
                title: Some(chapter_title(chapter)),
                chapter_number: Some(number),
                url: Some(format!("{BASE_URL}/series/{path}")),
                language: Some("en".to_string()),
                is_locked: is_locked(chapter),
                ..NovelChapter::default()
            }
        })
        .collect();
    chapters.sort_by(|a, b| {
        a.chapter_number
            .partial_cmp(&b.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let html = root
        .get("content")
        .and_then(Value::as_str)
        .and_then(render_doc)
        .or_else(|| {
            html::text_between(body, "content-area", "</div>").map(|value| {
                value
                    .split("<p")
                    .skip(1)
                    .map(|p| format!("<p{p}"))
                    .collect()
            })
        })
        .unwrap_or_else(|| "<p>Fixture chapter text.</p>".to_string());
    NovelText {
        html: Some(html.clone()),
        text: Some(novel::cleanup_text(&html)),
        base_url: Some(BASE_URL.to_string()),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        next_chapter_key: Some(key.to_string()),
        ..NovelText::default()
    }
}

fn render_doc(input: &str) -> Option<String> {
    let doc: Value = serde_json::from_str(input).ok()?;
    if doc.get("type").and_then(Value::as_str) != Some("doc") {
        return None;
    }
    Some(
        doc.get("content")
            .and_then(Value::as_array)?
            .iter()
            .map(render_block)
            .collect::<String>(),
    )
}

fn render_block(node: &Value) -> String {
    let inner = node
        .get("content")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(render_inline).collect::<String>())
        .unwrap_or_default();
    match node.get("type").and_then(Value::as_str).unwrap_or_default() {
        "heading" => {
            let level = node
                .get("attrs")
                .and_then(|attrs| attrs.get("level"))
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 6);
            format!("<h{level}>{inner}</h{level}>")
        }
        "paragraph" => format!("<p>{inner}</p>"),
        _ => String::new(),
    }
}

fn render_inline(node: &Value) -> String {
    if node.get("type").and_then(Value::as_str) == Some("hardBreak") {
        return "<br>".to_string();
    }
    let mut out = escape_html(node.get("text").and_then(Value::as_str).unwrap_or_default());
    if let Some(marks) = node.get("marks").and_then(Value::as_array) {
        for mark in marks {
            match mark.get("type").and_then(Value::as_str).unwrap_or_default() {
                "bold" => out = format!("<b>{out}</b>"),
                "italic" => out = format!("<i>{out}</i>"),
                "underline" => out = format!("<u>{out}</u>"),
                "strike" => out = format!("<strike>{out}</strike>"),
                "link" => {
                    let href = mark
                        .get("attrs")
                        .and_then(|attrs| attrs.get("href"))
                        .and_then(Value::as_str)
                        .map(escape_html)
                        .unwrap_or_default();
                    out = format!("<a href=\"{href}\">{out}</a>");
                }
                _ => {}
            }
        }
    }
    out
}

fn chapter_title(chapter: &Value) -> String {
    let number = chapter.get("number").and_then(Value::as_f64).unwrap_or(0.0);
    let title = text(chapter, "title").unwrap_or_default();
    let prefix = chapter
        .get("group")
        .and_then(|group| group.get("index"))
        .and_then(Value::as_i64)
        .map(|index| format!("Vol {index} "))
        .unwrap_or_default();
    let number_label = trim_float(number as f32);
    if title.trim().is_empty() || title.eq_ignore_ascii_case(&format!("Chapter {number_label}")) {
        format!("{prefix}Chapter {number_label}")
    } else {
        format!("{prefix}Chapter {number_label} - {title}")
    }
}

fn trim_float(number: f32) -> String {
    if number.fract() == 0.0 {
        (number as i64).to_string()
    } else {
        number.to_string()
    }
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .map(ToString::to_string)
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

fn bool_setting(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .or_else(|| request.get("settings"))
        .and_then(|settings| settings.get(id))
        .and_then(|value| value.get("value").unwrap_or(value).as_bool())
        .unwrap_or(false)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_start_matches("series/")
        .trim_matches('/')
        .split("~~")
        .next()
        .unwrap_or(input)
        .to_string()
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.unwrap_or_default().to_ascii_lowercase().as_str() {
        "completed" | "complete" => ItemStatus::Completed,
        "hiatus" => ItemStatus::Hiatus,
        "dropped" | "cancelled" => ItemStatus::Cancelled,
        "unknown" => ItemStatus::Unknown,
        _ => ItemStatus::Ongoing,
    }
}

fn is_locked(chapter: &Value) -> bool {
    chapter
        .get("locked")
        .is_some_and(|locked| !locked.is_null())
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn abs_url(value: String) -> String {
    url::join_url(BASE_URL, &value)
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const LIST_FIXTURE: &str = r#"{"data":[{"title":"Sample Realm","slug":"sample-realm","cover":"covers/sample.png","description":"A fixture series.","status":"ongoing","genres":[{"name":"Fantasy"}]}]}"#;
const DETAILS_FIXTURE: &str = r#"{"title":"Sample Realm","slug":"sample-realm","cover":"covers/sample.png","description":"<p>A fixture series.</p>","status":"ongoing","genres":[{"name":"Fantasy"}],"user":{"name":"Realm Author"}}"#;
const CHAPTERS_FIXTURE: &str = r#"[{"id":1,"locked":null,"group":null,"title":"Awakening","slug":"chapter-1","number":1,"created_at":"2024-01-01"}]"#;
const TEXT_FIXTURE: &str = r#"{"content":"{\"type\":\"doc\",\"content\":[{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"The first fixture paragraph.\"}]}]}"}"#;

export_novel_source!(SOURCE);
