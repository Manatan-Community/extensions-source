use aes::{
    Aes256,
    cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray},
};
use base64::{Engine as _, engine::general_purpose};
use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Firescans = Firescans;

struct Firescans;

impl MangaSource for Firescans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: manga::Madara::parse_listing(LIST_FIXTURE, &config),
                has_next_page: manga::Madara::has_next_page(LIST_FIXTURE, &config),
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.list_url(page, order), LIST_FIXTURE);
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &config),
            has_next_page: manga::Madara::has_next_page(&body, &config),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(config.base_url) {
            let key = config.normalize_manga_key(query);
            let body = manga::Madara::fetch_document_or_fixture(&config, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(&body, Some(key), &config)],
                has_next_page: false,
            });
        }
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.search_url(page, query),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &config),
            has_next_page: manga::Madara::has_next_page(&body, &config),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config();
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_details(&body, Some(key), &config))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config();
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_chapters(&body, &key, &config))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config();
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), PAGES_FIXTURE);
        let protected = parse_protected_pages(&body, &config);
        if !protected.is_empty() {
            return Ok(protected);
        }
        Ok(manga::Madara::parse_pages(&body, &config))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = config();
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(config.base_url) {
            return Ok(Some(UrlResolveResult {
                item: Some(manga::Madara::parse_details(
                    &manga::Madara::fetch_document_or_fixture(&config, input, DETAILS_FIXTURE),
                    Some(config.normalize_manga_key(input)),
                    &config,
                )),
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
}

fn config() -> MadaraConfig {
    MadaraConfig {
        base_url: "https://firescans.xyz",
        lang: "en",
        content_rating: "safe",
        manga_path: "manga",
        popular_url_marker: "post-title",
        use_load_more: false,
        latest_enabled: true,
    }
}

fn parse_protected_pages(body: &str, config: &MadaraConfig) -> Vec<MangaPage> {
    let Some(script) = chapter_protector_script(body) else {
        return Vec::new();
    };
    let password = script
        .split("wpmangaprotectornonce='")
        .nth(1)
        .and_then(|rest| rest.split("';").next())
        .unwrap_or_default();
    let Some(raw) = script
        .split("chapter_data='")
        .nth(1)
        .and_then(|rest| rest.split("';").next())
    else {
        return Vec::new();
    };
    let cleaned = raw.replace("\\/", "/").replace("\\\"", "\"");
    let images = serde_json::from_str::<Value>(&cleaned)
        .ok()
        .and_then(|value| protected_image_values(value, password))
        .unwrap_or_default();
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(config.base_url, &image),
                context: Some(manga::image_headers(config.base_url)),
            },
            headers: manga::image_headers(config.base_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn chapter_protector_script(body: &str) -> Option<String> {
    let chunk = body
        .split("chapter-protector-data")
        .nth(1)
        .filter(|chunk| chunk.contains("chapter_data"))?;
    if let Some(src) =
        html::attr(chunk, "src").filter(|src| src.starts_with("data:text/javascript;base64,"))
    {
        let encoded = src.split_once(',')?.1;
        return general_purpose::STANDARD
            .decode(encoded)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok());
    }
    Some(chunk.to_string())
}

fn protected_image_values(value: Value, password: &str) -> Option<Vec<String>> {
    if let Some(array) = value.as_array() {
        return Some(
            array
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect(),
        );
    }
    let object = value.as_object()?;
    let ct = object.get("ct")?.as_str()?;
    let salt = object.get("s")?.as_str()?;
    let decrypted = cryptojs_decrypt(ct, salt, password)?;
    let decoded = serde_json::from_str::<Value>(&decrypted).ok()?;
    let array_value = if let Some(text) = decoded.as_str() {
        serde_json::from_str::<Value>(text).ok()?
    } else {
        decoded
    };
    Some(
        array_value
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
    )
}

fn cryptojs_decrypt(ct_base64: &str, salt_hex: &str, password: &str) -> Option<String> {
    let cipher_text = general_purpose::STANDARD.decode(ct_base64).ok()?;
    let salt = hex::decode(salt_hex).ok()?;
    if salt.len() < 8 {
        return None;
    }
    let (key, iv) = evp_bytes_to_key(password.as_bytes(), &salt[..8]);
    let plaintext = aes256_cbc_decrypt(&cipher_text, &key, &iv)?;
    String::from_utf8(plaintext).ok()
}

fn evp_bytes_to_key(password: &[u8], salt: &[u8]) -> ([u8; 32], [u8; 16]) {
    let mut generated = Vec::with_capacity(48);
    let mut previous = Vec::new();
    while generated.len() < 48 {
        let mut input = Vec::with_capacity(previous.len() + password.len() + salt.len());
        input.extend_from_slice(&previous);
        input.extend_from_slice(password);
        input.extend_from_slice(salt);
        previous = md5::compute(input).0.to_vec();
        generated.extend_from_slice(&previous);
    }
    let mut key = [0u8; 32];
    let mut iv = [0u8; 16];
    key.copy_from_slice(&generated[..32]);
    iv.copy_from_slice(&generated[32..48]);
    (key, iv)
}

fn aes256_cbc_decrypt(cipher_text: &[u8], key: &[u8; 32], iv: &[u8; 16]) -> Option<Vec<u8>> {
    if cipher_text.is_empty() || cipher_text.len() % 16 != 0 {
        return None;
    }
    let cipher = Aes256::new(GenericArray::from_slice(key));
    let mut previous = *iv;
    let mut out = Vec::with_capacity(cipher_text.len());
    for block in cipher_text.chunks(16) {
        let mut decrypted = GenericArray::clone_from_slice(block);
        cipher.decrypt_block(&mut decrypted);
        for (idx, byte) in decrypted.iter().enumerate() {
            out.push(byte ^ previous[idx]);
        }
        previous.copy_from_slice(block);
    }
    let padding = *out.last()? as usize;
    if padding == 0 || padding > 16 || padding > out.len() {
        return None;
    }
    let unpadded_len = out.len() - padding;
    if out[unpadded_len..]
        .iter()
        .any(|byte| *byte as usize != padding)
    {
        return None;
    }
    out.truncate(unpadded_len);
    Some(out)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div>
<div class="nav-previous"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample Manga</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<div class="post-status"><div class="post-content_item"><h5>Genres</h5><div class="summary-content"><a>Drama</a></div></div></div>
<ul class="main version-chap"><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">2024-01-01</span></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"><img class="wp-manga-chapter-img" src="/page2.jpg"></div>"#;
#[cfg(test)]
const PROTECTED_FIXTURE: &str = r#"<script id="chapter-protector-data">wpmangaprotectornonce='pass'; chapter_data='["/page1.jpg","/page2.jpg"]';</script>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_madara_source() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Manga"
        );
        assert_eq!(
            SOURCE
                .pages(json!({"chapter":"/manga/sample/chapter-1"}))
                .unwrap()
                .len(),
            2
        );
        assert_eq!(parse_protected_pages(PROTECTED_FIXTURE, &config()).len(), 2);
    }
}
