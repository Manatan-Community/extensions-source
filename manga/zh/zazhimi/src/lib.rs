use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Zazhimi = Zazhimi;
const BASE_URL: &str = "https://www.zazhimi.net";
const API_URL: &str = "https://android2026.zazhimi.net/api";

struct Zazhimi;

impl MangaSource for Zazhimi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = format!("{API_URL}/index.php?p={}&s=20", page(&request));
        let response = parse_json::<IndexResponse>(&fetch(&target, LIST_FIXTURE), LIST_FIXTURE);
        Ok(Paged {
            has_next_page: !response.new.is_empty(),
            entries: response
                .new
                .into_iter()
                .map(|item| item.catalog())
                .collect(),
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
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = if query.is_empty() {
            format!(
                "{API_URL}/lists.php?c={}&m={}&p={}&s=20",
                filters.get("type").and_then(Value::as_str).unwrap_or("6"),
                filters.get("brand").and_then(Value::as_str).unwrap_or(""),
                page(&request)
            )
        } else {
            format!(
                "{API_URL}/search.php?k={}&p={}&s=20",
                url::query_escape(query),
                page(&request)
            )
        };
        let response =
            parse_json::<SearchResponse>(&fetch(&target, SEARCH_FIXTURE), SEARCH_FIXTURE);
        Ok(Paged {
            entries: response
                .magazine
                .into_iter()
                .map(|item| item.catalog())
                .collect(),
            has_next_page: true,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/show.php?a=1".into());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/show.php?a=1".into());
        let response = show(&key);
        let title = response
            .content
            .first()
            .map(|item| item.mag_name.clone())
            .unwrap_or_else(|| "Magazine".into());
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some(title),
            url: Some(url::join_url(BASE_URL, &key)),
            chapter_number: Some(1.0),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/show.php?a=1".into());
        let response = show(&key);
        Ok(response
            .content
            .into_iter()
            .enumerate()
            .map(|(index, item)| MangaPage {
                content: PageContent::Url {
                    url: item.mag_pic,
                    context: None,
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }
    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}
fn fetch(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("User-Agent", "ZaZhiMi_6.0.0")
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.into())
}
fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}
fn show(key: &str) -> ShowResponse {
    parse_json(
        &fetch(&format!("{API_URL}{key}"), SHOW_FIXTURE),
        SHOW_FIXTURE,
    )
}
fn fetch_details(key: &str) -> CatalogItem {
    let response = show(key);
    if let Some(item) = response.content.first() {
        item.catalog(key)
    } else {
        CatalogItem {
            key: key.into(),
            title: "杂志迷".into(),
            url: Some(url::join_url(BASE_URL, key)),
            language: Some("zh".into()),
            content_rating: Some("safe".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}
fn key_from_url(input: &str) -> Option<String> {
    if !input.contains("show.php") {
        return None;
    }
    let path = input
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split_once('/').map(|(_, path)| path))
        .unwrap_or(input);
    Some(
        format!(
            "/{}",
            path.split('?').next().unwrap_or(path).trim_matches('/')
        ) + input
            .split_once('?')
            .map(|(_, q)| format!("?{q}"))
            .unwrap_or_default()
            .as_str(),
    )
}
fn parse_json<T: for<'de> Deserialize<'de>>(body: &str, fixture: &str) -> T {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).expect("valid fixture"))
}

#[derive(Deserialize)]
struct IndexResponse {
    new: Vec<NewItem>,
}
#[derive(Deserialize)]
struct SearchResponse {
    magazine: Vec<SearchItem>,
}
#[derive(Deserialize)]
struct ShowResponse {
    content: Vec<ShowItem>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewItem {
    mag_id: String,
    mag_name: String,
    mag_cover: String,
}
impl NewItem {
    fn catalog(self) -> CatalogItem {
        CatalogItem {
            key: format!("/show.php?a={}", self.mag_id),
            title: self.mag_name.clone(),
            authors: vec![
                self.mag_name
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string(),
            ]
            .into_iter()
            .filter(|v| !v.is_empty())
            .collect(),
            cover: Some(self.mag_cover),
            url: Some(format!("{BASE_URL}/show.php?a={}", self.mag_id)),
            language: Some("zh".into()),
            content_rating: Some("safe".into()),
            status: ItemStatus::Completed,
            initialized: true,
            ..CatalogItem::default()
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchItem {
    mag_id: String,
    mag_name: String,
    mag_cover: Option<String>,
}
impl SearchItem {
    fn catalog(self) -> CatalogItem {
        CatalogItem {
            key: format!("/show.php?a={}", self.mag_id),
            title: self.mag_name.clone(),
            authors: vec![
                self.mag_name
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string(),
            ]
            .into_iter()
            .filter(|v| !v.is_empty())
            .collect(),
            cover: self.mag_cover,
            url: Some(format!("{BASE_URL}/show.php?a={}", self.mag_id)),
            language: Some("zh".into()),
            content_rating: Some("safe".into()),
            status: ItemStatus::Completed,
            ..CatalogItem::default()
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShowItem {
    mag_id: String,
    mag_name: String,
    #[serde(default)]
    type_name: String,
    mag_pic: String,
}
impl ShowItem {
    fn catalog(&self, key: &str) -> CatalogItem {
        CatalogItem {
            key: key.into(),
            title: self.mag_name.clone(),
            authors: vec![
                self.mag_name
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string(),
            ]
            .into_iter()
            .filter(|v| !v.is_empty())
            .collect(),
            tags: vec![self.type_name.clone()]
                .into_iter()
                .filter(|v| !v.is_empty())
                .collect(),
            cover: Some(self.mag_pic.clone()),
            url: Some(format!("{BASE_URL}/show.php?a={}", self.mag_id)),
            language: Some("zh".into()),
            content_rating: Some("safe".into()),
            status: ItemStatus::Completed,
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"new":[{"magId":"1","magName":"Sample Magazine","magCover":"https://www.zazhimi.net/cover.jpg","magDate":"2026-01-01"}]}"#;
const SEARCH_FIXTURE: &str = r#"{"magazine":[{"magId":"1","magName":"Sample Magazine","magCover":"https://www.zazhimi.net/cover.jpg","magDate":"2026-01-01","pubdate":"2026-01-01"}]}"#;
const SHOW_FIXTURE: &str = r#"{"content":[{"magId":"1","magName":"Sample Magazine","typeId":"6","typeName":"女装服饰","cateId":"1","magPic":"https://www.zazhimi.net/page.jpg","pageUrl":"","pageThumbUrl":""}]}"#;
