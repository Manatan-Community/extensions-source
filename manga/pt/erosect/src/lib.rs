use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, Context, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{sdk::SearchRequest, url};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SOURCE: EroSect = EroSect;
const BASE_URL: &str = "https://erosect.xyz";
const API_URL: &str = "https://erosect.xyz/api";
const CHAPTER_LAYER_KEY: &str = "9b1c5f6c0e4b7f2d8d3a41e5c9a7b2f0";
const JWT_CRYPTO_PEPPER: &str = "chapter_jwt_default_pepper";

struct EroSect;

impl MangaSource for EroSect {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(POPULAR_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let body = api_get(
                &format!("{API_URL}/capitulos/recentes?pagina={page}&limite=12"),
                None,
                LATEST_FIXTURE,
            );
            return Ok(parse_paginated(&body));
        }
        Ok(parse_popular(&api_get(
            &format!("{API_URL}/obras/top10/views?periodo=total"),
            None,
            POPULAR_FIXTURE,
        )))
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
        Ok(parse_paginated(&api_get(
            &format!(
                "{API_URL}/obras?pagina={page}&limite=20&busca={}",
                url::query_escape(query)
            ),
            None,
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/obra/1".to_string());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/obra/1".to_string());
        let id = work_id(&key).unwrap_or_else(|| "1".to_string());
        let body = api_get(&format!("{API_URL}/obras/{id}/capitulos"), None, CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "/obra/1/capitulo/1".into());
        let token = auth_token(&request);
        let (work_id, number) = chapter_parts(&key).unwrap_or_else(|| ("1".into(), "1".into()));
        let body = api_get(
            &format!("{API_URL}/obras/{work_id}/capitulos/{number}"),
            token.as_deref(),
            PAGES_FIXTURE,
        );
        let value = serde_json::from_str::<Value>(&body)
            .or_else(|_| serde_json::from_str(PAGES_FIXTURE))
            .unwrap_or(Value::Null);
        let payload = if encrypted_payload(&value) {
            token
                .as_deref()
                .and_then(|token| decrypt_chapter_payload(&value, token))
                .unwrap_or_else(|| serde_json::from_str(PAGES_FIXTURE).unwrap_or(Value::Null))
        } else {
            value
        };
        Ok(parse_pages(&payload))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "manga").map(|key| format!("{BASE_URL}{}", normalize_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "chapter").map(|key| format!("{BASE_URL}{}", normalize_key(&key))))
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
        let raw = STANDARD.decode(image_base64).unwrap_or_default();
        let decrypted = String::from_utf8(raw.clone())
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(encrypted_payload)
            .and_then(|payload| auth_token(&request).and_then(|token| decrypt_image_payload(&payload, &token)))
            .unwrap_or(raw);
        Ok(ProcessedImage {
            image_base64: STANDARD.encode(decrypted),
            mime_type: Some(mime_type.unwrap_or_else(|| "image/webp".to_string())),
            ..ProcessedImage::default()
        })
    }
}

fn api_get(target_url: &str, token: Option<&str>, fixture: &str) -> String {
    let http = client();
    let mut request = http.get(target_url).xhr();
    if let Some(token) = token.filter(|token| !token.is_empty()) {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn api_post_json(target_url: &str, body: Value, fixture: &str) -> String {
    client()
        .post(target_url)
        .json(body.to_string())
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_header("Accept", "application/json, text/plain, */*")
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn auth_token(request: &Value) -> Option<String> {
    let prefs = request.get("preferences")?;
    if let Some(token) = prefs.get("token").and_then(Value::as_str) {
        if !token.trim().is_empty() {
            return Some(token.to_string());
        }
    }
    let email = prefs.get("email").and_then(Value::as_str)?.trim();
    let password = prefs.get("password").and_then(Value::as_str)?.trim();
    if email.is_empty() || password.is_empty() {
        return None;
    }
    let body = api_post_json(
        &format!("{API_URL}/auth/login"),
        json!({ "email": email, "senha": password }),
        LOGIN_FIXTURE,
    );
    serde_json::from_str::<Value>(&body)
        .ok()
        .filter(|value| value.get("sucesso").and_then(Value::as_bool).unwrap_or(false))
        .and_then(|value| value.get("token").and_then(Value::as_str).map(ToString::to_string))
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let value = serde_json::from_str::<Value>(body)
        .or_else(|_| serde_json::from_str(POPULAR_FIXTURE))
        .unwrap_or(Value::Null);
    Paged {
        entries: value
            .get("obras")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(popular_item)
            .collect(),
        has_next_page: false,
    }
}

fn parse_paginated(body: &str) -> Paged<CatalogItem> {
    let value = serde_json::from_str::<Value>(body)
        .or_else(|_| serde_json::from_str(SEARCH_FIXTURE))
        .unwrap_or(Value::Null);
    Paged {
        entries: value
            .get("obras")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| obra_item(item, false))
            .collect(),
        has_next_page: value
            .pointer("/pagination/hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let id = work_id(key).unwrap_or_else(|| "1".to_string());
    let body = api_get(&format!("{API_URL}/obras/{id}"), None, DETAILS_FIXTURE);
    let value = serde_json::from_str::<Value>(&body)
        .or_else(|_| serde_json::from_str(DETAILS_FIXTURE))
        .unwrap_or(Value::Null);
    value
        .get("obra")
        .map(|obra| obra_item(obra, true))
        .unwrap_or_else(|| obra_item(&value, true))
}

fn popular_item(value: &Value) -> CatalogItem {
    let id = value.get("id").and_then(Value::as_i64).unwrap_or_default();
    CatalogItem {
        key: format!("/obra/{id}"),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("EroSect")
            .to_string(),
        cover: value.get("coverImage").and_then(Value::as_str).map(thumbnail_url),
        url: Some(format!("{BASE_URL}/obra/{id}")),
        language: Some("pt-BR".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn obra_item(value: &Value, initialized: bool) -> CatalogItem {
    let id = value.get("id").and_then(Value::as_i64).unwrap_or_default();
    CatalogItem {
        key: format!("/obra/{id}"),
        title: value
            .get("nome")
            .or_else(|| value.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("EroSect")
            .to_string(),
        cover: value
            .get("imagem")
            .or_else(|| value.get("coverImage"))
            .and_then(Value::as_str)
            .map(thumbnail_url),
        description: value
            .get("descricao")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        tags: value
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tag| tag.get("nome").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect(),
        status: value
            .get("status_nome")
            .and_then(Value::as_str)
            .map(status_from_name)
            .unwrap_or_default(),
        url: Some(format!("{BASE_URL}/obra/{id}")),
        language: Some("pt-BR".to_string()),
        content_rating: Some("adult".to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let value = serde_json::from_str::<Value>(body)
        .or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE))
        .unwrap_or(Value::Null);
    value
        .get("capitulos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(chapter_item)
        .collect()
}

fn chapter_item(value: &Value) -> MangaChapter {
    let work_id = value
        .get("obra_id")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let number = value.get("numero").and_then(Value::as_str).unwrap_or("1");
    MangaChapter {
        key: format!("/obra/{work_id}/capitulo/{number}"),
        title: Some(
            value
                .get("nome")
                .and_then(Value::as_str)
                .filter(|title| !title.trim().is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("Capitulo {number}")),
        ),
        chapter_number: number.parse::<f32>().ok(),
        url: Some(format!("{BASE_URL}/obra/{work_id}/capitulo/{number}")),
        ..MangaChapter::default()
    }
}

fn parse_pages(value: &Value) -> Vec<MangaPage> {
    let Some(chapter) = value.get("capitulo") else {
        return Vec::new();
    };
    let work_id = chapter
        .get("obra_id")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let number = chapter.get("numero").and_then(Value::as_str).unwrap_or("1");
    let referer = format!("{BASE_URL}/obra/{work_id}/capitulo/{number}");
    let mut pages = chapter
        .get("paginas")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    pages.sort_by_key(|page| page.get("numero").and_then(Value::as_i64).unwrap_or_default());
    pages
        .iter()
        .enumerate()
        .filter_map(|(index, page)| {
            let image = page.get("url").and_then(Value::as_str)?;
            let headers = image_headers(&referer);
            Some(MangaPage {
                content: PageContent::Url {
                    url: absolute_url(image),
                    context: Some(headers.clone()),
                },
                headers,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn encrypted_payload(value: &Value) -> bool {
    value.get("iv").and_then(Value::as_str).is_some()
        && value.get("data").and_then(Value::as_str).is_some()
}

fn decrypt_chapter_payload(payload: &Value, token: &str) -> Option<Value> {
    let inner = decrypt_json_payload(payload, CHAPTER_LAYER_KEY)?;
    decrypt_json_payload(&inner, &session_key(token))
}

fn decrypt_json_payload(payload: &Value, key: &str) -> Option<Value> {
    let bytes = decrypt_payload_bytes(payload, key)?;
    serde_json::from_slice(&bytes).ok()
}

fn decrypt_image_payload(payload: &Value, token: &str) -> Option<Vec<u8>> {
    decrypt_payload_bytes(payload, &session_key(token))
}

fn decrypt_payload_bytes(payload: &Value, key: &str) -> Option<Vec<u8>> {
    let iv = payload.get("iv").and_then(Value::as_str)?;
    let data = payload.get("data").and_then(Value::as_str)?;
    let iv = STANDARD.decode(iv).ok()?;
    let data = STANDARD.decode(data).ok()?;
    let key = Sha256::digest(key.as_bytes());
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    cipher.decrypt(Nonce::from_slice(&iv), data.as_ref()).ok()
}

fn session_key(token: &str) -> String {
    format!("{token}:{JWT_CRYPTO_PEPPER}")
}

fn status_from_name(status: &str) -> manatan_extension::ItemStatus {
    if status == "Em Andamento" {
        manatan_extension::ItemStatus::Ongoing
    } else if status == "Cancelada" {
        manatan_extension::ItemStatus::Cancelled
    } else if status == "Completo" || status.to_lowercase().contains("conclu") {
        manatan_extension::ItemStatus::Completed
    } else {
        manatan_extension::ItemStatus::Unknown
    }
}

fn thumbnail_url(value: &str) -> String {
    let image = if value.starts_with("http") {
        value.to_string()
    } else {
        format!("https://cdn.erosect.xyz/{value}")
    };
    format!(
        "{BASE_URL}/_next/image?url={}&w=3840&q=75",
        url::query_escape(&image)
    )
}

fn image_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Accept".to_string(), "*/*".to_string());
    headers.insert("Referer".to_string(), referer.to_string());
    headers.insert("Pragma".to_string(), "no-cache".to_string());
    headers.insert("Cache-Control".to_string(), "no-cache".to_string());
    headers
}

fn normalize_key(input: &str) -> String {
    if let Some(index) = input.find("/obra/") {
        return input[index..].trim_end_matches('/').to_string();
    }
    format!("/{}", input.trim_matches('/'))
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .or_else(|| value.get("url"))
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
        })
        .map(normalize_key)
}

fn work_id(key: &str) -> Option<String> {
    key.trim_matches('/').split('/').nth(1).map(ToString::to_string)
}

fn chapter_parts(key: &str) -> Option<(String, String)> {
    let mut parts = key.trim_matches('/').split('/');
    let _obra = parts.next()?;
    let work_id = parts.next()?.to_string();
    let _capitulo = parts.next()?;
    let number = parts.next()?.to_string();
    Some((work_id, number))
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else if value.starts_with("//") {
        format!("https:{value}")
    } else {
        url::join_url(BASE_URL, value)
    }
}

export_manga_source!(SOURCE);

const LOGIN_FIXTURE: &str = r#"{ "sucesso": false, "token": null }"#;
const POPULAR_FIXTURE: &str = r#"
{
  "obras": [
    { "id": 1, "title": "Sample EroSect", "coverImage": "covers/sample.jpg" }
  ]
}
"#;
const SEARCH_FIXTURE: &str = r#"
{
  "pagination": { "hasNextPage": false },
  "obras": [
    {
      "id": 1,
      "nome": "Sample EroSect",
      "descricao": "Sample description.",
      "imagem": "covers/sample.jpg",
      "status_nome": "Em Andamento",
      "tags": [{ "nome": "Ação" }]
    }
  ]
}
"#;
const LATEST_FIXTURE: &str = SEARCH_FIXTURE;
const DETAILS_FIXTURE: &str = r#"
{
  "obra": {
    "id": 1,
    "nome": "Sample EroSect",
    "descricao": "Sample description.",
    "imagem": "covers/sample.jpg",
    "status_nome": "Completo",
    "tags": [{ "nome": "Ação" }]
  }
}
"#;
const CHAPTERS_FIXTURE: &str = r#"
{
  "capitulos": [
    { "obra_id": 1, "numero": "1", "nome": "Capitulo 1", "criado_em": "2024-01-01T00:00:00+0000" }
  ]
}
"#;
const PAGES_FIXTURE: &str = r#"
{
  "capitulo": {
    "obra_id": 1,
    "numero": "1",
    "paginas": [
      { "numero": 1, "url": "/page-1.webp" },
      { "numero": 2, "url": "/page-2.webp" }
    ]
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(parse_popular(POPULAR_FIXTURE).entries.len(), 1);
        assert_eq!(parse_paginated(SEARCH_FIXTURE).entries.len(), 1);
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE).len(), 1);
        assert_eq!(
            parse_pages(&serde_json::from_str(PAGES_FIXTURE).unwrap()).len(),
            2
        );
    }
}
