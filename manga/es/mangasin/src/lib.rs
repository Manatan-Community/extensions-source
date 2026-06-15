use aes::{
    Aes256,
    cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

const SOURCE: MangasIn = MangasIn;
const BASE_URL: &str = "https://m440.in";
const CONTENT_RATING: &str = "adult";

struct MangasIn;

impl MangaSource for MangasIn {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_latest(
                &fetch_text(&format!("{BASE_URL}/lasted?p={page}"), LATEST_FIXTURE),
                page,
            ));
        }
        Ok(parse_listing(&fetch_text(
            &format!("{BASE_URL}/filterList?page={page}&sortBy=views&asc=false"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) && query.contains("/manga/") {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_text(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(parse_suggestions(&fetch_text(
                &format!("{BASE_URL}/search?q={}", url::query_escape(query)),
                SEARCH_FIXTURE,
            )));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_text(
            &format!("{BASE_URL}/filterList?page={page}"),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_text(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_text(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_encrypted_chapters(&body, &key).unwrap_or_else(|| parse_chapters(&body, &key)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_text(&absolute_url(&key), PAGES_FIXTURE)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_text(input, DETAILS_FIXTURE),
                    Some(key),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    format!("/{}", path.trim_matches('/'))
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| chunk.contains("media") || chunk.contains("chapter-container"))
            .filter_map(parse_card)
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("rel=\"next\"") || body.contains("pagination"),
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
    if !href.contains("/manga/") {
        return None;
    }
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "media-heading", "</")
            .or_else(|| html::text_between(chunk, "manga-heading", "</"))
            .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: image_attr(chunk)
            .map(|image| absolute_url(&image))
            .or_else(|| Some(guess_cover(&key))),
        url: Some(absolute_url(&key)),
        language: Some("es".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_latest(body: &str, page: u64) -> Paged<CatalogItem> {
    let Ok(root) = serde_json::from_str::<LatestResponse>(body) else {
        return parse_listing(body);
    };
    Paged {
        has_next_page: page < root.total_pages,
        entries: root
            .data
            .into_iter()
            .map(|entry| {
                let key = format!("/manga/{}", entry.slug.trim_matches('/'));
                CatalogItem {
                    key: key.clone(),
                    title: entry.name,
                    cover: Some(guess_cover(&key)),
                    url: Some(absolute_url(&key)),
                    language: Some("es".to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                }
            })
            .collect(),
    }
}

fn parse_suggestions(body: &str) -> Paged<CatalogItem> {
    let suggestions = serde_json::from_str::<Vec<Suggestion>>(body)
        .or_else(|_| serde_json::from_str::<SearchWrapper>(body).map(|wrapper| wrapper.suggestions))
        .unwrap_or_default();
    Paged {
        has_next_page: false,
        entries: suggestions
            .into_iter()
            .map(|suggestion| {
                let key = format!("/manga/{}", suggestion.data.trim_matches('/'));
                CatalogItem {
                    key: key.clone(),
                    title: suggestion.value,
                    cover: Some(guess_cover(&key)),
                    url: Some(absolute_url(&key)),
                    language: Some("es".to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                }
            })
            .collect(),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "listmanga-header", "</")
            .or_else(|| html::text_between(body, "widget-title", "</"))
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: image_attr(body)
            .map(|image| absolute_url(&image))
            .or_else(|| Some(guess_cover(&key))),
        description: html::text_between(body, "well", "</div>")
            .or_else(|| html::text_between(body, "description", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: panel_value(body, "autor").into_iter().collect(),
        artists: panel_value(body, "artista").into_iter().collect(),
        tags: panel_value(body, "categor")
            .into_iter()
            .flat_map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|value| !value.is_empty())
            .collect(),
        status: status_from_body(body),
        url: Some(absolute_url(&key)),
        language: Some("es".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-title-rtl") || chunk.contains("chapters"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: html::text_between(chunk, "date-chapter-title-rtl", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .chain(std::iter::once(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Read".to_string()),
            url: Some(absolute_url(manga_key)),
            ..MangaChapter::default()
        }))
        .take_while(|chapter| chapter.key != manga_key || !body.contains("chapter-title-rtl"))
        .collect()
}

fn parse_encrypted_chapters(body: &str, manga_key: &str) -> Option<Vec<MangaChapter>> {
    let raw = encrypted_chapter_json(body)?;
    let data: CryptoPayload = serde_json::from_str(&raw).ok()?;
    let key = fetch_decryption_key().unwrap_or_default();
    let decrypted = cryptojs_decrypt(&data.ct, &data.s, &key)?;
    let unescaped = unescape_js_string(&decrypted)
        .trim_matches('"')
        .replace("\\/", "/");
    let chapters = serde_json::from_str::<Vec<ChapterDto>>(&unescaped)
        .or_else(|_| serde_json::from_str::<Vec<ChapterDto>>(&html::html_unescape(&unescaped)))
        .ok()?;
    Some(
        chapters
            .into_iter()
            .map(|chapter| {
                let key = format!(
                    "{}/{}",
                    manga_key.trim_end_matches('/'),
                    chapter.slug.trim_matches('/')
                );
                let title = if chapter.name == format!("Capítulo {}", chapter.number) {
                    chapter.name
                } else {
                    format!("Capítulo {}: {}", chapter.number, chapter.name)
                };
                MangaChapter {
                    key: key.clone(),
                    title: Some(title),
                    date_uploaded: manatan_shared::dates::parse_fixture_date(&chapter.created_at),
                    url: Some(absolute_url(&key)),
                    ..MangaChapter::default()
                }
            })
            .collect(),
    )
}

fn encrypted_chapter_json(body: &str) -> Option<String> {
    let re = Regex::new(r#"\{(?s:[^{}]*\\?"ct\\?"[^{}]*\\?"s\\?"[^{}]*)\}"#).ok()?;
    let raw = re.find(body)?.as_str();
    Some(raw.replace("\\\"", "\"").replace("\\/", "/"))
}

fn fetch_decryption_key() -> Option<String> {
    let script = client()
        .get(format!("{BASE_URL}/js/ads2.js"))
        .send_text()
        .ok()?;
    key_from_script(&script)
}

fn key_from_script(script: &str) -> Option<String> {
    let call_re = Regex::new(r#"decrypt\([^,]+,\s*([^,\)]+)"#).ok()?;
    let token = call_re.captures(script)?.get(1)?.as_str().trim();
    if token.starts_with('"') || token.starts_with('\'') {
        return Some(token.trim_matches(&['"', '\''][..]).to_string());
    }
    let var_re = Regex::new(&format!(
        r#"(?:let|var|const)\s+{}\s*=\s*['"]([^'"]+)['"]"#,
        regex::escape(token)
    ))
    .ok()?;
    var_re
        .captures(script)
        .and_then(|caps| caps.get(1).map(|value| value.as_str().to_string()))
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("img-responsive") || chunk.contains("#all"))
        .filter_map(image_attr)
        .filter_map(|image| decode_page_image(&image))
        .enumerate()
        .map(|(index, image)| {
            let headers = manga::image_headers(BASE_URL);
            MangaPage {
                content: PageContent::Url {
                    url: absolute_url(&image),
                    context: Some(headers.clone()),
                },
                headers,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn decode_page_image(value: &str) -> Option<String> {
    if value.starts_with("http") || value.starts_with('/') {
        return Some(value.to_string());
    }
    let encoded = value.split("://").nth(1)?;
    let decoded = STANDARD.decode(encoded).ok()?;
    String::from_utf8(decoded)
        .ok()
        .map(|text| percent_decode(&text))
}

fn cryptojs_decrypt(ct_base64: &str, salt_hex: &str, password: &str) -> Option<String> {
    let cipher_text = STANDARD.decode(ct_base64).ok()?;
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

fn image_attr(input: &str) -> Option<String> {
    [
        "data-background-image",
        "data-cfsrc",
        "data-lazy-src",
        "data-src",
        "src",
    ]
    .into_iter()
    .find_map(|attr| html::attr_after(input, "<img", attr).or_else(|| html::attr(input, attr)))
}

fn panel_value(body: &str, label: &str) -> Option<String> {
    body.split("dl-horizontal")
        .chain(body.split("post-content_item"))
        .find(|chunk| chunk.to_ascii_lowercase().contains(label))
        .map(html::strip_tags)
        .map(|value| {
            value
                .replace("Autor:", "")
                .replace("Autor", "")
                .replace("Artista:", "")
                .replace("Artista", "")
                .replace("Categorías:", "")
                .replace("Categorias:", "")
                .replace("Categorías", "")
                .replace("Categorias", "")
        })
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|value| !value.is_empty())
}

fn status_from_body(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("finalizado") || lower.contains("completo") {
        ItemStatus::Completed
    } else if lower.contains("dropped") || lower.contains("cancelado") {
        ItemStatus::Cancelled
    } else if lower.contains("activo") || lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn guess_cover(key: &str) -> String {
    format!(
        "{BASE_URL}/uploads/manga/{}/cover/cover_250x350.jpg",
        key.trim_matches('/').trim_start_matches("manga/")
    )
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn percent_decode(input: &str) -> String {
    let mut out = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&input[index + 1..index + 3], 16) {
                out.push(hex);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn unescape_js_string(input: &str) -> String {
    let mut out = String::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('u') => {
                let hex = chars.by_ref().take(4).collect::<String>();
                if let Ok(value) = u32::from_str_radix(&hex, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or(())
                {
                    out.push(value);
                }
            }
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

#[derive(Deserialize)]
struct LatestResponse {
    data: Vec<LatestEntry>,
    #[serde(rename = "totalPages")]
    total_pages: u64,
}

#[derive(Deserialize)]
struct LatestEntry {
    #[serde(rename = "manga_name")]
    name: String,
    #[serde(rename = "manga_slug")]
    slug: String,
}

#[derive(Deserialize, Default)]
struct SearchWrapper {
    suggestions: Vec<Suggestion>,
}

#[derive(Deserialize, Default)]
struct Suggestion {
    value: String,
    data: String,
}

#[derive(Deserialize)]
struct CryptoPayload {
    ct: String,
    s: String,
}

#[derive(Deserialize)]
struct ChapterDto {
    slug: String,
    name: String,
    number: String,
    #[serde(rename = "created_at")]
    created_at: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="media"><a href="/manga/sample"><img src="/cover.jpg"><h3 class="media-heading">Sample Manga</h3></a></div>
<a rel="next" href="/filterList?page=2">next</a>
"#;
const LATEST_FIXTURE: &str =
    r#"{"data":[{"manga_name":"Sample Manga","manga_slug":"sample"}],"totalPages":1}"#;
const SEARCH_FIXTURE: &str = r#"[{"value":"Sample Manga","data":"sample"}]"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="listmanga-header">Sample Manga</h1><div class="row"><img class="img-responsive" src="/cover.jpg"><div class="well">Resumen</div></div>
<ul class="chapters"><li><div class="chapter-title-rtl"><a href="/manga/sample/chapter-1">Chapter 1</a></div><span class="date-chapter-title-rtl">2024-01-01</span></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"<div id="all"><img class="img-responsive" src="/page1.jpg"><img class="img-responsive" src="x://aHR0cHMlM0ElMkYlMkZtNDQwLmluJTJGcGFnZTIuanBn"></div>"#;
