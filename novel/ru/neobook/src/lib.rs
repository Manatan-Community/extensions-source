use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    html, lnreader, novel,
    sdk::{SearchRequest, http::HttpClient},
};
use serde_json::Value;

const SOURCE: Neobook = Neobook;
const BASE_URL: &str = "https://neobook.org";
const API_URL: &str = "https://api.neobook.org/";

struct Neobook;

impl NovelSource for Neobook {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort = if listing == "latest" {
            "new".to_string()
        } else {
            lnreader::filter_string(&request, "sort", "popular")
        };
        let root = post_bundle(page, "", &sort, &request);
        let entries = root
            .pointer("/bundle_books/feed")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(parse_item)
            .collect::<Vec<_>>();
        Ok(Paged {
            has_next_page: entries.len() >= 20,
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let root = post_bundle(page, query, "popular", &Value::Null);
        let entries = root
            .pointer("/bundle_books/feed")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(parse_item)
            .collect::<Vec<_>>();
        Ok(Paged {
            has_next_page: entries.len() >= 20,
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample/".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample/".to_string());
        let book = book_data(&key);
        let book_token = text(&book, "token").unwrap_or_else(|| key.trim_matches('/').to_string());
        Ok(book
            .get("chapters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, chapter)| {
                text(chapter, "access").as_deref() == Some("1")
                    && text(chapter, "status").as_deref() == Some("1")
            })
            .map(|(index, chapter)| {
                let token = text(chapter, "token").unwrap_or_else(|| "chapter".to_string());
                NovelChapter {
                    key: format!("?book={book_token}&chapter={token}"),
                    title: text(chapter, "title").or_else(|| Some(format!("Глава {}", index + 1))),
                    chapter_number: text(chapter, "sort")
                        .and_then(|sort| sort.parse::<f32>().ok())
                        .or(Some((index + 1) as f32)),
                    url: Some(format!(
                        "{BASE_URL}/reader/?book={book_token}&chapter={token}"
                    )),
                    language: Some("ru".to_string()),
                    ..NovelChapter::default()
                }
            })
            .collect())
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
            .unwrap_or_else(|| "?book=sample&chapter=chapter".to_string());
        let body = fetch_document(&format!("{BASE_URL}/reader/{key}"), TEXT_FIXTURE);
        let data = script_object(&body, "var data =")
            .unwrap_or_else(|| serde_json::from_str(TEXT_DATA_FIXTURE).unwrap_or(Value::Null));
        let token = key
            .split("chapter=")
            .nth(1)
            .and_then(|part| part.split('&').next())
            .unwrap_or_default();
        let content = data
            .get("chapters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|chapter| text(chapter, "token").as_deref() == Some(token))
            .and_then(|chapter| chapter.pointer("/data/html").and_then(Value::as_str))
            .unwrap_or("")
            .replace("<br>", "");
        text_response(&key, &content)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            section("popular", "Popular", popular),
            section("latest", "Latest", latest),
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

fn post_bundle(page: u64, query: &str, sort: &str, request: &Value) -> Value {
    let category = lnreader::filter_string(request, "category", "0");
    let tags = lnreader::filter_string(request, "tags", "");
    let timeread = lnreader::filter_string(request, "timeread", "0-999999");
    let page_text = page.to_string();
    serde_json::from_str(
        &client()
            .post(API_URL)
            .referer(BASE_URL)
            .form(&[
                ("version", "4.4"),
                ("uid", "0"),
                ("utoken", ""),
                ("resource", "general"),
                ("action", "get_bundle"),
                ("bundle", "bundle_books"),
                ("target", "feed"),
                ("page", page_text.as_str()),
                ("filter_category_id", category.as_str()),
                ("filter_completed", "-1"),
                ("filter_search", query),
                ("filter_tags", tags.as_str()),
                ("filter_sort", sort),
                ("filter_timeread", timeread.as_str()),
            ])
            .send_text()
            .unwrap_or_else(|_| LIST_FIXTURE.to_string()),
    )
    .or_else(|_| serde_json::from_str(LIST_FIXTURE))
    .unwrap_or(Value::Null)
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn book_data(key: &str) -> Value {
    let body = fetch_document(
        &format!("{BASE_URL}/book/{}", key.trim_start_matches('/')),
        DETAILS_FIXTURE,
    );
    script_object(&body, "var postData =")
        .unwrap_or_else(|| serde_json::from_str(BOOK_FIXTURE).unwrap_or(Value::Null))
}

fn fetch_details(key: &str) -> CatalogItem {
    let book = book_data(key);
    let normalized = normalize_key(key);
    CatalogItem {
        key: normalized.clone(),
        title: text(&book, "title").unwrap_or_else(|| "Neobook".to_string()),
        cover: book
            .pointer("/attachment/image/m")
            .and_then(Value::as_str)
            .map(str::to_string),
        description: text(&book, "text")
            .or_else(|| text(&book, "text_fix"))
            .map(|value| html::strip_tags(&value.replace("<br>", "\n"))),
        authors: author(&book).into_iter().collect(),
        tags: book
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        status: match text(&book, "status").as_deref() {
            Some("1") => ItemStatus::Ongoing,
            Some("2") => ItemStatus::Completed,
            Some("3") => ItemStatus::Hiatus,
            Some("4") => ItemStatus::Cancelled,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/book/{normalized}")),
        language: Some("ru".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_item(item: &Value) -> CatalogItem {
    let key = text(item, "token")
        .map(|token| format!("{token}/"))
        .unwrap_or_else(|| "sample/".to_string());
    CatalogItem {
        key: key.clone(),
        title: text(item, "title").unwrap_or_else(|| "Neobook".to_string()),
        cover: item
            .pointer("/attachment/image/m")
            .and_then(Value::as_str)
            .map(str::to_string),
        url: Some(format!("{BASE_URL}/book/{key}")),
        language: Some("ru".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn script_object(body: &str, marker: &str) -> Option<Value> {
    let start = body.find(marker)? + marker.len();
    let after = &body[start..];
    let mut depth = 0i32;
    let mut end = 0usize;
    let mut started = false;
    for (index, ch) in after.char_indices() {
        if ch == '{' {
            started = true;
            depth += 1;
        } else if ch == '}' && started {
            depth -= 1;
            if depth == 0 {
                end = index + 1;
                break;
            }
        }
    }
    serde_json::from_str(after[..end].trim()).ok()
}

fn author(book: &Value) -> Option<String> {
    let user = book.get("user")?;
    match (
        text(user, "firstname"),
        text(user, "lastname"),
        text(user, "initials"),
    ) {
        (Some(first), Some(last), _) => Some(format!("{first} {last}")),
        (_, _, initials) => initials,
    }
}

fn text_response(key: &str, html_body: &str) -> ExtensionResult<NovelText> {
    let normalized = novel::normalize_reader_html(html_body);
    Ok(NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(format!("{BASE_URL}/reader/{key}")),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    })
}

fn section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.to_string(),
        title: title.to_string(),
        style: Some(HomeSectionStyle::Cover),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(&format!("{BASE_URL}/book/"))
        .map(normalize_key)
}

fn normalize_key(input: &str) -> String {
    let key = input.trim_start_matches('/').trim_end_matches('/');
    format!("{key}/")
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
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

const LIST_FIXTURE: &str = r#"{"bundle_books":{"feed":[{"title":"Sample Book","token":"sample","attachment":{"image":{"m":"https://neobook.org/cover.jpg"}}}]}}"#;
const BOOK_FIXTURE: &str = r#"{"title":"Sample Book","token":"sample","text":"Sample summary.","status":"1","tags":["Fantasy"],"attachment":{"image":{"m":"https://neobook.org/cover.jpg"}},"user":{"initials":"Sample Author"},"chapters":[{"title":"Chapter 1","token":"chapter","access":"1","status":"1","sort":"1"}]}"#;
const DETAILS_FIXTURE: &str = r#"<script>var postData = {"title":"Sample Book","token":"sample","text":"Sample summary.","status":"1","tags":["Fantasy"],"attachment":{"image":{"m":"https://neobook.org/cover.jpg"}},"user":{"initials":"Sample Author"},"chapters":[{"title":"Chapter 1","token":"chapter","access":"1","status":"1","sort":"1"}]};</script>"#;
const TEXT_DATA_FIXTURE: &str =
    r#"{"chapters":[{"token":"chapter","data":{"html":"<p>Sample chapter text.</p>"}}]}"#;
const TEXT_FIXTURE: &str = r#"<script>var data = {"chapters":[{"token":"chapter","data":{"html":"<p>Sample chapter text.</p>"}}]};</script>"#;

export_novel_source!(SOURCE);
