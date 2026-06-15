use aes::Aes256;
use base64::{Engine, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use hmac::{Hmac, Mac};
use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, url, vi_html as vh};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Sha512;

const SOURCE: TeamLanhLung = TeamLanhLung;
const BASE_URL: &str = "https://lunghihi.icu";
const KEY_PART_1: &str = "DA9TqD";
const KEY_PART_2: &str = "QqNm2h";
const KEY_PART_3: &str = "wSUU8q";

struct TeamLanhLung;

impl MangaSource for TeamLanhLung {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = vh::page_number(&request);
        if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            return Ok(Paged {
                entries: parse_popular(&vh::fetch_document(
                    BASE_URL,
                    &format!("{BASE_URL}/xem-nhieu-nhat/"),
                    LIST_FIXTURE,
                )),
                has_next_page: false,
            });
        }
        let target = if page > 1 {
            format!("{BASE_URL}/page/{page}/")
        } else {
            BASE_URL.to_string()
        };
        Ok(parse_latest(&vh::fetch_document(
            BASE_URL,
            &target,
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = vh::query(&request);
        if let Some(key) = vh::key_from_url(BASE_URL, &query, "/truyen-tranh/") {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            let body = vh::browser_client(BASE_URL)
                .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
                .header("Content-Type", "application/x-www-form-urlencoded")
                .form(&[("action", "searchtax"), ("keyword", query.as_str())])
                .send_text()
                .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
            return Ok(parse_search_json(&body));
        }
        if let Some(genre) = vh::filter(&request, "genre") {
            let body = vh::fetch_document(BASE_URL, &format!("{BASE_URL}/{genre}/"), LIST_FIXTURE);
            return Ok(Paged {
                entries: parse_popular(&body),
                has_next_page: false,
            });
        }
        self.list(json!({"page": vh::page_number(&request), "listingId": "latest"}))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(parse_chapters(&vh::fetch_document(
            BASE_URL,
            &vh::absolute_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen-tranh/sample-chap-1".into());
        let chapter_url = vh::absolute_url(BASE_URL, &key);
        let body = vh::fetch_document(BASE_URL, &chapter_url, PAGES_FIXTURE);
        if body.contains("post-password-form") || body.contains("post_password") {
            return Ok(vec![vh::text_page(
                "Vui long nhap mat khau chuong nay qua WebView",
            )]);
        }
        let images = extract_image_urls(&body);
        Ok(if images.is_empty() {
            vec![vh::text_page("Khong tim thay hinh anh")]
        } else {
            images
                .iter()
                .enumerate()
                .map(|(i, image)| vh::image_page(i, image, &chapter_url))
                .collect()
        })
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            vh::home_section(
                "popular",
                "Popular",
                self.list(json!({"page": 1, "listingId": "popular"})),
            )?,
            vh::home_section(
                "latest",
                "Latest",
                self.list(json!({"page": 1, "listingId": "latest"})),
            )?,
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| vh::absolute_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| vh::absolute_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = vh::key_from_url(BASE_URL, input, "/truyen-tranh/") {
            let is_chapter = key.contains("-chap-") || key.contains("/chap-");
            return Ok(Some(UrlResolveResult {
                item: (!is_chapter).then(|| details_by_key(&key)),
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

fn parse_popular(body: &str) -> Vec<CatalogItem> {
    body.split("position-relative")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/truyen-tranh/") {
                return None;
            }
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, "super-title", "</p>")
                .map(|v| html::strip_tags(&v))
                .or_else(|| vh::title_from(chunk))
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(vh::catalog_item(
                BASE_URL,
                key,
                title,
                vh::image_attr(chunk).map(strip_small_thumb),
                "adult",
            ))
        })
        .fold(Vec::new(), vh::push_unique)
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("comic-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/truyen-tranh/") {
                return None;
            }
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, "comic-title", "</h3>")
                .map(|v| html::strip_tags(&v))
                .or_else(|| vh::title_from(chunk))
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(vh::catalog_item(
                BASE_URL,
                key,
                title,
                vh::image_attr(chunk).map(strip_small_thumb),
                "adult",
            ))
        })
        .fold(Vec::new(), vh::push_unique);
    Paged {
        entries,
        has_next_page: body.contains("li.next") && !body.contains("next disabled"),
    }
}

fn parse_search_json(body: &str) -> Paged<CatalogItem> {
    let parsed = serde_json::from_str::<SearchResponse>(body).unwrap_or_default();
    let entries = if parsed.success {
        parsed
            .data
            .into_iter()
            .filter_map(|entry| {
                let link = entry.link?;
                if !link.contains("/truyen-tranh/") {
                    return None;
                }
                let key = vh::normalize_key(BASE_URL, &link);
                Some(vh::catalog_item(
                    BASE_URL,
                    key,
                    entry.title?,
                    entry.img.map(strip_small_thumb),
                    "adult",
                ))
            })
            .fold(Vec::new(), vh::push_unique)
    } else {
        Vec::new()
    };
    Paged {
        entries,
        has_next_page: false,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(
        &vh::fetch_document(BASE_URL, &vh::absolute_url(BASE_URL, key), DETAILS_FIXTURE),
        key,
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: vh::normalize_key(BASE_URL, key),
        title: html::text_between(body, "info-title", "</h2>")
            .map(|v| html::strip_tags(&v))
            .or_else(|| html::text_between(body, "<h1", "</h1>").map(|v| html::strip_tags(&v)))
            .unwrap_or_else(|| "Manga".into()),
        cover: vh::image_attr(body)
            .map(strip_small_thumb)
            .map(|v| vh::absolute_url(BASE_URL, &v)),
        authors: info_value(body, "Tác giả")
            .into_iter()
            .filter(|v| v != "Đang cập nhật" && v != "Không có")
            .collect(),
        tags: body
            .split("/the-loai/")
            .skip(1)
            .filter_map(|c| html::text_between(c, ">", "</a>").map(|v| html::strip_tags(&v)))
            .collect(),
        description: html::text_between(body, "hide-long-text", "</div>")
            .map(|v| {
                html::strip_tags(&v)
                    .replace("— Xem Thêm —", "")
                    .trim()
                    .trim_matches('"')
                    .to_string()
            })
            .filter(|v| !v.is_empty()),
        status: info_value(body, "comic-stt")
            .map(|v| vh::status_from_vi(&v))
            .unwrap_or(ItemStatus::Unknown),
        url: Some(vh::absolute_url(BASE_URL, key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.find(label)
        .map(|idx| {
            html::strip_tags(body[idx..].split("</").next().unwrap_or_default())
                .replace(label, "")
                .replace(':', "")
                .trim()
                .to_string()
        })
        .filter(|v| !v.is_empty())
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<tr")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "text-capitalize", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = vh::normalize_key(BASE_URL, &href);
            let mut title = vh::title_from(chunk).unwrap_or_else(|| "Chapter".into());
            if title.to_lowercase().contains("lock") {
                title = format!("Locked {title}");
            }
            Some(MangaChapter {
                key: key.clone(),
                title: Some(normalize_chapter_name(&title)),
                date_uploaded: vh::parse_dd_mm_yyyy(chunk),
                url: Some(vh::absolute_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn extract_image_urls(body: &str) -> Vec<String> {
    html_content_var(body)
        .and_then(|encrypted| decrypt_content(&encrypted))
        .map(|decrypted| images_from_html(&decrypted))
        .filter(|images| !images.is_empty())
        .unwrap_or_else(|| images_from_html(body))
}

fn html_content_var(body: &str) -> Option<String> {
    let tail = body.split("var htmlContent").nth(1)?;
    let first_quote = tail.find('"')?;
    let after = &tail[first_quote + 1..];
    let end = after.find("\";").or_else(|| after.find('"'))?;
    Some(after[..end].replace("\\\"", "\"").replace("\\/", "/"))
}

fn decrypt_content(encrypted_json: &str) -> Option<String> {
    let data = serde_json::from_str::<EncryptedContent>(encrypted_json).ok()?;
    let passphrase = format!("{KEY_PART_1}{KEY_PART_2}{KEY_PART_3}");
    let mut ciphertext = STANDARD.decode(data.ciphertext).ok()?;
    let iv = hex_decode(&data.iv)?;
    let salt = hex_decode(&data.salt)?;
    let key = pbkdf2_hmac_sha512(passphrase.as_bytes(), &salt, 999, 32);
    let decryptor = cbc::Decryptor::<Aes256>::new_from_slices(&key, &iv).ok()?;
    let plaintext = decryptor
        .decrypt_padded_mut::<Pkcs7>(&mut ciphertext)
        .ok()?;
    String::from_utf8(plaintext.to_vec()).ok()
}

fn images_from_html(body: &str) -> Vec<String> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| {
            let image = html::attr(chunk, "data-da9tqd")
                .filter(|value| !value.is_empty() && value != "loaded" && value != "stored")
                .map(deobfuscate_url)
                .or_else(|| vh::image_attr(chunk));
            image.map(|value| vh::absolute_url(BASE_URL, &value))
        })
        .collect()
}

fn deobfuscate_url(value: String) -> String {
    value
        .replace(KEY_PART_1, ".")
        .replace(KEY_PART_2, ":")
        .replace(KEY_PART_3, "/")
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
    let mut chunks = input.as_bytes().chunks_exact(2);
    for pair in &mut chunks {
        let text = std::str::from_utf8(pair).ok()?;
        bytes.push(u8::from_str_radix(text, 16).ok()?);
    }
    chunks.remainder().is_empty().then_some(bytes)
}

fn normalize_chapter_name(raw: &str) -> String {
    raw.replace("chap", "CHAP")
        .replace("Chap", "CHAP")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_small_thumb(value: String) -> String {
    value.replace("-150x150.", ".")
}

#[derive(Default, Deserialize)]
struct SearchResponse {
    data: Vec<SearchEntry>,
    success: bool,
}

#[derive(Deserialize)]
struct SearchEntry {
    img: Option<String>,
    link: Option<String>,
    title: Option<String>,
}

#[derive(Deserialize)]
struct EncryptedContent {
    ciphertext: String,
    iv: String,
    salt: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="comic-item"><a href="/truyen-tranh/sample"><h3 class="comic-title">Sample</h3><img src="/cover-150x150.jpg"></a></div>"#;
const SEARCH_FIXTURE: &str = r#"{"success":true,"data":[{"title":"Sample","link":"https://lunghihi.icu/truyen-tranh/sample","img":"/cover.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"<h2 class="info-title">Sample</h2><img class="img-thumbnail" src="/cover.jpg"><div class="chapter-table"><table><tbody><tr><td><a class="text-capitalize" href="/truyen-tranh/sample-chap-1">Chap 1</a></td><td>01/01/2024</td></tr></tbody></table></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="view-chapter"><img src="/page1.jpg"></div>"#;
