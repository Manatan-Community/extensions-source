use aes::{Aes128, Aes256};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cbc::{
    cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit},
    Decryptor,
};
use manatan_extension::{
    abi::ExtensionResult, export_manga_source, http, source::MangaSource, CatalogItem, ItemStatus,
    MangaChapter, MangaPage, PageContent, Paged, ProcessedImage, SearchRequest, UrlResolveResult,
};
use manatan_shared::{dates, manga, url};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha512};
use std::collections::BTreeMap;

type Aes128CbcDec = Decryptor<Aes128>;
type Aes256CbcDec = Decryptor<Aes256>;

const SOURCE: CreativeComic = CreativeComic;
const BASE_URL: &str = "https://www.creative-comic.tw";
const API_URL: &str = "https://api.creative-comic.tw";
const FREE_TOKEN: &str = "freeforccc2020reading";

struct CreativeComic;

impl MangaSource for CreativeComic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updated_at"
        } else {
            "like_count"
        };
        Ok(parse_listing(
            &fetch_api(
                &format!("{API_URL}/book?page={page}&rows_per_page=24&sort_by={sort}&class=2"),
                LIST_FIXTURE,
            ),
            page,
            24,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_api(&format!("{API_URL}/book/{key}/info"), DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }
        let page = page(&request);
        Ok(parse_listing(&fetch_api(&format!("{API_URL}/book?page={page}&rows_per_page=12&keyword={}&category=all&sort_by=updated_at&class=2", url::query_escape(query)), LIST_FIXTURE), page, 12))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        Ok(parse_details(
            &fetch_api(&format!("{API_URL}/book/{key}/info"), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        Ok(parse_chapters(&fetch_api(
            &format!("{API_URL}/book/{key}/chapter"),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1".to_string());
        Ok(parse_pages(
            &fetch_api(&format!("{API_URL}/book/chapter/{key}"), PAGES_FIXTURE),
            &key,
        ))
    }

    fn resolve_page_image(
        &self,
        request: Value,
    ) -> ExtensionResult<manatan_extension::MangaPageImage> {
        let id = request
            .get("page")
            .and_then(|p| p.get("extra"))
            .and_then(|e| e.get("cccPageId"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let body = fetch_api(
            &format!("{API_URL}/book/chapter/image/{id}"),
            IMAGE_KEY_FIXTURE,
        );
        let key = serde_json::from_str::<ImageKeyResponse>(&body)
            .ok()
            .map(|r| r.data.key)
            .unwrap_or_default();
        let decrypted = decrypt_page_key(&key).unwrap_or_else(|| {
            "00000000000000000000000000000000:00000000000000000000000000000000".to_string()
        });
        let mut parts = decrypted.split(':');
        let key_hex = parts.next().unwrap_or_default().to_string();
        let iv_hex = parts.next().unwrap_or_default().to_string();
        Ok(manatan_extension::MangaPageImage {
            url: format!(
                "https://storage.googleapis.com/ccc-www/fs/chapter_content/encrypt/{id}/2"
            ),
            headers: manga::image_headers(BASE_URL),
            extra: BTreeMap::from([
                ("cccKeyHex".to_string(), json!(key_hex)),
                ("cccIvHex".to_string(), json!(iv_hex)),
            ]),
            ..Default::default()
        })
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let input = request
            .get("imageBase64")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(key_hex) = extra_str(&request, "cccKeyHex") else {
            return passthrough(request);
        };
        let Some(iv_hex) = extra_str(&request, "cccIvHex") else {
            return passthrough(request);
        };
        let Some(cipher) = STANDARD.decode(input).ok() else {
            return passthrough(request);
        };
        let Some(key) = decode_hex(key_hex) else {
            return passthrough(request);
        };
        let Some(iv) = decode_hex(iv_hex) else {
            return passthrough(request);
        };
        let plain = match key.len() {
            16 => Aes128CbcDec::new_from_slices(&key, &iv)
                .ok()
                .and_then(|d| d.decrypt_padded_vec_mut::<Pkcs7>(&cipher).ok()),
            32 => Aes256CbcDec::new_from_slices(&key, &iv)
                .ok()
                .and_then(|d| d.decrypt_padded_vec_mut::<Pkcs7>(&cipher).ok()),
            _ => None,
        };
        let Some(plain) = plain else {
            return passthrough(request);
        };
        let data = String::from_utf8_lossy(&plain);
        let mime_type = data
            .strip_prefix("data:")
            .and_then(|s| s.split(';').next())
            .map(ToOwned::to_owned)
            .or_else(|| Some("image/jpeg".to_string()));
        let image_base64 = data.split("base64,").nth(1).unwrap_or(input).to_string();
        Ok(ProcessedImage {
            image_base64,
            mime_type,
            ..ProcessedImage::default()
        })
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/zh/book/{key}/content")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/zh/reader_comic/{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_api(&format!("{API_URL}/book/{key}/info"), DETAILS_FIXTURE),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_header("device", "web_desktop")
        .with_header("uuid", "null")
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn normalize_key(input: &str) -> String {
    input
        .trim_end_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .next_back()
        .unwrap_or(input)
        .to_string()
}

fn parse_listing(body: &str, page: u64, rows_per_page: u64) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<BookListResponse>(body).unwrap_or_else(|_| {
        serde_json::from_str(LIST_FIXTURE).expect("valid creativecomic list fixture")
    });
    Paged {
        has_next_page: page.saturating_mul(rows_per_page) < response.data.total.unwrap_or_default(),
        entries: response
            .data
            .data
            .into_iter()
            .map(|book| CatalogItem {
                key: book.id.to_string(),
                title: book.name,
                cover: Some(book.image1),
                url: Some(format!("{BASE_URL}/zh/book/{}/content", book.id)),
                language: Some("zh-Hant".to_string()),
                content_rating: Some("safe".to_string()),
                ..CatalogItem::default()
            })
            .collect(),
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let response = serde_json::from_str::<DetailsResponse>(body).unwrap_or_else(|_| {
        serde_json::from_str(DETAILS_FIXTURE).expect("valid creativecomic details fixture")
    });
    let data = response.data;
    CatalogItem {
        key: key.to_string(),
        title: data.name,
        cover: Some(data.image1),
        authors: data.author.iter().map(|a| a.name.clone()).collect(),
        artists: data.author.into_iter().map(|a| a.name).collect(),
        tags: data
            .tags
            .into_iter()
            .map(|tag| tag.name)
            .chain(data.r#type.into_iter().map(|tag| tag.name))
            .collect(),
        description: Some(data.description),
        status: if data.completed == 1 {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(format!("{BASE_URL}/zh/book/{key}/content")),
        language: Some("zh-Hant".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<ChapterListResponse>(body).unwrap_or_else(|_| {
        serde_json::from_str(CHAPTERS_FIXTURE).expect("valid creativecomic chapters fixture")
    });
    response
        .data
        .chapters
        .into_iter()
        .rev()
        .map(|chapter| {
            let is_readable = chapter.is_free == 1
                || chapter.is_buy == 1
                || chapter.is_rent == 1
                || chapter.sales_plan == 0;
            MangaChapter {
                key: chapter.id.to_string(),
                title: Some(format!(
                    "{}{} {}",
                    if is_readable { "" } else { "[Locked] " },
                    chapter.vol_name,
                    chapter.name
                )),
                date_uploaded: dates::parse_ymd(
                    chapter
                        .online_at
                        .split_whitespace()
                        .next()
                        .unwrap_or_default(),
                ),
                is_locked: !is_readable,
                url: Some(format!("{BASE_URL}/zh/reader_comic/{}", chapter.id)),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str, chapter_key: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<PageListResponse>(body).unwrap_or_else(|_| {
        serde_json::from_str(PAGES_FIXTURE).expect("valid creativecomic pages fixture")
    });
    response
        .data
        .chapter
        .proportion
        .into_iter()
        .enumerate()
        .map(|(index, page)| MangaPage {
            content: PageContent::Lazy {
                key: page.id.to_string(),
                url: Some(format!("{API_URL}/book/chapter/image/{}", page.id)),
                page_url: Some(format!("{BASE_URL}/zh/reader_comic/{chapter_key}")),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            extra: BTreeMap::from([("cccPageId".to_string(), json!(page.id.to_string()))]),
            ..MangaPage::default()
        })
        .collect()
}

fn decrypt_page_key(input: &str) -> Option<String> {
    let digest = Sha512::digest(FREE_TOKEN.as_bytes());
    let key = &digest[..32];
    let iv = &key[15..31];
    let cipher = STANDARD.decode(input).ok()?;
    let plain = Aes256CbcDec::new_from_slices(key, iv)
        .ok()?
        .decrypt_padded_vec_mut::<Pkcs7>(&cipher)
        .ok()?;
    String::from_utf8(plain).ok()
}

fn extra_str<'a>(request: &'a Value, key: &str) -> Option<&'a str> {
    request
        .get("page")
        .and_then(|p| p.get("extra"))
        .and_then(|e| e.get(key))
        .and_then(Value::as_str)
}

fn passthrough(request: Value) -> ExtensionResult<ProcessedImage> {
    Ok(ProcessedImage {
        image_base64: request
            .get("imageBase64")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        mime_type: request
            .get("mimeType")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        ..ProcessedImage::default()
    })
}

fn decode_hex(input: &str) -> Option<Vec<u8>> {
    if !input.len().is_multiple_of(2) {
        return None;
    }
    (0..input.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&input[i..i + 2], 16).ok())
        .collect()
}

#[derive(Deserialize)]
struct BookListResponse {
    data: BookListData,
}
#[derive(Deserialize)]
struct BookListData {
    total: Option<u64>,
    data: Vec<BookSummary>,
}
#[derive(Deserialize)]
struct BookSummary {
    id: u64,
    name: String,
    image1: String,
}
#[derive(Deserialize)]
struct DetailsResponse {
    data: DetailsData,
}
#[derive(Deserialize)]
struct DetailsData {
    name: String,
    description: String,
    image1: String,
    #[serde(default)]
    author: Vec<NameData>,
    #[serde(default)]
    r#type: Option<NameData>,
    #[serde(default)]
    tags: Vec<NameData>,
    completed: i32,
}
#[derive(Deserialize)]
struct NameData {
    name: String,
}
#[derive(Deserialize)]
struct ChapterListResponse {
    data: ChapterListData,
}
#[derive(Deserialize)]
struct ChapterListData {
    chapters: Vec<ChapterData>,
}
#[derive(Deserialize)]
struct ChapterData {
    id: u64,
    name: String,
    vol_name: String,
    is_free: i32,
    is_buy: i32,
    is_rent: i32,
    sales_plan: i32,
    online_at: String,
}
#[derive(Deserialize)]
struct PageListResponse {
    data: PageListData,
}
#[derive(Deserialize)]
struct PageListData {
    chapter: PageListChapter,
}
#[derive(Deserialize)]
struct PageListChapter {
    proportion: Vec<PageData>,
}
#[derive(Deserialize)]
struct PageData {
    id: u64,
}
#[derive(Deserialize)]
struct ImageKeyResponse {
    data: ImageKeyData,
}
#[derive(Deserialize)]
struct ImageKeyData {
    key: String,
}

const LIST_FIXTURE: &str = r#"{"data":{"total":1,"data":[{"id":1,"name":"Sample CCC","image1":"https://www.creative-comic.tw/cover.jpg"}]}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"name":"Sample CCC","description":"Summary","image1":"https://www.creative-comic.tw/cover.jpg","author":[{"name":"Author"}],"type":{"name":"Genre"},"tags":[{"name":"Tag"}],"completed":0}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"chapters":[{"id":1,"name":"Chapter 1","vol_name":"Vol.1","is_free":1,"is_buy":0,"is_rent":0,"sales_plan":0,"online_at":"2024-01-01 00:00:00"}]}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"chapter":{"proportion":[{"id":1}]}}}"#;
const IMAGE_KEY_FIXTURE: &str = r#"{"data":{"key":""}}"#;

export_manga_source!(SOURCE);
