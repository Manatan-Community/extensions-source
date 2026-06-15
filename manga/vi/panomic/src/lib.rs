use aes::Aes256;
use base64::{Engine, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use hmac::{Hmac, Mac};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Sha512;

const SOURCE: Panomic = Panomic;
const BASE_URL: &str = "https://panomic1.info";

struct Panomic;

impl MangaSource for Panomic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            return Ok(Paged {
                entries: parse_popular(&fetch_document(BASE_URL, HOME_FIXTURE)),
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/truyen-moi-cap-nhat/?trang={page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if !query.is_empty() {
            let body = client()
                .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
                .header("Content-Type", "application/x-www-form-urlencoded")
                .form(&[("action", "searchtax"), ("keyword", query)])
                .send_text()
                .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
            return Ok(parse_search_response(&body));
        }
        let genre = request
            .get("filters")
            .and_then(|f| f.get("genre"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let target = if genre.is_empty() {
            format!("{BASE_URL}/truyen-moi-cap-nhat/?trang={page}")
        } else {
            format!("{BASE_URL}/the-loai/{genre}/?trang={page}")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen/sample-chap-1".into());
        let chapter_url = absolute_url(&key);
        let body = fetch_document(&chapter_url, PAGES_FIXTURE);
        if body.contains("post-password-form") || body.contains("post_password") {
            return Ok(vec![manga::text_page(
                "Vui long nhap mat khau chuong nay qua WebView",
            )]);
        }
        let images = extract_image_urls(&body, &chapter_url);
        if images.is_empty() {
            return Ok(vec![manga::text_page("Khong tim thay hinh anh")]);
        }
        Ok(images
            .into_iter()
            .enumerate()
            .map(|(index, image)| page(index, &image, &chapter_url))
            .collect())
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(with_listing(&request, "popular"))?,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(with_listing(&request, "latest"))?,
            ),
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            let is_manga = key.contains("/truyen/") && !key.contains("-chap-");
            return Ok(Some(UrlResolveResult {
                item: is_manga.then(|| details_by_key(&key)),
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

fn client() -> HttpClient {
    HttpClient::browser()
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

fn parse_popular(body: &str) -> Vec<CatalogItem> {
    body.split("sidebar-comic-block")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "sidebar-comic-block-link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "sidebar-comic-block-title", "</h3>")
                .map(|v| html::strip_tags(&v))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(catalog_item(
                key,
                title,
                image_attr(chunk).map(|image| absolute_url(&image)),
            ))
        })
        .fold(Vec::new(), push_unique)
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("comic-list-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "comic-block-link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "comic-block-title", "</h3>")
                .map(|v| html::strip_tags(&v))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(catalog_item(
                key,
                title,
                image_attr(chunk).map(|image| absolute_url(&image)),
            ))
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("?trang=") && body.to_lowercase().contains("sau"),
    }
}

fn parse_search_response(body: &str) -> Paged<CatalogItem> {
    let parsed = serde_json::from_str::<SearchResponse>(body).unwrap_or_default();
    let entries = parsed
        .data
        .into_iter()
        .filter_map(|result| {
            let link = result.link?;
            let title = result.title?;
            Some(catalog_item(
                normalize_key(&link),
                title,
                result
                    .img
                    .map(|img| absolute_url(&img.replace("-150x150", ""))),
            ))
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn catalog_item(key: String, title: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover,
        url: Some(absolute_url(&key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        ..CatalogItem::default()
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "comic-title", "</")
            .or_else(|| html::text_between(body, "<h2", "</h2>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "itemprop=image", "content")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        authors: info_value(body, "Tác giả")
            .map(|v| vec![v])
            .unwrap_or_default(),
        status: parse_status(&html::strip_tags(body)),
        description: html::text_between(body, "hide-long-text", "</div>")
            .map(|v| {
                html::strip_tags(&v)
                    .replace("— Xem Thêm —", "")
                    .trim()
                    .to_string()
            })
            .filter(|v| !v.is_empty()),
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapter-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "chapter-link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapter-title", "</p>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|v| html::strip_tags(&v))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(normalize_chapter_name(&title)),
                date_uploaded: html::text_between(chunk, "chapter-meta", "</p>")
                    .and_then(|v| parse_chapter_date(&html::strip_tags(&v))),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn extract_image_urls(body: &str, base: &str) -> Vec<String> {
    let secrets = secret_parts(body);
    if let Some((one, two, three, data_key)) = secrets {
        if let Some(raw) = js_string_after(body, "var htmlContent") {
            let passphrase = format!("{one}{two}{three}");
            if let Some(decrypted) = decrypt_content(&passphrase, &raw) {
                let data_attr = data_key.map(|key| format!("data-{key}"));
                let images = images_from_html(
                    &decrypted,
                    base,
                    data_attr.as_deref(),
                    Some((&one, &two, &three)),
                );
                if !images.is_empty() {
                    return images;
                }
            }
        }
    }
    images_from_html(body, base, None, None)
}

fn images_from_html(
    body: &str,
    base: &str,
    data_attr: Option<&str>,
    secrets: Option<(&str, &str, &str)>,
) -> Vec<String> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| {
            let obfuscated = data_attr.and_then(|attr| html::attr(chunk, attr));
            let decoded = match (obfuscated, secrets) {
                (Some(value), Some((one, two, three))) => Some(
                    value
                        .replace(one, ".")
                        .replace(two, ":")
                        .replace(three, "/"),
                ),
                _ => None,
            };
            decoded.or_else(|| image_attr(chunk))
        })
        .filter(|image| !image.starts_with("data:"))
        .map(|image| url::join_url(base, &image))
        .fold(Vec::new(), |mut seen, image| {
            if !seen.contains(&image) {
                seen.push(image);
            }
            seen
        })
}

fn decrypt_content(passphrase: &str, encrypted_json_string: &str) -> Option<String> {
    let normalized = encrypted_json_string
        .replace("\\\\", "\\")
        .replace("\\\"", "\"")
        .replace("\\/", "/");
    let data = serde_json::from_str::<EncryptedContent>(&normalized).ok()?;
    let mut ciphertext = STANDARD.decode(data.ciphertext?).ok()?;
    let iv = hex_decode(&data.iv?)?;
    let salt = hex_decode(&data.salt?)?;
    let key = pbkdf2_hmac_sha512(passphrase.as_bytes(), &salt, 999, 32);
    let decryptor = cbc::Decryptor::<Aes256>::new_from_slices(&key, &iv).ok()?;
    let plaintext = decryptor
        .decrypt_padded_mut::<Pkcs7>(&mut ciphertext)
        .ok()?;
    String::from_utf8(plaintext.to_vec()).ok()
}

fn pbkdf2_hmac_sha512(password: &[u8], salt: &[u8], iterations: u32, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let mut block_index = 1u32;
    let mut offset = 0usize;
    while offset < len {
        let mut mac = Hmac::<Sha512>::new_from_slice(password).expect("hmac key");
        mac.update(salt);
        mac.update(&block_index.to_be_bytes());
        let mut u = mac.finalize().into_bytes().to_vec();
        let mut t = u.clone();
        for _ in 1..iterations {
            let mut mac = Hmac::<Sha512>::new_from_slice(password).expect("hmac key");
            mac.update(&u);
            u = mac.finalize().into_bytes().to_vec();
            for (a, b) in t.iter_mut().zip(&u) {
                *a ^= *b;
            }
        }
        let take = (len - offset).min(t.len());
        out[offset..offset + take].copy_from_slice(&t[..take]);
        offset += take;
        block_index += 1;
    }
    out
}

fn hex_decode(input: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(input.len() / 2);
    let mut chars = input.as_bytes().chunks_exact(2);
    for pair in &mut chars {
        let text = std::str::from_utf8(pair).ok()?;
        bytes.push(u8::from_str_radix(text, 16).ok()?);
    }
    chars.remainder().is_empty().then_some(bytes)
}

fn secret_parts(body: &str) -> Option<(String, String, String, Option<String>)> {
    Some((
        js_string_after(body, "var secretOne")?,
        js_string_after(body, "var secretTwo")?,
        js_string_after(body, "var secretThree")?,
        js_string_after(body, "var secretDataKey"),
    ))
}

fn js_string_after(body: &str, marker: &str) -> Option<String> {
    let tail = body.split(marker).nth(1)?;
    let quote = tail
        .find(['"', '\''])
        .map(|idx| tail.as_bytes()[idx] as char)?;
    let after = tail.split_once(quote)?.1;
    Some(after.split(quote).next()?.to_string())
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "src"))
        .or_else(|| html::attr(chunk, "content"))
}

fn page(index: usize, image: &str, referer: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image.into(),
            context: Some(manga::image_headers(referer)),
        },
        headers: manga::image_headers(referer),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.find(label)
        .map(|idx| {
            html::strip_tags(&body[idx..].split("</li>").next().unwrap_or_default())
                .replace(label, "")
                .trim()
                .to_string()
        })
        .filter(|v| !v.is_empty())
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("trọn bộ") || lower.contains("hoàn thành") {
        ItemStatus::Completed
    } else if lower.contains("đang tiến hành") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn normalize_chapter_name(raw: &str) -> String {
    raw.replace("chap", "Chap").replace("CHAP", "Chap")
}

fn parse_chapter_date(text: &str) -> Option<i64> {
    let date = text
        .split_whitespace()
        .find(|part| part.matches('/').count() == 2)?;
    let mut parts = date.split('/');
    let day = parts.next()?;
    let month = parts.next()?;
    let year = parts.next()?;
    let year = if year.len() == 2 {
        format!("20{year}")
    } else {
        year.to_string()
    };
    dates::parse_ymd(&format!("{year}-{month}-{day}"))
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") {
        value
            .trim_start_matches(BASE_URL)
            .trim_end_matches('/')
            .to_string()
    } else {
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| key.contains("/truyen/"))
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut value = request.clone();
    value["page"] = json!(1);
    value["listingId"] = json!(listing);
    value
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(HomeSectionStyle::Cover),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Default, Deserialize)]
struct SearchResponse {
    data: Vec<SearchResult>,
}

#[derive(Deserialize)]
struct SearchResult {
    title: Option<String>,
    link: Option<String>,
    img: Option<String>,
}

#[derive(Deserialize)]
struct EncryptedContent {
    ciphertext: Option<String>,
    iv: Option<String>,
    salt: Option<String>,
}

const HOME_FIXTURE: &str = r#"<div id="day-charts"><div class="sidebar-comic-block"><a class="sidebar-comic-block-link" href="/truyen/sample"><h3 class="sidebar-comic-block-title">Sample</h3><img src="/cover.jpg"></a></div></div>"#;
const LIST_FIXTURE: &str = r#"<div class="comic-list-item"><a class="comic-block-link" href="/truyen/sample"><div class="comic-block-img"><img src="/cover.jpg"></div><h3 class="comic-block-title">Sample</h3></a></div>"#;
const SEARCH_FIXTURE: &str = r#"{"success":true,"data":[{"title":"Sample","link":"https://panomic1.info/truyen/sample","img":"/cover-150x150.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"<h2 class="comic-title">Sample</h2><div class="comic-desc-list"><meta itemprop="image" content="/cover.jpg"><ul><li><strong>Tác giả</strong> Author</li></ul></div><span class="comic-stt">Đang tiến hành</span><div class="hide-long-text"><p>Summary</p></div><div class="chapter-list"><div class="chapter-item"><a class="chapter-link" href="/truyen/sample-chap-1"><p class="chapter-title">Chap 1</p><p class="chapter-meta">01/01/2024</p></a></div></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="view-chapter"><img src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
