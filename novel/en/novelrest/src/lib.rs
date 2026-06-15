use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    dates, novel,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: NovelRest = NovelRest;
const BASE_URL: &str = "https://novelrest.vercel.app";
const API_BASE: &str = "https://novelrest.vercel.app/api/lnreader";

struct NovelRest;

impl NovelSource for NovelRest {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let mut target = format!("{API_BASE}/novels?page={page}&limit=20");
        let sort = if listing == "latest" {
            "latest".to_string()
        } else {
            filter_string(&request, "sort", "popular")
        };
        target.push_str("&sort=");
        target.push_str(&url::query_escape(&sort));
        if let Some(status) = filter_string_opt(&request, "status") {
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
            "{API_BASE}/novels?q={}&page={page}&limit=20",
            url::query_escape(query)
        );
        Ok(parse_listing(&fetch_json_or_fixture(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        let data = fetch_details_json(&key);
        Ok(parse_chapters(&data, &slug_from_key(&key)))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".to_string());
        let (slug, chapter) = key.rsplit_once('/').unwrap_or(("sample", "1"));
        let target = format!("{API_BASE}/novels/{slug}/chapters/{chapter}");
        let body = fetch_json_or_fixture(&target, TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: popular.entries,
            has_more: popular.has_next_page,
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
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details_json(key: &str) -> Value {
    let slug = slug_from_key(key);
    serde_json::from_str(&fetch_json_or_fixture(
        &format!("{API_BASE}/novels/{slug}"),
        DETAILS_FIXTURE,
    ))
    .unwrap_or(Value::Null)
}

fn fetch_details(key: &str) -> CatalogItem {
    parse_details(&fetch_details_json(key), &slug_from_key(key))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let entries = root
        .get("novels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(parse_listing_item)
        .collect();
    Paged {
        entries,
        has_next_page: root
            .get("hasNextPage")
            .and_then(Value::as_bool)
            .or_else(|| root.get("hasMore").and_then(Value::as_bool))
            .unwrap_or(false),
    }
}

fn parse_listing_item(item: &Value) -> CatalogItem {
    let slug = text(item, "slug").unwrap_or_else(|| "sample".to_string());
    CatalogItem {
        key: slug.clone(),
        title: text(item, "title").unwrap_or_else(|| "Novel".to_string()),
        cover: text(item, "coverImage"),
        url: Some(format!("{BASE_URL}/novels/{slug}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(data: &Value, slug: &str) -> CatalogItem {
    CatalogItem {
        key: slug.to_string(),
        title: text(data, "title").unwrap_or_else(|| "Novel".to_string()),
        cover: text(data, "coverImage"),
        description: text(data, "description"),
        authors: text(data, "author").into_iter().collect(),
        tags: data
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| {
                genre
                    .as_str()
                    .map(ToString::to_string)
                    .or_else(|| text(genre, "name"))
            })
            .collect(),
        status: parse_status(text(data, "status").as_deref()),
        url: Some(format!("{BASE_URL}/novels/{slug}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(data: &Value, slug: &str) -> Vec<NovelChapter> {
    data.get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|chapter| {
            let number = chapter.get("number").and_then(Value::as_f64).unwrap_or(1.0);
            let display = if number.fract() == 0.0 {
                format!("{}", number as i64)
            } else {
                number.to_string()
            };
            NovelChapter {
                key: format!("{slug}/{display}"),
                title: text(chapter, "title").or_else(|| Some(format!("Chapter {display}"))),
                chapter_number: Some(number as f32),
                date_uploaded: text(chapter, "createdAt")
                    .and_then(|date| dates::parse_fixture_date(&date)),
                url: Some(format!("{BASE_URL}/novels/{slug}/{display}")),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            }
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let html = text(&root, "contentHtml")
        .unwrap_or_else(|| "<p>Chapter content could not be loaded.</p>".to_string());
    let normalized = novel::normalize_reader_html(&html);
    NovelText {
        title: text(&root, "title"),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(format!("{BASE_URL}/novels/{key}")),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; }".to_string()),
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
        "COMPLETED" => ItemStatus::Completed,
        "ONGOING" => ItemStatus::Ongoing,
        "HIATUS" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn filter_string(request: &Value, key: &str, default: &str) -> String {
    filter_string_opt(request, key).unwrap_or_else(|| default.to_string())
}

fn filter_string_opt(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn key_from_url(input: &str) -> Option<String> {
    input.contains("novelrest.vercel.app/novels/").then(|| {
        input
            .split("/novels/")
            .nth(1)
            .unwrap_or(input)
            .trim_matches('/')
            .to_string()
    })
}

fn slug_from_key(key: &str) -> String {
    key.trim_start_matches("/novels/")
        .trim_matches('/')
        .split('/')
        .next()
        .unwrap_or("sample")
        .to_string()
}

const LIST_FIXTURE: &str = r#"{"novels":[{"title":"Sample Novel","slug":"sample","coverImage":"https://novelrest.vercel.app/cover.jpg"}],"hasNextPage":false}"#;
const DETAILS_FIXTURE: &str = r#"{"title":"Sample Novel","slug":"sample","author":"Sample Author","coverImage":"https://novelrest.vercel.app/cover.jpg","description":"Sample summary.","status":"ONGOING","genres":[{"name":"Fantasy"}],"chapters":[{"title":"Chapter 1","number":1,"createdAt":"2024-01-01"}]}"#;
const TEXT_FIXTURE: &str = r#"{"title":"Chapter 1","contentHtml":"<p>Sample chapter text.</p>"}"#;

export_novel_source!(SOURCE);
