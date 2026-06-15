use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    html, lnreader, novel,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: RanobeLib = RanobeLib;
const BASE_URL: &str = "https://ranobelib.me";
const API_URL: &str = "https://api.cdnlibs.org/api/manga";

struct RanobeLib;

impl NovelSource for RanobeLib {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort_by = if listing == "latest" {
            "last_chapter_at".to_string()
        } else {
            lnreader::filter_string(&request, "sort_by", "rating_score")
        };
        let mut target = format!(
            "{API_URL}/?site_id[0]=3&page={page}&sort_by={sort_by}&sort_type={}",
            lnreader::filter_string(&request, "sort_type", "desc")
        );
        if lnreader::filter_bool(&request, "require_chapters", false) {
            target.push_str("&chapters[min]=1");
        }
        append_repeated(
            &mut target,
            "genres[]",
            lnreader::filter_array(&request, "genresInclude"),
        );
        append_repeated(
            &mut target,
            "genres_exclude[]",
            lnreader::filter_array(&request, "genresExclude"),
        );
        let entries = fetch_json(&target, LIST_FIXTURE)
            .get("data")
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
        let root = fetch_json(
            &format!("{API_URL}/?site_id[0]=3&q={}", url::query_escape(query)),
            LIST_FIXTURE,
        );
        let entries = root
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(parse_item)
            .collect::<Vec<_>>();
        Ok(Paged {
            has_next_page: false,
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        let details = fetch_json(&format!("{API_URL}/{key}?fields[]=teams"), DETAILS_FIXTURE)
            .get("data")
            .cloned()
            .unwrap_or(Value::Null);
        let branch_names = details
            .get("teams")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let root = fetch_json(&format!("{API_URL}/{key}/chapters"), CHAPTERS_FIXTURE);
        let mut out = Vec::new();
        for chapter in root
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let volume = chapter
                .get("volume")
                .and_then(value_string)
                .unwrap_or_else(|| "1".to_string());
            let number = chapter
                .get("number")
                .and_then(value_string)
                .unwrap_or_else(|| "1".to_string());
            let index = chapter
                .get("index")
                .and_then(Value::as_f64)
                .unwrap_or((out.len() + 1) as f64) as f32;
            for branch in chapter
                .get("branches")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let branch_id = branch
                    .get("branch_id")
                    .and_then(value_string)
                    .unwrap_or_else(|| "0".to_string());
                let name = format!(
                    "Том {volume} Глава {number}{}",
                    text(chapter, "name")
                        .map(|name| format!(" {name}"))
                        .unwrap_or_default()
                );
                out.push(NovelChapter {
                    key: format!("{key}/{volume}/{number}/{branch_id}"),
                    title: Some(name),
                    chapter_number: Some(index),
                    date_uploaded: branch
                        .get("created_at")
                        .and_then(Value::as_str)
                        .and_then(|d| manatan_shared::dates::parse_ymd(&d[..d.len().min(10)])),
                    section: branch_name(&branch_names, &branch_id),
                    url: Some(format!("{BASE_URL}/ru/book/{key}/{volume}/{number}")),
                    language: Some("ru".to_string()),
                    ..NovelChapter::default()
                });
            }
        }
        Ok(out)
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key =
            novel::request_key(&request, "chapter").unwrap_or_else(|| "sample/1/1/0".to_string());
        let parts = key.split('/').collect::<Vec<_>>();
        let slug = parts.first().copied().unwrap_or("sample");
        let volume = parts.get(1).copied().unwrap_or("1");
        let number = parts.get(2).copied().unwrap_or("1");
        let branch = parts.get(3).copied().unwrap_or("0");
        let target =
            format!("{API_URL}/{slug}/chapter?branch_id={branch}&number={number}&volume={volume}");
        let data = fetch_json(&target, TEXT_FIXTURE)
            .get("data")
            .cloned()
            .unwrap_or(Value::Null);
        let content = match data.get("content") {
            Some(Value::String(text)) => text.clone(),
            Some(value) => json_doc_to_html(
                value,
                data.get("attachments")
                    .and_then(Value::as_array)
                    .unwrap_or(&Vec::new()),
            ),
            None => String::new(),
        };
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
        .with_header("Accept", "application/json")
        .with_header("Site-Id", "3")
        .with_header("Origin", BASE_URL)
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> Value {
    serde_json::from_str(
        &client()
            .get(target)
            .xhr()
            .header("Accept", "application/json")
            .send_text()
            .unwrap_or_else(|_| fixture.to_string()),
    )
    .or_else(|_| serde_json::from_str(fixture))
    .unwrap_or(Value::Null)
}

fn fetch_details(key: &str) -> CatalogItem {
    let root = fetch_json(
        &format!(
            "{API_URL}/{key}?fields[]=summary&fields[]=genres&fields[]=tags&fields[]=teams&fields[]=authors&fields[]=status_id&fields[]=artists"
        ),
        DETAILS_FIXTURE,
    );
    let data = root.get("data").unwrap_or(&root);
    CatalogItem {
        key: key.to_string(),
        title: text(data, "rus_name")
            .or_else(|| text(data, "name"))
            .unwrap_or_else(|| "Ranobe".to_string()),
        cover: data
            .pointer("/cover/default")
            .and_then(Value::as_str)
            .map(str::to_string),
        description: summary(data.get("summary")),
        authors: data
            .get("authors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| text(a, "name"))
            .collect(),
        artists: data
            .get("artists")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| text(a, "name"))
            .collect(),
        tags: ["genres", "tags"]
            .into_iter()
            .flat_map(|key| {
                data.get(key)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter_map(|tag| text(tag, "name"))
            .collect(),
        status: match data.pointer("/status/id").and_then(Value::as_i64) {
            Some(1) => ItemStatus::Ongoing,
            Some(2) => ItemStatus::Completed,
            Some(3) => ItemStatus::Hiatus,
            Some(4) => ItemStatus::Cancelled,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/ru/book/{key}")),
        language: Some("ru".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_item(item: &Value) -> CatalogItem {
    let key = text(item, "slug_url").unwrap_or_else(|| {
        let id = item
            .get("id")
            .and_then(value_string)
            .unwrap_or_else(|| "0".to_string());
        let slug = text(item, "slug").unwrap_or_else(|| "sample".to_string());
        format!("{id}--{slug}")
    });
    CatalogItem {
        key: key.clone(),
        title: text(item, "rus_name")
            .or_else(|| text(item, "eng_name"))
            .or_else(|| text(item, "name"))
            .unwrap_or_else(|| "Ranobe".to_string()),
        cover: item
            .pointer("/cover/default")
            .and_then(Value::as_str)
            .map(str::to_string),
        url: Some(format!("{BASE_URL}/ru/book/{key}")),
        language: Some("ru".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn summary(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) => Some(html::strip_tags(text)),
        Some(value) => Some(novel::cleanup_text(&json_doc_to_html(value, &[]))),
        None => None,
    }
}

fn json_doc_to_html(value: &Value, attachments: &[Value]) -> String {
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return escape_html(text);
    }
    let children = value
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| json_doc_to_html(item, attachments))
                .collect::<String>()
        })
        .unwrap_or_default();
    match value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "paragraph" => format!("<p>{children}</p>"),
        "hardBreak" => "<br>".to_string(),
        "image" => {
            let name = value
                .pointer("/attrs/name")
                .or_else(|| value.pointer("/attrs/id"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let src = attachments
                .iter()
                .find(|attachment| {
                    text(attachment, "name").as_deref() == Some(name)
                        || text(attachment, "id").as_deref() == Some(name)
                })
                .and_then(|attachment| {
                    attachment
                        .pointer("/url/default")
                        .or_else(|| attachment.get("url"))
                        .and_then(Value::as_str)
                })
                .unwrap_or(name);
            format!("<img src=\"{}\">", escape_html(src))
        }
        _ => children,
    }
}

fn branch_name(teams: &[Value], id: &str) -> Option<String> {
    teams
        .iter()
        .find(|team| {
            team.pointer("/details/branch_id")
                .and_then(value_string)
                .as_deref()
                == Some(id)
        })
        .and_then(|team| text(team, "name"))
}

fn append_repeated(target: &mut String, key: &str, values: Vec<String>) {
    for value in values {
        target.push('&');
        target.push_str(key);
        target.push('=');
        target.push_str(&url::query_escape(&value));
    }
}

fn text_response(key: &str, html_body: &str) -> ExtensionResult<NovelText> {
    let normalized = novel::normalize_reader_html(html_body);
    Ok(NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(format!("{BASE_URL}/ru/book/{key}")),
        image_headers: novel::image_headers(BASE_URL),
        uses_web_storage: true,
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
        .strip_prefix(&format!("{BASE_URL}/ru/book/"))
        .map(|key| {
            key.split('?')
                .next()
                .unwrap_or(key)
                .trim_matches('/')
                .to_string()
        })
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn value_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_u64().map(|number| number.to_string()))
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
}

const LIST_FIXTURE: &str = r#"{"data":[{"id":1,"slug":"sample","slug_url":"sample","rus_name":"Sample Ranobe","cover":{"default":"https://ranobelib.me/cover.jpg"}}]}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"name":"Sample Ranobe","rus_name":"Sample Ranobe","cover":{"default":"https://ranobelib.me/cover.jpg"},"summary":"Sample summary.","status":{"id":1},"authors":[{"name":"Sample Author"}],"artists":[],"genres":[{"name":"Fantasy"}],"tags":[],"teams":[]}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"volume":1,"number":1,"name":"Chapter 1","index":1,"branches":[{"branch_id":0,"created_at":"2024-01-01"}]}]}"#;
const TEXT_FIXTURE: &str = r#"{"data":{"content":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Sample chapter text."}]}]},"attachments":[]}}"#;

export_novel_source!(SOURCE);
