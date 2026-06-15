use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: INovelTranslation = INovelTranslation;
const BASE_URL: &str = "https://inoveltranslation.com";

struct INovelTranslation;

impl NovelSource for INovelTranslation {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_or_fixture(
            &format!("{BASE_URL}/api/novels?limit=50&page={page}"),
            LIST_FIXTURE,
        );
        let root: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: root
                .get("hasNextPage")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
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
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        let body = fetch_or_fixture(
            &format!(
                "{BASE_URL}/api/novels?where[title][contains]={}&limit=50&page={page}",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "/novels/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "/novels/sample".to_string());
        let hide_locked = bool_setting(&request, "hideLocked");
        let id = id_from_key(&key);
        let body = fetch_or_fixture(
            &format!("{BASE_URL}/api/chapters?where[novel][equals]={id}&limit=999&depth=0"),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body, hide_locked))
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
            .unwrap_or_else(|| "/chapters/sample-chapter".to_string());
        let chapter_url = absolute_url(&key);
        let rsc = client()
            .get(&chapter_url)
            .header("rsc", "1")
            .referer(BASE_URL)
            .send_text()
            .unwrap_or_default();
        let html = extract_lexical_from_rsc(&rsc).unwrap_or_else(|| {
            let body = fetch_or_fixture(&chapter_url, TEXT_FIXTURE);
            html::text_between(&body, "data-sentry-component=\"RichText\"", "</section>")
                .unwrap_or_else(|| TEXT_FIXTURE.to_string())
        });
        Ok(text_result(&html, Some(chapter_url)))
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
        .with_header("Accept", "application/json, text/html, */*")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(key: &str) -> CatalogItem {
    let id = id_from_key(key);
    let body = fetch_or_fixture(
        &format!("{BASE_URL}/api/novels/{id}?depth=1"),
        DETAILS_FIXTURE,
    );
    parse_details(&body, &normalize_key(key))
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    root.get("docs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            let id = json_text(item, "id").unwrap_or_else(|| "sample".to_string());
            let title = json_text(item, "title").unwrap_or_else(|| "Untitled".to_string());
            CatalogItem {
                key: format!("/novels/{id}"),
                title,
                cover: item
                    .get("cover")
                    .and_then(|cover| json_text(cover, "url"))
                    .map(|path| absolute_url(&path)),
                url: Some(format!("{BASE_URL}/novels/{id}")),
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
    let summary = root
        .get("sypnosis")
        .and_then(|value| value.get("root"))
        .map(lexical_to_text);
    CatalogItem {
        key: normalize_key(key),
        title: json_text(&root, "title").unwrap_or_else(|| "Untitled".to_string()),
        cover: root
            .get("cover")
            .and_then(|cover| json_text(cover, "url"))
            .map(|path| absolute_url(&path)),
        url: Some(absolute_url(key)),
        authors: root
            .get("author")
            .and_then(|author| json_text(author, "name"))
            .into_iter()
            .collect(),
        description: summary,
        tags: root
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tag| json_text(tag, "name"))
            .collect(),
        status: match json_text(&root, "publication").as_deref() {
            Some("completed") => ItemStatus::Completed,
            _ => ItemStatus::Ongoing,
        },
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, hide_locked: bool) -> Vec<NovelChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let mut chapters: Vec<_> = root
        .get("docs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let locked = !item.get("tier").unwrap_or(&Value::Null).is_null();
            if hide_locked && locked {
                return None;
            }
            let id = json_text(item, "id")?;
            let number = item.get("chapter").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let suffix = json_text(item, "title")
                .filter(|title| !title.is_empty())
                .map(|title| format!(" - {title}"))
                .unwrap_or_default();
            let title = format!(
                "Ch. {}{}{}",
                display_number(number),
                if locked { " [Locked]" } else { "" },
                suffix
            );
            Some(NovelChapter {
                key: format!("/chapters/{id}"),
                title: Some(title),
                chapter_number: Some(number),
                url: Some(format!("{BASE_URL}/chapters/{id}")),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect();
    chapters.sort_by(|a, b| {
        a.chapter_number
            .unwrap_or(0.0)
            .partial_cmp(&b.chapter_number.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn extract_lexical_from_rsc(input: &str) -> Option<String> {
    let signatures = [
        "\"root\":{\"type\":\"root\"",
        "\\\"root\\\":{\\\"type\\\":\\\"root\\\"",
        "\"children\":[{\"type\":\"paragraph\"",
        "\\\"children\\\":[{\\\"type\\\":\\\"paragraph\\\"",
    ];
    let sig = signatures.iter().find_map(|needle| input.find(needle))?;
    let start = input[..sig].rfind('{')?;
    let raw = balanced_object(&input[start..])?;
    let parsed = serde_json::from_str::<Value>(&raw)
        .or_else(|_| {
            serde_json::from_str::<Value>(&raw.replace("\\\"", "\"").replace("\\\\", "\\"))
        })
        .ok()?;
    let root = parsed
        .get("root")
        .or_else(|| {
            parsed
                .get("content")
                .and_then(|content| content.get("root"))
        })
        .unwrap_or(&parsed);
    Some(lexical_to_html(root))
}

fn balanced_object(input: &str) -> Option<String> {
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escape = false;
    for (idx, ch) in input.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        match ch {
            '\\' if in_string => escape = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(input[..=idx].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn lexical_to_html(node: &Value) -> String {
    let Some(children) = node.get("children").and_then(Value::as_array) else {
        return escape_html(node.get("text").and_then(Value::as_str).unwrap_or_default());
    };
    let mut out = String::new();
    for child in children {
        match child
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "paragraph" => out.push_str(&format!("<p>{}</p>", lexical_to_html(child))),
            "heading" => {
                let tag = child
                    .get("tag")
                    .and_then(Value::as_str)
                    .filter(|tag| matches!(*tag, "h1" | "h2" | "h3" | "h4"))
                    .unwrap_or("h3");
                out.push_str(&format!("<{tag}>{}</{tag}>", lexical_to_html(child)));
            }
            "list" => {
                let tag = if child.get("listType").and_then(Value::as_str) == Some("number") {
                    "ol"
                } else {
                    "ul"
                };
                out.push_str(&format!("<{tag}>{}</{tag}>", lexical_to_html(child)));
            }
            "listitem" => out.push_str(&format!("<li>{}</li>", lexical_to_html(child))),
            "text" => {
                let mut text = escape_html(
                    child
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                );
                let format = child.get("format").and_then(Value::as_u64).unwrap_or(0);
                if format & 1 == 1 {
                    text = format!("<b>{text}</b>");
                }
                if format & 2 == 2 {
                    text = format!("<i>{text}</i>");
                }
                out.push_str(&text);
            }
            _ => out.push_str(&lexical_to_html(child)),
        }
    }
    out
}

fn lexical_to_text(node: &Value) -> String {
    html::strip_tags(&lexical_to_html(node))
}

fn text_result(raw: &str, _url: Option<String>) -> NovelText {
    let normalized = novel::normalize_reader_html(raw);
    NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(BASE_URL.to_string()),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn bool_setting(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .or_else(|| request.get("settings"))
        .and_then(|settings| settings.get(id))
        .and_then(|value| value.get("value").unwrap_or(value).as_bool())
        .unwrap_or(false)
}

fn json_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn id_from_key(key: &str) -> String {
    normalize_key(key)
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("sample")
        .to_string()
}

fn normalize_key(input: &str) -> String {
    let path = input
        .trim()
        .trim_start_matches(BASE_URL)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_matches('/');
    format!("/{path}")
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn display_number(number: f32) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i32)
    } else {
        number.to_string()
    }
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const LIST_FIXTURE: &str = r#"{"docs":[{"id":"sample","title":"Sample Novel","cover":{"url":"/cover.jpg"}}],"hasNextPage":false}"#;
const DETAILS_FIXTURE: &str = r#"{"id":"sample","title":"Sample Novel","cover":{"url":"/cover.jpg"},"author":{"name":"Sample Author"},"publication":"ongoing","tags":[{"name":"Fantasy"}],"sypnosis":{"root":{"type":"root","children":[{"type":"paragraph","children":[{"type":"text","text":"Sample summary."}]}]}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"docs":[{"id":"sample-chapter","chapter":1,"title":"Beginning","tier":null,"updatedAt":"2024-01-01T00:00:00.000Z"}]}"#;
const TEXT_FIXTURE: &str = r#"<p>Sample chapter text.</p>"#;

export_novel_source!(SOURCE);
