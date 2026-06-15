use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Mangitto = Mangitto;
const BASE_URL: &str = "https://mangtto.com";
const CDN_URL: &str = "https://cdn.zukrein.com";

struct Mangitto;

impl MangaSource for Mangitto {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_list(LIST_FIXTURE, &["data", "mangas"], true));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let (target, path) = if listing == "latest" {
            (
                format!("{BASE_URL}/api/manga/last-added?page={page}"),
                vec!["data", "chapters"],
            )
        } else {
            (
                format!("{BASE_URL}/api/manga/trends?page={page}"),
                vec!["data", "mangas"],
            )
        };
        let body = fetch_json_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_manga_list(&body, &path, listing == "popular"))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_json_or_fixture(&format!("{BASE_URL}/api/manga/{key}"), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details_json(&body, Some(key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = format!(
            "{BASE_URL}/api/manga/search?page={page}&q={}&genre={}&isAdult={}&isFinished={}&meanScore={}&releaseDate={}",
            url::query_escape(query),
            url::query_escape(filter_str(filters, "genre").unwrap_or("")),
            filter_bool(filters, "isAdult"),
            filter_bool(filters, "isFinished"),
            url::query_escape(filter_str(filters, "meanScore").filter(|v| !v.is_empty()).unwrap_or("0")),
            url::query_escape(filter_str(filters, "releaseDate").filter(|v| !v.is_empty()).unwrap_or("0"))
        );
        let body = fetch_json_or_fixture(&target, SEARCH_FIXTURE);
        Ok(parse_search(&body, page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = fetch_json_or_fixture(&format!("{BASE_URL}/api/manga/{key}"), DETAILS_FIXTURE);
        Ok(parse_details_json(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let mut chapters = Vec::new();
        let mut page = 1;
        loop {
            let target = format!("{BASE_URL}/api/manga/{key}/chapters?page={page}");
            let body = fetch_json_or_fixture(&target, CHAPTERS_FIXTURE);
            let Ok(root) = serde_json::from_str::<Value>(&body) else {
                break;
            };
            let data = root.get("data").unwrap_or(&Value::Null);
            for chapter in data
                .get("chapters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(chapter_number) = value_f32(chapter.get("chapter")) {
                    let number = trim_float(chapter_number as f64);
                    chapters.push(MangaChapter {
                        key: format!("{key}/{number}"),
                        title: Some(format!("Bolum {number}")),
                        chapter_number: Some(chapter_number),
                        url: Some(format!("{BASE_URL}/manga/{key}/{number}")),
                        ..MangaChapter::default()
                    });
                }
            }
            let total_pages = data.get("pages").and_then(Value::as_u64).unwrap_or(1);
            if page >= total_pages {
                break;
            }
            page += 1;
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".to_string());
        let body = fetch_json_or_fixture(&format!("{BASE_URL}/api/manga/{key}"), PAGES_FIXTURE);
        Ok(parse_pages_json(&body, &key))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_json_or_fixture(&format!("{BASE_URL}/api/manga/{key}"), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details_json(&body, Some(key))),
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
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_manga_list(body: &str, path: &[&str], has_next_page: bool) -> Paged<CatalogItem> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Paged::default();
    };
    let mut node = &root;
    for segment in path {
        node = node.get(segment).unwrap_or(&Value::Null);
    }
    let entries = node
        .as_array()
        .into_iter()
        .flatten()
        .map(catalog_from_manga)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_search(body: &str, page: u64) -> Paged<CatalogItem> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Paged::default();
    };
    let data = root.get("data").unwrap_or(&Value::Null);
    let entries = data
        .get("hits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(catalog_from_manga)
        .fold(Vec::new(), push_unique);
    let total = data
        .get("estimatedTotalHits")
        .and_then(Value::as_u64)
        .unwrap_or(entries.len() as u64);
    let limit = data.get("limit").and_then(Value::as_u64).unwrap_or(42);
    Paged {
        entries,
        has_next_page: page.saturating_mul(limit) < total,
    }
}

fn catalog_from_manga(value: &Value) -> CatalogItem {
    let key = value
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or("sample")
        .trim_matches('/')
        .to_string();
    CatalogItem {
        key: key.clone(),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Manga")
            .to_string(),
        cover: value
            .get("coverImage")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(format!("{BASE_URL}/manga/{key}")),
        language: Some("tr".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details_json(body: &str, key: Option<String>) -> CatalogItem {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return CatalogItem::default();
    };
    let data = root.get("data").unwrap_or(&root);
    let key = key.unwrap_or_else(|| {
        data.get("slug")
            .and_then(Value::as_str)
            .unwrap_or("sample")
            .to_string()
    });
    CatalogItem {
        key: key.clone(),
        title: data
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Manga")
            .to_string(),
        cover: data
            .get("coverImage")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        description: data
            .get("description")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        tags: data
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| genre.get("name").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect(),
        status: match data.get("status").and_then(Value::as_str).unwrap_or_default() {
            "FINISHED" => ItemStatus::Completed,
            "ONGOING" => ItemStatus::Ongoing,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/manga/{key}")),
        language: Some("tr".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages_json(body: &str, chapter_key: &str) -> Vec<MangaPage> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let data = root.get("data").unwrap_or(&root);
    let chapter = data.get("chapter").unwrap_or(data);
    let static_info = chapter
        .get("static")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .unwrap_or(&Value::Null);
    let Some(file_size) = static_info.get("fileSize").and_then(Value::as_u64) else {
        return Vec::new();
    };
    let fansub_id = static_info
        .get("fansubId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut parts = chapter_key.split('/');
    let manga_slug = parts.next().unwrap_or("sample");
    let chapter_number = chapter
        .get("chapter")
        .and_then(value_f64)
        .map(trim_float)
        .or_else(|| parts.next().map(ToString::to_string))
        .unwrap_or_else(|| "1".to_string());
    (1..=file_size)
        .enumerate()
        .map(|(index, page)| MangaPage {
            content: PageContent::Url {
                url: format!("{CDN_URL}/{manga_slug}/{chapter_number}/{page}-{fansub_id}.jpeg"),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn filter_str<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters.get(id).and_then(Value::as_str)
}

fn filter_bool(filters: &Value, id: &str) -> bool {
    filters.get(id).and_then(Value::as_bool).unwrap_or(false)
}

fn normalize_key(value: &str) -> String {
    value
        .trim_start_matches(BASE_URL)
        .trim_start_matches("/manga/")
        .trim_matches('/')
        .to_string()
}

fn value_f32(value: Option<&Value>) -> Option<f32> {
    value.and_then(value_f64).map(|number| number as f32)
}

fn value_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|raw| raw.parse().ok()))
}

fn trim_float(number: f64) -> String {
    if (number.fract()).abs() < f64::EPSILON {
        format!("{}", number as u64)
    } else {
        number.to_string()
    }
}

fn push_unique(mut out: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !out.iter().any(|existing| existing.key == item.key) {
        out.push(item);
    }
    out
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"success":true,"data":{"mangas":[{"title":"Sample","slug":"sample","coverImage":"https://cdn.zukrein.com/sample.jpg"}],"pages":1}}"#;
const SEARCH_FIXTURE: &str = r#"{"success":true,"data":{"hits":[{"title":"Sample","slug":"sample","coverImage":"https://cdn.zukrein.com/sample.jpg"}],"estimatedTotalHits":1,"limit":42}}"#;
const DETAILS_FIXTURE: &str = r#"{"success":true,"data":{"slug":"sample","title":"Sample","status":"ONGOING","description":"Desc","coverImage":"https://cdn.zukrein.com/sample.jpg","genres":[{"name":"Action"}]}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"success":true,"data":{"chapters":[{"chapter":1}],"pages":1}}"#;
const PAGES_FIXTURE: &str = r#"{"success":true,"data":{"chapter":{"chapter":1,"static":[{"id":"s","fansubId":"f","fileSize":1}]}}}"#;
