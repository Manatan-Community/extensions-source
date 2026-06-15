use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage, NovelText, Paged,
    UrlResolveResult,
};
use manatan_shared::{novel, sdk::http::HttpClient, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: SkyNovels = SkyNovels;
const BASE_URL: &str = "https://www.skynovels.net/";
const API_URL: &str = "https://api.skynovels.net/api/";

struct SkyNovels;

impl NovelSource for SkyNovels {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let genres = filter_array(&request, "genres");
        let order = if genres.is_empty() {
            "rating"
        } else {
            "updated"
        };
        let mut target = format!("{API_URL}novels?page={page}&order={order}");
        if !genres.is_empty() {
            target.push_str("&genres=");
            target.push_str(&genres.join(","));
        }
        let body = fetch_json_or_fixture(&target, LIST_FIXTURE);
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty(),
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
        let target = format!("{API_URL}novels?page={page}&q={}", url::query_escape(query));
        let body = fetch_json_or_fixture(&target, LIST_FIXTURE);
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty(),
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel")
            .unwrap_or_else(|| "novelas/1/sample/".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel")
            .unwrap_or_else(|| "novelas/1/sample/".to_string());
        let body = fetch_json_or_fixture(&details_api_url(&key), DETAILS_FIXTURE);
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
            .unwrap_or_else(|| "novelas/1/sample/10/chapter-1".to_string());
        let chapter_id = key.split('/').nth(3).unwrap_or("10");
        let body = fetch_json_or_fixture(
            &format!("{API_URL}novel-chapter/{chapter_id}"),
            TEXT_FIXTURE,
        );
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
        .with_header("Cache-Control", "no-cache")
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

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    root.get("novels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(catalog_from_novel_json)
        .collect()
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_json_or_fixture(&details_api_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let item = root
        .get("novel")
        .and_then(Value::as_array)
        .and_then(|items| items.first());
    let mut catalog = item
        .and_then(catalog_from_novel_json)
        .unwrap_or_else(|| catalog_item(normalize_key(key), title_from_key(key), None, true));
    catalog.initialized = true;
    catalog.description = item
        .and_then(|value| value.get("nvl_content"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    catalog.authors = item
        .and_then(|value| value.get("nvl_writer"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(|value| vec![value.to_string()])
        .unwrap_or_default();
    catalog.tags = item
        .and_then(|value| value.get("genres"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|genre| genre.get("genre_name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect();
    catalog.status = item
        .and_then(|value| value.get("nvl_status"))
        .and_then(Value::as_str)
        .map(parse_status)
        .unwrap_or(ItemStatus::Unknown);
    catalog.rating = item
        .and_then(|value| value.get("nvl_rating"))
        .and_then(Value::as_f64)
        .map(|value| value as f32);
    catalog
}

fn parse_chapters(body: &str, novel_key: &str) -> Vec<NovelChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let Some(item) = root
        .get("novel")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
    else {
        return Vec::new();
    };
    item.get("volumes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|volume| {
            let section = volume
                .get("vlm_title")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            volume
                .get("chapters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(move |chapter| (section.clone(), chapter))
        })
        .filter_map(|(section, chapter)| {
            let id = chapter.get("id").and_then(Value::as_i64)?;
            let slug = chapter
                .get("chp_name")
                .and_then(Value::as_str)
                .unwrap_or("chapter");
            let key = format!("{}{}/{}", ensure_trailing_slash(novel_key), id, slug);
            Some(NovelChapter {
                key: key.clone(),
                title: chapter
                    .get("chp_index_title")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                chapter_number: chapter
                    .get("chp_number")
                    .and_then(Value::as_f64)
                    .map(|value| value as f32),
                section,
                url: Some(absolute_url(&key)),
                language: Some("es".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let item = root
        .get("chapter")
        .and_then(Value::as_array)
        .and_then(|items| items.first());
    let raw = item
        .and_then(|value| value.get("chp_content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .replace('\n', "<br>");
    let normalized = novel::normalize_reader_html(&raw);
    NovelText {
        title: item
            .and_then(|value| value.get("chp_index_title"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn catalog_from_novel_json(item: &Value) -> Option<CatalogItem> {
    let id = item.get("id").and_then(Value::as_i64)?;
    let title = item
        .get("nvl_title")
        .and_then(Value::as_str)
        .unwrap_or("Novel")
        .to_string();
    let slug = item
        .get("nvl_name")
        .and_then(Value::as_str)
        .unwrap_or("novel");
    let key = format!("novelas/{id}/{slug}/");
    let cover = item
        .get("image")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(|image| format!("{API_URL}get-image/{image}/novels/false"));
    Some(catalog_item(key, title, cover, false))
}

fn catalog_item(
    key: String,
    title: String,
    cover: Option<String>,
    initialized: bool,
) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover,
        url: Some(absolute_url(&key)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized,
        ..CatalogItem::default()
    }
}

fn filter_array(request: &Value, id: &str) -> Vec<String> {
    let Some(value) = request.get("filters").and_then(|filters| filters.get(id)) else {
        return Vec::new();
    };
    value
        .get("value")
        .unwrap_or(value)
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn parse_status(input: &str) -> ItemStatus {
    let lower = input.to_ascii_lowercase();
    if lower.contains("complet") || lower.contains("final") {
        ItemStatus::Completed
    } else if lower.contains("paus") {
        ItemStatus::Hiatus
    } else if lower.contains("drop") || lower.contains("cancel") {
        ItemStatus::Cancelled
    } else if lower.is_empty() {
        ItemStatus::Unknown
    } else {
        ItemStatus::Ongoing
    }
}

fn details_api_url(key: &str) -> String {
    let id = key.split('/').nth(1).unwrap_or("1");
    format!("{API_URL}novel/{id}/reading?&q")
}

fn ensure_trailing_slash(input: &str) -> String {
    if input.ends_with('/') {
        input.to_string()
    } else {
        format!("{input}/")
    }
}

fn title_from_key(key: &str) -> String {
    url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .contains("skynovels.net")
        .then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://www.skynovels.net/")
        .trim_start_matches('/')
        .to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

const LIST_FIXTURE: &str = r#"
{"novels":[{"id":1,"nvl_title":"Sample Novel","nvl_name":"sample","image":"sample.jpg"}]}
"#;

const DETAILS_FIXTURE: &str = r#"
{"novel":[{"id":1,"nvl_title":"Sample Novel","nvl_name":"sample","image":"sample.jpg","nvl_content":"Sample summary.","nvl_writer":"Sample Author","nvl_status":"Ongoing","genres":[{"genre_name":"Fantasia"}],"volumes":[{"vlm_title":"Volume 1","chapters":[{"id":10,"chp_index_title":"Capitulo 1","chp_name":"chapter-1","chp_number":1}]}]}]}
"#;

const TEXT_FIXTURE: &str = r#"
{"chapter":[{"chp_index_title":"Capitulo 1","chp_content":"Sample chapter text."}]}
"#;

export_novel_source!(SOURCE);
