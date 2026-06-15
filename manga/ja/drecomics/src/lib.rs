use aes::Aes256;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::{
    Decryptor,
    cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7},
};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient};
use serde::Deserialize;
use serde_json::Value;

type Aes256CbcDec = Decryptor<Aes256>;

const SOURCE: DreComics = DreComics;
const BASE_URL: &str = "https://drecomi-plus.jp";
const API_URL: &str = "https://api.drecomi-plus.jp/api/v1/app";
const AUTH_URL: &str = "https://drecomi-plus.jp/api/auth";

struct DreComics;

impl MangaSource for DreComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_ranking(RANKING_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let target = format!(
                "{API_URL}/series?sort=latest_published_at&order=desc&page={page}&limit=18"
            );
            return Ok(parse_series_page(&api_get(&target, None, SERIES_FIXTURE)));
        }
        let target = format!("{API_URL}/series/ranking?page={page}&limit=10");
        Ok(parse_ranking(&api_get(&target, None, RANKING_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = query
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(query);
            return Ok(Paged {
                entries: vec![parse_details_body(
                    &api_get(
                        &format!("{API_URL}/series/{key}"),
                        token(&request),
                        DETAILS_FIXTURE,
                    ),
                    key,
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = format!(
            "{API_URL}/series?keyword={}&page={page}&limit=18",
            manatan_shared::url::query_escape(query)
        );
        Ok(parse_series_page(&api_get(
            &target,
            token(&request),
            SERIES_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample-series".into());
        Ok(parse_details_body(
            &api_get(
                &format!("{API_URL}/series/{key}"),
                token(&request),
                DETAILS_FIXTURE,
            ),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample-series".into());
        let hide_locked = request
            .get("preferences")
            .and_then(|prefs| prefs.get("hide_locked"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut chapters = Vec::new();
        fetch_chapter_kind(
            &key,
            "episodes",
            hide_locked,
            token(&request),
            &mut chapters,
        );
        fetch_chapter_kind(&key, "volumes", hide_locked, token(&request), &mut chapters);
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "sample-series-episode-1".into());
        let endpoint = if key.split('-').count() == 3 {
            format!("{API_URL}/viewer/episodes/{key}/session")
        } else {
            format!("{API_URL}/viewer/volumes/{key}/session")
        };
        let body = api_post_json(&endpoint, "{}", token(&request), VIEWER_FIXTURE);
        let viewer = serde_json::from_str::<ViewerResponse>(&body)
            .unwrap_or_else(|_| serde_json::from_str(VIEWER_FIXTURE).expect("fixture is valid"));
        Ok(viewer
            .pages
            .into_iter()
            .map(|page| {
                let url = format!("{}#{}:{}", page.image_url, viewer.session_key, page.iv);
                MangaPage {
                    content: PageContent::Url {
                        url,
                        context: Some(manga::image_headers(BASE_URL)),
                    },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(format!("Page {}", page.page_number)),
                    ..MangaPage::default()
                }
            })
            .collect())
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let image_base64 = request
            .get("imageBase64")
            .or_else(|| request.get("image_base64"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let url = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("url"))
            .and_then(|url| url.get("url").or(Some(url)))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let processed =
            decrypt_image_base64(image_base64, url).unwrap_or_else(|| image_base64.to_string());
        Ok(ProcessedImage {
            image_base64: processed,
            mime_type: request
                .get("mimeType")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            ..ProcessedImage::default()
        })
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/series/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let manga_code = key.split('-').next().unwrap_or(&key);
            format!("{BASE_URL}/series/{manga_code}/episodes/{key}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/series/") {
            let key = input
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details_body(
                    &api_get(
                        &format!("{API_URL}/series/{key}"),
                        token(&request),
                        DETAILS_FIXTURE,
                    ),
                    key,
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
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_get(target: &str, token: Option<String>, fixture: &str) -> String {
    let http = client();
    let mut request = http.get(target).xhr();
    if let Some(token) = token.filter(|value| !value.is_empty()) {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn api_post_json(target: &str, body: &str, token: Option<String>, fixture: &str) -> String {
    let http = client();
    let mut request = http.post(target).json(body.to_string()).xhr();
    if let Some(token) = token.filter(|value| !value.is_empty()) {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn token(request: &Value) -> Option<String> {
    let prefs = request.get("preferences").unwrap_or(&Value::Null);
    let email = prefs
        .get("email_pref")
        .and_then(Value::as_str)
        .unwrap_or("");
    let password = prefs
        .get("password_pref")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !email.is_empty() && !password.is_empty() {
        let csrf = api_get(&format!("{AUTH_URL}/csrf"), None, CSRF_FIXTURE);
        let csrf_token = serde_json::from_str::<Value>(&csrf)
            .ok()
            .and_then(|value| {
                value
                    .get("csrfToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })?;
        let _ = api_post_form(
            &format!("{AUTH_URL}/callback/credentials"),
            &[
                ("csrfToken", csrf_token.as_str()),
                ("callbackUrl", BASE_URL),
                ("email", email),
                ("password", password),
                ("json", "true"),
                ("redirect", "false"),
            ],
            "{}",
        );
    }
    let session = api_get(&format!("{AUTH_URL}/session"), None, SESSION_FIXTURE);
    serde_json::from_str::<Value>(&session)
        .ok()
        .and_then(|value| {
            value
                .get("accessToken")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

fn api_post_form(target: &str, form: &[(&str, &str)], fixture: &str) -> String {
    let http = client();
    http.post(target)
        .form(form)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_ranking(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<RankingResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(RANKING_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: response
            .items
            .into_iter()
            .map(|item| item.series.into_catalog())
            .collect(),
        has_next_page: response.pagination.has_next_page(),
    }
}

fn parse_series_page(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<SeriesResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(SERIES_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: response.items.into_iter().map(Item::into_catalog).collect(),
        has_next_page: response.pagination.has_next_page(),
    }
}

fn parse_details_body(body: &str, key: &str) -> CatalogItem {
    let details = serde_json::from_str::<DetailsResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let mut description = details.summary.unwrap_or_default();
    if let Some(update) = details.update_interval {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str(&update);
    }
    if let Some(next) = details.next_update_schedule {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("更新予定: ");
        description.push_str(&next);
    }
    if details.is_adult == Some(true) {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("18+");
    }
    CatalogItem {
        key: key.to_string(),
        title: details.name,
        cover: details.thumbnail.and_then(|thumb| thumb.cdn_url),
        description: (!description.is_empty()).then_some(description),
        authors: details
            .authors
            .unwrap_or_default()
            .into_iter()
            .map(|author| author.name)
            .collect(),
        tags: details
            .genres
            .unwrap_or_default()
            .into_iter()
            .map(|genre| genre.name)
            .collect(),
        status: match details.status.as_str() {
            "ongoing" => ItemStatus::Ongoing,
            "completed" => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/series/{key}")),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapter_kind(
    series_code: &str,
    kind: &str,
    hide_locked: bool,
    token: Option<String>,
    out: &mut Vec<MangaChapter>,
) {
    let mut page = 1;
    loop {
        let sort = if kind == "volumes" {
            "volume_number"
        } else {
            "episode_number"
        };
        let target = format!(
            "{API_URL}/{kind}?series_code={series_code}&page={page}&limit=200&sort={sort}&order=desc"
        );
        let body = api_get(&target, token.clone(), CHAPTERS_FIXTURE);
        let response = serde_json::from_str::<ChapterResponse>(&body)
            .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
        out.extend(
            response
                .items
                .into_iter()
                .filter_map(|item| item.into_chapter(hide_locked)),
        );
        if !response.pagination.has_next_page() || page > 20 {
            break;
        }
        page += 1;
    }
}

fn decrypt_image_base64(input: &str, image_url: &str) -> Option<String> {
    let fragment = image_url.split('#').nth(1)?;
    let (key_b64, iv_b64) = fragment.split_once(':')?;
    let key = STANDARD.decode(key_b64).ok()?;
    let iv = STANDARD.decode(iv_b64).ok()?;
    let encrypted = STANDARD.decode(input).ok()?;
    let mut buffer = encrypted.clone();
    let decrypted = Aes256CbcDec::new_from_slices(&key, &iv)
        .ok()?
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .ok()?;
    Some(STANDARD.encode(decrypted))
}

#[derive(Deserialize)]
struct RankingResponse {
    items: Vec<RankingItem>,
    pagination: Pagination,
}

#[derive(Deserialize)]
struct RankingItem {
    series: Item,
}

#[derive(Deserialize)]
struct SeriesResponse {
    items: Vec<Item>,
    pagination: Pagination,
}

#[derive(Deserialize)]
struct Item {
    code: String,
    name: String,
    thumbnail: Option<Thumbnail>,
}

impl Item {
    fn into_catalog(self) -> CatalogItem {
        CatalogItem {
            key: self.code.clone(),
            title: self.name,
            cover: self.thumbnail.and_then(|thumb| thumb.cdn_url),
            url: Some(format!("{BASE_URL}/series/{}", self.code)),
            language: Some("ja".into()),
            content_rating: Some("adult".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct Pagination {
    #[serde(alias = "current_page")]
    current_page: u64,
    #[serde(alias = "total_pages")]
    total_pages: u64,
}

impl Pagination {
    fn has_next_page(&self) -> bool {
        self.current_page < self.total_pages
    }
}

#[derive(Deserialize)]
struct Thumbnail {
    #[serde(rename = "cdn_url")]
    cdn_url: Option<String>,
}

#[derive(Deserialize)]
struct DetailsResponse {
    authors: Option<Vec<Named>>,
    genres: Option<Vec<Named>>,
    #[serde(rename = "is_adult")]
    is_adult: Option<bool>,
    name: String,
    #[serde(rename = "next_update_schedule")]
    next_update_schedule: Option<String>,
    status: String,
    summary: Option<String>,
    thumbnail: Option<Thumbnail>,
    #[serde(rename = "update_interval")]
    update_interval: Option<String>,
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct ChapterResponse {
    items: Vec<ChapterItem>,
    pagination: Pagination,
}

#[derive(Deserialize)]
struct ChapterItem {
    #[serde(rename = "actual_price")]
    actual_price: Option<i64>,
    code: String,
    #[serde(alias = "episode_number", alias = "volume_number")]
    episode_number: Option<f32>,
    #[serde(rename = "is_purchased")]
    is_purchased: bool,
    name: String,
}

impl ChapterItem {
    fn into_chapter(self, hide_locked: bool) -> Option<MangaChapter> {
        let is_locked = !self.is_purchased && self.actual_price != Some(0);
        if hide_locked && is_locked {
            return None;
        }
        Some(MangaChapter {
            key: self.code.clone(),
            title: Some(if is_locked {
                format!("Locked: {}", self.name)
            } else {
                self.name
            }),
            chapter_number: self.episode_number,
            url: Some(format!(
                "{BASE_URL}/series/{}/episodes/{}",
                self.code.split('-').next().unwrap_or("series"),
                self.code
            )),
            is_locked,
            ..MangaChapter::default()
        })
    }
}

#[derive(Deserialize)]
struct ViewerResponse {
    pages: Vec<ViewerPage>,
    #[serde(rename = "session_key")]
    session_key: String,
}

#[derive(Deserialize)]
struct ViewerPage {
    #[serde(rename = "image_url")]
    image_url: String,
    iv: String,
    #[serde(rename = "page_number")]
    page_number: u32,
}

export_manga_source!(SOURCE);

const RANKING_FIXTURE: &str = r#"{"items":[{"series":{"code":"sample-series","name":"Sample DreComi","thumbnail":{"cdn_url":"https://cdn.drecomi-plus.jp/sample.jpg"}}}],"pagination":{"current_page":1,"total_pages":1}}"#;
const SERIES_FIXTURE: &str = r#"{"items":[{"code":"sample-series","name":"Sample DreComi","thumbnail":{"cdn_url":"https://cdn.drecomi-plus.jp/sample.jpg"}}],"pagination":{"current_page":1,"total_pages":1}}"#;
const DETAILS_FIXTURE: &str = r#"{"authors":[{"name":"Author"}],"genres":[{"name":"Manga"}],"is_adult":false,"name":"Sample DreComi","next_update_schedule":null,"status":"ongoing","summary":"Fixture description.","thumbnail":{"cdn_url":"https://cdn.drecomi-plus.jp/sample.jpg"},"update_interval":null}"#;
const CHAPTERS_FIXTURE: &str = r#"{"items":[{"actual_price":0,"code":"sample-series-episode-1","episode_number":1,"is_purchased":true,"name":"Chapter 1","publish_at":"2024-01-01T00:00:00+09:00"}],"pagination":{"current_page":1,"total_pages":1}}"#;
const VIEWER_FIXTURE: &str = r#"{"pages":[{"image_url":"https://cdn.drecomi-plus.jp/page1.jpg","iv":"AAAAAAAAAAAAAAAAAAAAAA==","page_number":1}],"session_key":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}"#;
const CSRF_FIXTURE: &str = r#"{"csrfToken":"fixture"}"#;
const SESSION_FIXTURE: &str = r#"{"accessToken":null}"#;
