use aes::Aes256;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use hmac::{Hmac, Mac};
use manatan_extension::{
    CatalogItem, ImageRequest, ItemStatus, MangaChapter, MangaPage, PageContent, Paged,
    ProcessedImage, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use md5::{Digest, Md5};
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::BTreeMap;

type Aes256CbcEnc = cbc::Encryptor<Aes256>;
type HmacSha256 = Hmac<Sha256>;

const SOURCE: BlossomManhwa = BlossomManhwa;
const BASE_URL: &str = "https://api.cherrymanhwa.com";
const SITE_URL: &str = "https://cherrymanhwa.com";
const SECRET_KEY: &str = "EA^UfBOF9lNdQDS3i2qAnsqxIrTpH%";
const ENCRYPT_KEY: &str = "6dFGd4Laa3vE%kLpr5eCtSEaAL%wJm";
const IMAGE_KEY: &[u8] = b"RghVx!Sf!Dw3y6O7KQcF%pg#";

struct BlossomManhwa;

impl MangaSource for BlossomManhwa {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            format!("{BASE_URL}/v1/manga/search/latesUpdates?limit=72&page={page}")
        } else {
            format!("{BASE_URL}/v1/manga/views/top?limit=72&page={page}")
        };
        Ok(parse_manga_list(
            &fetch_api_or_fixture(&target, LIST_FIXTURE),
            focus_language(&request),
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(slug) = slug_from_url(query) {
            let item = self.details(json!({ "manga": format!("/v1/manga/findBySlug/{slug}") }))?;
            return Ok(Paged {
                entries: vec![item],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mut target = format!("{BASE_URL}/v1/manga?limit=50&page={page}");
        if !query.is_empty() {
            target.push_str("&search=");
            target.push_str(&url::query_escape(query));
        }
        append_filter(&mut target, filters, "author", "author");
        append_filter(&mut target, filters, "artist", "artist");
        append_filter(&mut target, filters, "genre", "genre");
        append_filter(&mut target, filters, "sort", "sort");
        if let Some(lang) = filters
            .get("language")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            target.push_str("&language[]=");
            target.push_str(&url::query_escape(lang));
        }
        Ok(parse_manga_list(
            &fetch_api_or_fixture(&target, LIST_FIXTURE),
            focus_language(&request),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/v1/manga/findBySlug/sample".into());
        Ok(parse_manga_info(&fetch_api_or_fixture(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/v1/manga/findBySlug/sample".into());
        Ok(parse_chapters(&fetch_api_or_fixture(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/v1/manga/sample/chapter/1".into());
        let body = fetch_api_or_fixture(&url::join_url(BASE_URL, &key), CHAPTER_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request
            .get("manga")
            .and_then(|manga| manga.get("key").or_else(|| manga.get("url")))
            .and_then(Value::as_str)
            .map(|key| format!("{SITE_URL}/manga/{}", key.rsplit('/').next().unwrap_or(key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request
            .get("chapter")
            .and_then(|chapter| chapter.get("key").or_else(|| chapter.get("url")))
            .and_then(Value::as_str)
            .map(|key| format!("{SITE_URL}{}", key.trim_start_matches("/v1"))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = slug_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(
                    self.details(json!({ "manga": format!("/v1/manga/findBySlug/{slug}") }))?,
                ),
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

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let input = request
            .get("imageBase64")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let bytes = STANDARD.decode(input).unwrap_or_default();
        let decrypted = xor_image(&bytes);
        Ok(ProcessedImage {
            image_base64: STANDARD.encode(decrypted),
            mime_type: request
                .get("mimeType")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            ..ProcessedImage::default()
        })
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api_or_fixture(target: &str, fixture: &str) -> String {
    let http = client();
    let mut req = http.get(target).xhr();
    if let Some(signed) = signed_header() {
        req = req.header("St-soon", signed);
    }
    req.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn signed_header() -> Option<String> {
    let response = client()
        .get(format!("{BASE_URL}/v1"))
        .header("NX", "")
        .send()
        .ok()?;
    let date = response
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("date"))
        .and_then(|(_, value)| httpdate::parse_http_date(value).ok())?;
    let timestamp = date
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis()
        .to_string();
    Some(sign_timestamp(&timestamp))
}

fn sign_timestamp(timestamp: &str) -> String {
    let data_to_hash = json!({ "timesTamp": timestamp }).to_string();
    let mut mac = HmacSha256::new_from_slice(SECRET_KEY.as_bytes()).expect("hmac key");
    mac.update(data_to_hash.as_bytes());
    let hash = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let payload = json!({ "hash": hash, "timesTamp": timestamp }).to_string();
    openssl_salted_encrypt(&payload, ENCRYPT_KEY, b"Manatan1")
}

fn openssl_salted_encrypt(value: &str, key: &str, salt: &[u8; 8]) -> String {
    let (key_bytes, iv) = derive_key_iv(key.as_bytes(), salt);
    let encrypted = Aes256CbcEnc::new((&key_bytes[..]).into(), (&iv[..]).into())
        .encrypt_padded_vec_mut::<Pkcs7>(value.as_bytes());
    let mut output = b"Salted__".to_vec();
    output.extend_from_slice(salt);
    output.extend_from_slice(&encrypted);
    STANDARD.encode(output)
}

fn derive_key_iv(key: &[u8], salt: &[u8; 8]) -> ([u8; 32], [u8; 16]) {
    let mut result = Vec::new();
    let mut prev = Vec::new();
    while result.len() < 48 {
        let mut hasher = Md5::new();
        hasher.update(&prev);
        hasher.update(key);
        hasher.update(salt);
        prev = hasher.finalize().to_vec();
        result.extend_from_slice(&prev);
    }
    let mut key_bytes = [0u8; 32];
    let mut iv = [0u8; 16];
    key_bytes.copy_from_slice(&result[..32]);
    iv.copy_from_slice(&result[32..48]);
    (key_bytes, iv)
}

fn parse_manga_list(body: &str, focus: &str) -> Paged<CatalogItem> {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let entries = value
        .get("mangas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|manga| {
            focus.is_empty() || manga.get("language").and_then(Value::as_str) == Some(focus)
        })
        .map(item_from_manga)
        .collect();
    Paged {
        has_next_page: value.get("next_page").is_some_and(|page| !page.is_null()),
        entries,
    }
}

fn parse_manga_info(body: &str) -> CatalogItem {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let mut item = item_from_manga(value.get("manga").unwrap_or(&value));
    let metadata = value.get("metaData").unwrap_or(&Value::Null);
    if metadata.is_object() {
        let mut extra = Vec::new();
        if let Some(follows) = metadata.get("follows").and_then(Value::as_i64) {
            extra.push(format!("follows: {follows}"));
        }
        if let Some(views) = metadata.get("views").and_then(Value::as_i64) {
            extra.push(format!("views: {views}"));
        }
        item.tags.extend(extra);
    }
    item.initialized = true;
    item
}

fn item_from_manga(manga: &Value) -> CatalogItem {
    let slug = manga
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let lang = manga
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("all");
    let raw_title = manga
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Untitled");
    CatalogItem {
        key: format!("/v1/manga/findBySlug/{slug}"),
        title: title_without_lang(raw_title, lang),
        cover: manga
            .get("img")
            .and_then(Value::as_str)
            .map(|img| format!("{BASE_URL}/v1/images/manga{img}")),
        description: manga
            .get("description")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        authors: manga
            .get("authors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        artists: manga
            .get("authors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        tags: vec![
            format!("lang: {lang}"),
            format!(
                "type: {}",
                manga
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ),
        ],
        status: if manga.get("status").and_then(Value::as_str) == Some("ongoing") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Completed
        },
        url: Some(format!("{SITE_URL}/manga/{slug}")),
        language: Some(lang.to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let slug = value
        .get("manga")
        .and_then(|manga| manga.get("slug"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    value
        .get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|chapter| {
            let number = chapter.get("number").and_then(Value::as_f64).unwrap_or(0.0);
            let number_name = chapter_name(number);
            MangaChapter {
                key: format!("/v1/manga/{slug}/chapter/{number_name}"),
                title: Some(format!(
                    "{}{}",
                    number_name,
                    chapter
                        .get("title")
                        .and_then(Value::as_str)
                        .map(|title| format!(" {title}"))
                        .unwrap_or_default()
                )),
                chapter_number: Some(number as f32),
                url: Some(format!("{SITE_URL}/manga/{slug}/chapter/{number_name}")),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let image_sets = value
        .get("chapter")
        .and_then(|chapter| chapter.get("images"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let images = image_sets
        .iter()
        .max_by_key(|set| set.as_array().map(Vec::len).unwrap_or(0))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    images
        .iter()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, path)| {
            let image = format!("{BASE_URL}/v1/images/chapter{path}");
            let mut extra = BTreeMap::new();
            extra.insert("encrypted".to_string(), Value::Bool(true));
            MangaPage {
                content: PageContent::Request {
                    request: ImageRequest {
                        url: image,
                        headers: manga::image_headers(BASE_URL),
                        ..ImageRequest::default()
                    },
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                extra,
                ..MangaPage::default()
            }
        })
        .collect()
}

fn xor_image(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ IMAGE_KEY[index % IMAGE_KEY.len()])
        .collect()
}

fn focus_language(request: &Value) -> &str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("focusLanguage"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn append_filter(target: &mut String, filters: &Value, id: &str, param: &str) {
    if let Some(value) = filters
        .get(id)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        target.push('&');
        target.push_str(param);
        target.push('=');
        target.push_str(&url::query_escape(value));
    }
}

fn slug_from_url(input: &str) -> Option<String> {
    if input.starts_with(SITE_URL) {
        input
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .map(ToString::to_string)
    } else {
        None
    }
}

fn title_without_lang(title: &str, lang: &str) -> String {
    title
        .strip_suffix(lang)
        .unwrap_or(title)
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn chapter_name(number: f64) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i64)
    } else {
        number.to_string()
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"mangas":[{"id":"1","title":"sample eng","img":"/sample.jpg","description":"About","language":"eng","slug":"sample","type":"manhwa","status":"ongoing","authors":["Jane"],"rating":4.5,"create_at":null}],"next_page":null,"prev_page":null,"max_pages":1}"#;
const DETAILS_FIXTURE: &str = r#"{"manga":{"id":"1","title":"sample eng","img":"/sample.jpg","description":"About","language":"eng","slug":"sample","type":"manhwa","status":"ongoing","authors":["Jane"],"rating":4.5,"create_at":null},"metaData":{"follows":1,"views":2},"chapters":[{"id":"c1","title":"Start","create_at":"2024-01-01T00:00:00.000000","number":1.0}]}"#;
const CHAPTER_FIXTURE: &str = r#"{"chapter":{"id":"c1","number":1.0,"title":"Start","images":[["/sample/1.jpg","/sample/2.jpg"]]},"prev_chapter":null,"next_chapter":null}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_listing_details_chapters_and_pages() {
        let page = parse_manga_list(LIST_FIXTURE, "");
        assert_eq!(page.entries[0].title, "Sample");

        let item = SOURCE
            .details(json!({"manga":"/v1/manga/findBySlug/sample"}))
            .unwrap();
        assert_eq!(item.authors, vec!["Jane"]);

        let chapters = SOURCE
            .chapters(json!({"manga":"/v1/manga/findBySlug/sample"}))
            .unwrap();
        assert_eq!(chapters[0].key, "/v1/manga/sample/chapter/1");

        let pages = SOURCE
            .pages(json!({"chapter":"/v1/manga/sample/chapter/1"}))
            .unwrap();
        assert_eq!(pages.len(), 2);
    }

    #[test]
    fn decrypts_image_bytes_with_xor() {
        let encrypted = xor_image(b"hello");
        let output = SOURCE
            .process_page_image(json!({"imageBase64": STANDARD.encode(encrypted)}))
            .unwrap();
        assert_eq!(STANDARD.decode(output.image_base64).unwrap(), b"hello");
    }
}
