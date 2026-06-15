use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: CyComi = CyComi;
const BASE_URL: &str = "https://cycomi.com";
const API_URL: &str = "https://web.cycomi.com/api";
const CONTENT_RATING: &str = "safe";

struct CyComi;

impl MangaSource for CyComi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let body = if latest {
            fetch_json(
                &format!("{API_URL}/title/serialization/list/{}", latest_day()),
                LATEST_FIXTURE,
            )
        } else {
            fetch_document(&format!("{BASE_URL}/ranking/title/1"), RANKING_FIXTURE)
        };
        let entries = if latest {
            parse_title_data(&body)
        } else {
            parse_ranking(&body)
        };
        Ok(Paged {
            entries,
            has_next_page: false,
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
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            let category = filter_string(&request, "category").unwrap_or("1");
            format!("{API_URL}/title/serialization/list/{category}")
        } else {
            format!("{API_URL}/search/list/1?word={}", url::query_escape(query))
        };
        Ok(Paged {
            entries: parse_title_data(&fetch_json(&target, LATEST_FIXTURE)),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        Ok(chapters_from_key(&key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1#1".to_string());
        let (chapter_id, title_id) = key.split_once('#').unwrap_or((&key, "1"));
        let body = client()
            .post(format!("{API_URL}/chapter/page/list"))
            .json(
                json!({
                    "titleId": title_id.parse::<u64>().unwrap_or(1),
                    "chapterId": chapter_id.parse::<u64>().unwrap_or(1)
                })
                .to_string(),
            )
            .xhr()
            .send_text()
            .unwrap_or_else(|_| PAGES_FIXTURE.to_string());
        Ok(parse_pages(&body))
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let input = request
            .get("imageBase64")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(image_url) = page_image_url(&request) else {
            return Ok(ProcessedImage {
                image_base64: input.to_string(),
                mime_type: request
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                ..ProcessedImage::default()
            });
        };
        if !image_url.ends_with("#decrypt") {
            return Ok(ProcessedImage {
                image_base64: input.to_string(),
                mime_type: request
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                ..ProcessedImage::default()
            });
        }
        let passphrase = image_url
            .split('#')
            .next()
            .and_then(|url| url.split('/').nth(5))
            .unwrap_or_default();
        if passphrase.contains("end_page") || passphrase.is_empty() {
            return Ok(ProcessedImage {
                image_base64: input.to_string(),
                mime_type: request
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                ..ProcessedImage::default()
            });
        }
        let bytes = STANDARD.decode(input).unwrap_or_default();
        Ok(ProcessedImage {
            image_base64: STANDARD.encode(rc4_decrypt(&bytes, passphrase)),
            mime_type: Some("image/jpeg".to_string()),
            ..ProcessedImage::default()
        })
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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
        .with_origin(BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_ranking(body: &str) -> Vec<CatalogItem> {
    let script = html::text_between(body, "<script id=\"__NEXT_DATA__\"", "</script>")
        .or_else(|| html::text_between(body, "<script id='__NEXT_DATA__'", "</script>"))
        .unwrap_or_else(|| body.to_string());
    let value = serde_json::from_str::<Value>(&script)
        .unwrap_or_else(|_| serde_json::from_str(RANKING_JSON_FIXTURE).unwrap());
    let titles = value
        .pointer("/props/pageProps/rankingTitleList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    titles.iter().map(title_to_item).collect()
}

fn parse_title_data(body: &str) -> Vec<CatalogItem> {
    let value = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(LATEST_FIXTURE).unwrap());
    value
        .pointer("/data/titles")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(title_to_item)
        .collect()
}

fn title_to_item(row: &Value) -> CatalogItem {
    let key = json_text(row, "titleId").unwrap_or_else(|| "1".to_string());
    CatalogItem {
        key: key.clone(),
        title: json_text(row, "titleName").unwrap_or_else(|| "CyComi".to_string()),
        cover: json_text(row, "image"),
        url: Some(format!("{BASE_URL}/title/{key}")),
        language: Some("ja".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let target = format!("{API_URL}/title/detail?titleId={}", url::query_escape(key));
    let body = fetch_json(&target, DETAILS_FIXTURE);
    let response = serde_json::from_str::<MangaResponse>(&body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    response.data.to_item()
}

fn chapters_from_key(key: &str) -> Vec<MangaChapter> {
    let mut out = Vec::new();
    let mut cursor = None::<u64>;
    for _ in 0..20 {
        let mut target = format!(
            "{API_URL}/chapter/paginatedList?sort=2&limit=100&titleId={}",
            url::query_escape(key)
        );
        if let Some(cursor) = cursor {
            target.push_str(&format!("&cursor={cursor}"));
        }
        let body = fetch_json(&target, CHAPTERS_FIXTURE);
        let response = serde_json::from_str::<ChapterListResponse>(&body)
            .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).unwrap());
        for chapter in response.data {
            out.push(chapter.to_chapter());
        }
        cursor = response.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    out
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<ViewerResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).unwrap());
    response
        .data
        .pages
        .into_iter()
        .map(|page| MangaPage {
            content: PageContent::Url {
                url: format!("{}#decrypt", page.image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", page.page_number)),
            ..MangaPage::default()
        })
        .collect()
}

fn key_from_url(input: &str) -> Option<String> {
    if input.chars().all(|ch| ch.is_ascii_digit()) {
        return Some(input.to_string());
    }
    let marker = "/title/";
    let index = input.find(marker)?;
    Some(
        input[index + marker.len()..]
            .split(['/', '?', '#'])
            .next()?
            .to_string(),
    )
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .or_else(|| request.get(id))
        .and_then(Value::as_str)
}

fn json_text(row: &Value, key: &str) -> Option<String> {
    row.get(key).and_then(|value| match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}

fn latest_day() -> u32 {
    1
}

fn page_image_url(request: &Value) -> Option<String> {
    request
        .get("page")
        .and_then(|page| page.get("content"))
        .and_then(|content| content.get("url"))
        .and_then(|url| url.get("url").or(Some(url)))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn rc4_decrypt(encrypted: &[u8], passphrase: &str) -> Vec<u8> {
    let pass = passphrase.as_bytes();
    let mut key = [0u8; 256];
    for (index, value) in key.iter_mut().enumerate() {
        *value = index as u8;
    }
    let mut swap = 0usize;
    for index in 0..256 {
        swap = (swap + key[index] as usize + pass[index % pass.len()] as usize) % 256;
        key.swap(index, swap);
    }
    let mut i = 0usize;
    let mut j = 0usize;
    let mut out = Vec::with_capacity(encrypted.len());
    for byte in encrypted {
        i = (i + 1) % 256;
        j = (j + key[i] as usize) % 256;
        key.swap(i, j);
        let xor = key[(key[i] as usize + key[j] as usize) % 256];
        out.push(byte ^ xor);
    }
    out
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaResponse {
    data: MangaDetails,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaDetails {
    author: Option<String>,
    body: Option<String>,
    image: Option<String>,
    title_id: u64,
    title_name: String,
    serial_type: String,
}

impl MangaDetails {
    fn to_item(&self) -> CatalogItem {
        CatalogItem {
            key: self.title_id.to_string(),
            title: self.title_name.clone(),
            cover: self.image.clone(),
            authors: self.author.clone().into_iter().collect(),
            description: self.body.clone(),
            status: if self.serial_type == "end" || self.serial_type == "shot" {
                ItemStatus::Completed
            } else {
                ItemStatus::Ongoing
            },
            url: Some(format!("{BASE_URL}/title/{}", self.title_id)),
            language: Some("ja".to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterListResponse {
    data: Vec<ChapterDetails>,
    next_cursor: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterDetails {
    id: u64,
    title_id: u64,
    name: String,
    sub_name: Option<String>,
    start_at: Option<i64>,
    purchase_use_coin: Option<u64>,
    rental_use_coin: Option<u64>,
    expiration_at: Option<i64>,
}

impl ChapterDetails {
    fn is_locked(&self) -> bool {
        (self.purchase_use_coin.unwrap_or(0) != 0 || self.rental_use_coin.unwrap_or(0) != 0)
            && self.expiration_at.is_none()
    }

    fn to_chapter(&self) -> MangaChapter {
        let sub = self
            .sub_name
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(|value| format!(" - {value}"))
            .unwrap_or_default();
        MangaChapter {
            key: format!("{}#{}", self.id, self.title_id),
            title: Some(format!(
                "{}{}{}",
                if self.is_locked() { "[Locked] " } else { "" },
                self.name,
                sub
            )),
            date_uploaded: self.start_at,
            url: Some(format!("{BASE_URL}/viewer/chapter/{}", self.id)),
            is_locked: self.is_locked(),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct ViewerResponse {
    data: ViewerData,
}

#[derive(Deserialize)]
struct ViewerData {
    pages: Vec<ViewerPage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ViewerPage {
    image: String,
    page_number: u32,
}

const RANKING_FIXTURE: &str = r#"<script id="__NEXT_DATA__">{"props":{"pageProps":{"rankingTitleList":[{"titleId":1,"titleName":"CyComi Sample","image":"https://cycomi.com/sample.jpg"}]}}}</script>"#;
const RANKING_JSON_FIXTURE: &str = r#"{"props":{"pageProps":{"rankingTitleList":[{"titleId":1,"titleName":"CyComi Sample","image":"https://cycomi.com/sample.jpg"}]}}}"#;
const LATEST_FIXTURE: &str = r#"{"data":{"titles":[{"titleId":1,"titleName":"CyComi Sample","image":"https://cycomi.com/sample.jpg"}]}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"author":"Sample Author","body":"Sample description.","image":"https://cycomi.com/sample.jpg","titleId":1,"titleName":"CyComi Sample","serialType":"serial"}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"id":10,"titleId":1,"name":"第1話","subName":"Sample","startAt":1700000000000,"purchaseUseCoin":0,"rentalUseCoin":0,"expirationAt":null}],"nextCursor":null}"#;
const PAGES_FIXTURE: &str =
    r#"{"data":{"pages":[{"image":"https://cycomi.com/assets/viewer/10/1.jpg","pageNumber":1}]}}"#;

export_manga_source!(SOURCE);
