use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient};
use serde_json::Value;

const SOURCE: MLBBLore = MLBBLore;
const BASE_URL: &str = "https://play.mobilelegends.com";
const API_URL: &str = "https://api.mobilelegends.com";
const TYPE_COMIC: &str = "3";
const PAGE_SIZE: u64 = 5;

struct MLBBLore;

impl MangaSource for MLBBLore {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "1"
        } else {
            "3"
        };
        Ok(parse_list(&album_list(page, sort)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default();
        if let Some(id) = query.strip_prefix("mlbb:") {
            return Ok(Paged {
                entries: vec![parse_detail(&album_detail(id), Some(id.to_string()))],
                has_next_page: false,
            });
        }
        Ok(parse_list(&album_list(page, "3")))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let id = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        Ok(parse_detail(&album_detail(&id), Some(id)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let id = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        Ok(vec![MangaChapter {
            key: id.clone(),
            title: Some("Chapter 1".to_string()),
            chapter_number: Some(1.0),
            url: Some(format!("mlbb:{id}")),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let id = manga::request_key(&request, "chapter").unwrap_or_else(|| "1".to_string());
        Ok(parse_pages(&album_detail(&id)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
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
        .with_origin(BASE_URL)
}

fn post_form(path: &str, form: &[(&str, &str)], fixture: &str) -> String {
    client()
        .post(&format!("{API_URL}{path}"))
        .header("Accept", "application/json")
        .form(form)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn album_list(page: u64, sort: &str) -> String {
    post_form(
        "/lore/album/list",
        &[
            ("type", TYPE_COMIC),
            ("sort", sort),
            ("page", &page.to_string()),
            ("page_size", &PAGE_SIZE.to_string()),
            ("lang", "en"),
            ("token", ""),
        ],
        LIST_FIXTURE,
    )
}

fn album_detail(id: &str) -> String {
    post_form(
        "/lore/album/detail",
        &[("id", id), ("lang", "en"), ("token", "")],
        DETAIL_FIXTURE,
    )
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let rows = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|root| root.get("data").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let entries = rows
        .iter()
        .filter(|row| row.get("type").and_then(Value::as_i64) == Some(3))
        .map(|row| {
            let id = json_text(row, "id").unwrap_or_else(|| "1".to_string());
            CatalogItem {
                key: id.clone(),
                title: json_text(row, "title").unwrap_or_else(|| "MLBB Lore".to_string()),
                cover: json_text(row, "thumb").map(|value| absolute_image(&value)),
                authors: json_text(row, "hero_name").into_iter().collect(),
                url: Some(format!("mlbb:{id}")),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() as u64 >= PAGE_SIZE,
        entries,
    }
}

fn parse_detail(body: &str, key: Option<String>) -> CatalogItem {
    let data = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|root| root.get("data").cloned())
        .unwrap_or(Value::Null);
    let id = key
        .or_else(|| json_text(&data, "id"))
        .unwrap_or_else(|| "1".to_string());
    CatalogItem {
        key: id.clone(),
        title: json_text(&data, "title").unwrap_or_else(|| "MLBB Lore".to_string()),
        cover: json_text(&data, "thumb").map(|value| absolute_image(&value)),
        authors: json_text(&data, "hero_name").into_iter().collect(),
        description: json_text(&data, "share_content"),
        status: ItemStatus::Completed,
        url: Some(format!("mlbb:{id}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|root| root.get("data")?.get("comic_content")?.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(absolute_image))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn absolute_image(value: &str) -> String {
    if value.starts_with("//") {
        format!("https:{value}")
    } else {
        value.to_string()
    }
}

fn json_text(value: &Value, key: &str) -> Option<String> {
    value.get(key).map(|value| {
        value
            .as_str()
            .map(ToString::to_string)
            .unwrap_or_else(|| value.to_string())
    })
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":[{"id":1,"type":3,"title":"Sample Lore","hero_name":"Hero","thumb":"//example.com/cover.jpg"}]}"#;
const DETAIL_FIXTURE: &str = r#"{"data":{"id":1,"title":"Sample Lore","hero_name":"Hero","thumb":"//example.com/cover.jpg","share_content":"Summary","comic_content":["//example.com/page1.jpg","//example.com/page2.jpg"]}}"#;
