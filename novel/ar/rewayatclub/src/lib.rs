use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: RewayatClub = RewayatClub;
const BASE_URL: &str = "https://rewayat.club";
const API_URL: &str = "https://api.rewayat.club";

struct RewayatClub;

impl NovelSource for RewayatClub {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = page(&request);
        let latest = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            == Some("latest");
        let target = if latest {
            format!("{BASE_URL}/api/chapters/weekly/list/?page={page}")
        } else {
            format!("{API_URL}/api/novels/?type=0&ordering=-num_chapters&page={page}")
        };
        Ok(parse_listing(&fetch_or_fixture(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
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
        let target = format!(
            "{API_URL}/api/novels/?type=0&ordering=-num_chapters&page={page}&search={}",
            url::query_escape(query)
        );
        Ok(parse_listing(&fetch_or_fixture(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        Ok(parse_details(
            &fetch_or_fixture(&novel_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        Ok(self.chapters_page(request)?.entries)
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        let page = page(&request);
        let slug = key.trim_start_matches("novel/").trim_matches('/');
        let target = format!("{API_URL}/api/chapters/{slug}/?ordering=number&page={page}");
        let body = fetch_or_fixture(&target, CHAPTERS_FIXTURE);
        Ok(NovelChapterPage {
            entries: parse_chapters(&body, &key),
            has_next_page: has_next_page(&body),
            next_page: has_next_page(&body).then_some(page as u32 + 1),
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key =
            novel::request_key(&request, "chapter").unwrap_or_else(|| "novel/sample/1".to_string());
        let target = format!(
            "{BASE_URL}/api/chapters/{}",
            key.trim_start_matches("novel/")
        );
        Ok(parse_text(&fetch_or_fixture(&target, TEXT_FIXTURE), &key))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let listing = parse_listing(LIST_FIXTURE);
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: listing.entries,
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
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn novel_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let entries = root
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(parse_listing_item)
        .collect();
    Paged {
        entries,
        has_next_page: root
            .get("next")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()),
    }
}

fn parse_listing_item(item: &Value) -> CatalogItem {
    let novel_node = item.get("novel").unwrap_or(item);
    let slug = text(item, "slug")
        .or_else(|| text(novel_node, "slug"))
        .unwrap_or_else(|| "sample".to_string());
    let cover = text(item, "poster_url")
        .or_else(|| text(novel_node, "poster_url"))
        .map(api_asset);
    CatalogItem {
        key: format!("novel/{slug}"),
        title: text(item, "arabic")
            .or_else(|| text(novel_node, "arabic"))
            .or_else(|| text(item, "english"))
            .or_else(|| text(novel_node, "english"))
            .unwrap_or_else(|| "Novel".to_string()),
        cover,
        url: Some(format!("{BASE_URL}/novel/{slug}")),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let status_text = texts_in(body, "v-chip__content")
        .into_iter()
        .find(|text| ["مكتملة", "متوقفة", "مستمرة"].contains(&text.as_str()))
        .unwrap_or_default();
    CatalogItem {
        key: key.to_string(),
        title: text_between_marker(body, "h1.primary--text", "</h1>")
            .or_else(|| text_between_marker(body, "<h1", "</h1>"))
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: poster_from_nuxt(body)
            .or_else(|| html::attr_after(body, "<img", "src").map(api_asset)),
        description: text_between_marker(body, "text-pre-line", "</div>"),
        authors: text_between_marker(body, "novel-author", "</")
            .into_iter()
            .collect(),
        tags: texts_in(body, "v-slide-group__content a"),
        status: match status_text.as_str() {
            "مكتملة" => ItemStatus::Completed,
            "متوقفة" => ItemStatus::Hiatus,
            "مستمرة" => ItemStatus::Ongoing,
            _ => ItemStatus::Unknown,
        },
        url: Some(novel_url(key)),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, novel_key: &str) -> Vec<NovelChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    root.get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            let number = item.get("number").and_then(Value::as_f64).unwrap_or(1.0);
            let key = format!("{}/{}", novel_key.trim_end_matches('/'), number as u64);
            NovelChapter {
                key: key.clone(),
                title: text(item, "title"),
                chapter_number: Some(number as f32),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ar".to_string()),
                ..NovelChapter::default()
            }
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let title = text(&root, "title");
    let html = root
        .get("content")
        .map(content_html)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "<p>Fixture chapter text.</p>".to_string());
    NovelText {
        title,
        text: Some(html::strip_tags(&html)),
        html: Some(html.clone()),
        base_url: Some(BASE_URL.to_string()),
        css: Some("body { line-height: 1.8; direction: rtl; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        next_chapter_key: next_numeric_key(key),
        ..NovelText::default()
    }
}

fn content_html(value: &Value) -> String {
    match value {
        Value::String(text) => format!("<p>{}</p>", html::html_unescape(text)),
        Value::Array(items) => items
            .iter()
            .map(content_html)
            .collect::<Vec<_>>()
            .join("<br>"),
        _ => String::new(),
    }
}

fn has_next_page(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|root| {
            root.get("next")
                .and_then(Value::as_str)
                .map(|value| !value.is_empty())
        })
        .unwrap_or(false)
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn api_asset(path: String) -> String {
    if path.starts_with("http") {
        path
    } else {
        format!("{API_URL}/{}", path.trim_start_matches('/'))
    }
}

fn poster_from_nuxt(body: &str) -> Option<String> {
    let start = body.find("poster_url:")?;
    let rest = &body[start..];
    let quoted = rest.split('"').nth(1)?;
    Some(api_asset(quoted.replace("\\u002F", "/")))
}

fn text_between_marker(body: &str, marker: &str, end: &str) -> Option<String> {
    html::text_between(body, marker, end)
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn texts_in(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn next_numeric_key(key: &str) -> Option<String> {
    let (prefix, number) = key.rsplit_once('/')?;
    let next = number.parse::<u64>().ok()? + 1;
    Some(format!("{prefix}/{next}"))
}

const LIST_FIXTURE: &str = r#"{"next":null,"results":[{"arabic":"Sample Novel","slug":"sample","poster_url":"/media/sample.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="primary--text"><span>Sample Novel</span></h1><div class="novel-author">Sample Author</div><div class="text-pre-line"><span>Sample description.</span></div>"#;
const CHAPTERS_FIXTURE: &str =
    r#"{"next":null,"results":[{"number":1,"title":"Chapter 1","date":"2024-01-01"}]}"#;
const TEXT_FIXTURE: &str = r#"{"title":"Chapter 1","content":["The first fixture paragraph."]}"#;

export_novel_source!(SOURCE);
