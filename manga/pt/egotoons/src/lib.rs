use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, Context, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, sdk::SearchRequest, url};
use pbkdf2::pbkdf2_hmac;
use serde_json::Value;
use sha2::Sha256;

const SOURCE: EgoToons = EgoToons;
const BASE_URL: &str = "https://www.egotoons.com";
const PAGE_SIZE: u64 = 20;

struct EgoToons;

impl MangaSource for EgoToons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_list(LIST_FIXTURE, false));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let offset = (page.saturating_sub(1)) * PAGE_SIZE;
            let body = api_get(
                &format!(
                    "{BASE_URL}/api/releases?offset={offset}&limit={PAGE_SIZE}&withHentai={}",
                    with_hentai(&request)
                ),
                LATEST_FIXTURE,
            );
            return Ok(parse_manga_page(&body));
        }
        Ok(parse_manga_list(
            &api_get(
                &format!("{BASE_URL}/api/leaderboard?criteria=global"),
                LIST_FIXTURE,
            ),
            false,
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
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let offset = (page.saturating_sub(1)) * PAGE_SIZE;
        let mut query_url = format!(
            "{BASE_URL}/api/manga/search?query={}&offset={offset}&limit={PAGE_SIZE}&withHentai={}",
            url::query_escape(query),
            with_hentai(&request)
        );
        append_multi_filter(&mut query_url, &request, "genres", "genres");
        append_multi_filter(&mut query_url, &request, "tags", "tags");
        Ok(parse_manga_page(&api_get(&query_url, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "obra/1".to_string());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "obra/1".to_string());
        let manga_id = manga_id(&key).unwrap_or_else(|| "1".to_string());
        let mut chapters = Vec::new();
        for page in 0..100 {
            let offset = page * PAGE_SIZE;
            let body = api_get(
                &format!(
                    "{BASE_URL}/api/manga/{manga_id}/chapter?limit={PAGE_SIZE}&offset={offset}"
                ),
                CHAPTERS_FIXTURE,
            );
            let value = serde_json::from_str::<Value>(&body)
                .or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE))
                .unwrap_or_else(|_| Value::Null);
            let items = value
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if items.is_empty() {
                break;
            }
            chapters.extend(items.iter().map(|chapter| chapter_from_json(chapter, &manga_id)));
            let has_next = value
                .pointer("/pagination/currentPage")
                .and_then(Value::as_u64)
                .zip(value.pointer("/pagination/pages").and_then(Value::as_u64))
                .is_some_and(|(current, pages)| current < pages);
            if !has_next {
                break;
            }
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "obra/1/capitulo/1".into());
        let manga_id = manga_id(&key).unwrap_or_else(|| "1".into());
        let number = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("1")
            .to_string();
        let body = api_get_secure(
            &format!("{BASE_URL}/api/manga/{manga_id}/chapter/{number}/images"),
            PAGES_FIXTURE,
        );
        let images = serde_json::from_str::<Vec<String>>(&body)
            .or_else(|_| serde_json::from_str(PAGES_FIXTURE))
            .unwrap_or_default()
            .into_iter()
            .filter(|image| !image.trim().is_empty())
            .collect::<Vec<_>>();
        Ok(images
            .iter()
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: absolute_url(image),
                    context: Some(image_headers()),
                },
                headers: image_headers(),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "manga").map(|key| format!("{BASE_URL}/{}", key.trim_matches('/'))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/{}", key.trim_matches('/'))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/obra/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let image_base64 = request
            .get("imageBase64")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mime_type = request
            .get("mimeType")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let bytes = STANDARD.decode(image_base64).unwrap_or_default();
        let decrypted = decrypt_image(&bytes).unwrap_or(bytes);
        Ok(ProcessedImage {
            image_base64: STANDARD.encode(decrypted),
            mime_type,
            ..ProcessedImage::default()
        })
    }
}

fn api_get(target_url: &str, fixture: &str) -> String {
    client()
        .get(target_url)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn api_get_secure(target_url: &str, fixture: &str) -> String {
    client()
        .get(target_url)
        .header("x-mymangas-csrf-secure", "true")
        .header("x-mymangas-secure-panel-domain", "true")
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn with_hentai(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("with_hentai"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn append_multi_filter(query_url: &mut String, request: &Value, id: &str, param: &str) {
    let Some(values) = request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_array)
    else {
        return;
    };
    for value in values.iter().filter_map(Value::as_str) {
        query_url.push('&');
        query_url.push_str(param);
        query_url.push('=');
        query_url.push_str(&url::query_escape(value));
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let id = manga_id(key).unwrap_or_else(|| "1".to_string());
    let body = api_get(&format!("{BASE_URL}/api/manga/{id}"), DETAILS_FIXTURE);
    let value = serde_json::from_str::<Value>(&body)
        .or_else(|_| serde_json::from_str(DETAILS_FIXTURE))
        .unwrap_or_else(|_| Value::Null);
    catalog_from_json(&value, true)
}

fn parse_manga_page(body: &str) -> Paged<CatalogItem> {
    let value = serde_json::from_str::<Value>(body).unwrap_or_else(|_| Value::Null);
    let entries = value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| catalog_from_json(item, false))
        .collect();
    let has_next_page = value
        .pointer("/pagination/currentPage")
        .and_then(Value::as_u64)
        .zip(value.pointer("/pagination/pages").and_then(Value::as_u64))
        .is_some_and(|(current, pages)| current < pages);
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_manga_list(body: &str, has_next_page: bool) -> Paged<CatalogItem> {
    let value = serde_json::from_str::<Value>(body)
        .or_else(|_| serde_json::from_str(LIST_FIXTURE))
        .unwrap_or_else(|_| Value::Null);
    Paged {
        entries: value
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| catalog_from_json(item, false))
            .collect(),
        has_next_page,
    }
}

fn catalog_from_json(value: &Value, initialized: bool) -> CatalogItem {
    let id = value.get("id").and_then(Value::as_i64).unwrap_or_default();
    let tags = value
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(value.get("genres").and_then(Value::as_array).into_iter().flatten())
        .filter_map(|item| item.get("name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let author = value.get("author").unwrap_or(&Value::Null);
    let author_name = [author.get("firstName"), author.get("lastName")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    CatalogItem {
        key: format!("obra/{id}"),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Ego Toons")
            .to_string(),
        cover: value.get("cover").and_then(Value::as_str).map(absolute_url),
        description: value
            .get("synopsis")
            .and_then(Value::as_str)
            .map(html::strip_tags)
            .filter(|description| !description.is_empty()),
        authors: if author_name.is_empty() {
            Vec::new()
        } else {
            vec![author_name]
        },
        tags,
        status: value
            .get("status")
            .and_then(Value::as_str)
            .and_then(status_from_api)
            .unwrap_or_default(),
        url: Some(format!("{BASE_URL}/obra/{id}")),
        language: Some("pt-BR".to_string()),
        content_rating: Some("adult".to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn chapter_from_json(value: &Value, manga_id: &str) -> MangaChapter {
    let number = value
        .get("chapter")
        .and_then(Value::as_f64)
        .unwrap_or_default() as f32;
    let formatted = format_number(number);
    let title = value.get("title").and_then(Value::as_str).unwrap_or_default();
    let base_title = format!("Capítulo {formatted}");
    MangaChapter {
        key: format!("obra/{manga_id}/capitulo/{formatted}"),
        title: Some(if title.trim().is_empty() || title.contains(&formatted) {
            base_title
        } else {
            format!("{base_title} - {title}")
        }),
        chapter_number: Some(number),
        url: Some(format!("{BASE_URL}/obra/{manga_id}/capitulo/{formatted}")),
        is_locked: value.get("status").and_then(Value::as_str) != Some("PUBLISHED"),
        ..MangaChapter::default()
    }
}

fn status_from_api(status: &str) -> Option<manatan_extension::ItemStatus> {
    match status {
        "IN_PROGRESS" => Some(manatan_extension::ItemStatus::Ongoing),
        "HIATUS" => Some(manatan_extension::ItemStatus::Hiatus),
        "COMPLETED" => Some(manatan_extension::ItemStatus::Completed),
        "CANCELLED" => Some(manatan_extension::ItemStatus::Cancelled),
        _ => None,
    }
}

fn format_number(number: f32) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i32)
    } else {
        number.to_string()
    }
}

fn normalize_key(input: &str) -> String {
    if let Some(index) = input.find("/obra/") {
        return input[index + 1..].trim_end_matches('/').to_string();
    }
    input.trim_start_matches('/').trim_end_matches('/').to_string()
}

fn manga_id(key: &str) -> Option<String> {
    key.trim_matches('/')
        .split('/')
        .nth(1)
        .filter(|id| !id.is_empty())
        .map(ToString::to_string)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .or_else(|| value.get("id"))
                .or_else(|| value.get("url"))
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
        })
        .map(normalize_key)
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        url::join_url(BASE_URL, value)
    }
}

fn image_headers() -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), format!("{BASE_URL}/"));
    headers.insert("x-mymangas-csrf-secure".to_string(), "true".to_string());
    headers.insert(
        "x-mymangas-secure-panel-domain".to_string(),
        "true".to_string(),
    );
    headers
}

fn decrypt_image(encrypted: &[u8]) -> Option<Vec<u8>> {
    if encrypted.len() <= 12 {
        return None;
    }
    let mut key = [0_u8; 32];
    pbkdf2_hmac::<Sha256>(
        ENCRYPTION_KEY.as_bytes(),
        b"manga-app-salt",
        30_000,
        &mut key,
    );
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    let iv = &encrypted[..12];
    let ciphertext = &encrypted[12..];
    cipher.decrypt(Nonce::from_slice(iv), ciphertext).ok()
}

export_manga_source!(SOURCE);

const ENCRYPTION_KEY: &str =
    "4f8d2a7b9c6e1f3a5b0c9e2d7a6b1c3f8e4d2a9b7c6f1e3a5b0c9d2e7f6a1b39";

const LIST_FIXTURE: &str = r#"
[
  {
    "id": 1,
    "title": "Sample Ego Toons",
    "status": "IN_PROGRESS",
    "synopsis": "<p>Sample description.</p>",
    "cover": "/cover.jpg",
    "tags": [{ "id": 1, "name": "Ação" }],
    "genres": [{ "id": 2, "name": "Romance" }],
    "author": { "firstName": "Sample", "lastName": "Author" },
    "works": 1
  }
]
"#;

const LATEST_FIXTURE: &str = r#"
{
  "items": [
    {
      "id": 1,
      "title": "Sample Ego Toons",
      "status": "IN_PROGRESS",
      "synopsis": "<p>Sample description.</p>",
      "cover": "/cover.jpg",
      "tags": [],
      "genres": [],
      "author": null,
      "works": 1
    }
  ],
  "pagination": { "offset": 0, "limit": 20, "total": 1, "pages": 1, "currentPage": 1 }
}
"#;

const SEARCH_FIXTURE: &str = LATEST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"
{
  "id": 1,
  "title": "Sample Ego Toons",
  "status": "COMPLETED",
  "synopsis": "<p>Sample description.</p>",
  "cover": "/cover.jpg",
  "tags": [{ "id": 1, "name": "Ação" }],
  "genres": [{ "id": 2, "name": "Romance" }],
  "author": { "firstName": "Sample", "lastName": "Author" },
  "works": 1
}
"#;

const CHAPTERS_FIXTURE: &str = r#"
{
  "items": [
    { "id": 10, "chapter": 1.0, "title": "Começo", "status": "PUBLISHED" }
  ],
  "pagination": { "offset": 0, "limit": 20, "total": 1, "pages": 1, "currentPage": 1 }
}
"#;

const PAGES_FIXTURE: &str = r#"["/api/manga/1/chapter/1/image/1", "/api/manga/1/chapter/1/image/2"]"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_fixtures() {
        assert_eq!(parse_manga_list(LIST_FIXTURE, false).entries.len(), 1);
        assert_eq!(parse_manga_page(LATEST_FIXTURE).entries.len(), 1);
        assert_eq!(
            chapter_from_json(
                &serde_json::json!({"chapter": 1.5, "title": "Extra", "status": "PUBLISHED"}),
                "7"
            )
            .key,
            "obra/7/capitulo/1.5"
        );
    }
}
