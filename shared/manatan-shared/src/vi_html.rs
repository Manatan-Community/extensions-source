use crate::{
    dates, html,
    sdk::{
        CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
        PageContent, Paged, abi::ExtensionResult, http::HttpClient,
    },
    url,
};
use aes::Aes256;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha512;

pub fn browser_client(base_url: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", base_url.trim_end_matches('/')))
        .with_cookies_for(base_url)
        .with_webview_challenge_fallback()
}

pub fn fetch_document(base_url: &str, target: &str, fixture: &str) -> String {
    browser_client(base_url)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

pub fn page_number(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

pub fn query(request: &Value) -> String {
    request
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn filter<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub fn normalize_key(base_url: &str, value: &str) -> String {
    let value = value
        .trim()
        .trim_start_matches(base_url.trim_end_matches('/'))
        .trim_end_matches('/');
    format!("/{}", value.trim_start_matches('/'))
}

pub fn key_from_url(base_url: &str, input: &str, marker: &str) -> Option<String> {
    input
        .starts_with(base_url)
        .then(|| normalize_key(base_url, input))
        .filter(|key| key.contains(marker))
}

pub fn absolute_url(base_url: &str, value: &str) -> String {
    url::join_url(base_url, value)
}

pub fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-original"))
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "src"))
        .or_else(|| html::attr(chunk, "content"))
        .filter(|value| !value.starts_with("data:"))
}

pub fn image_from_style(style: &str) -> Option<String> {
    style
        .split("url(")
        .nth(1)
        .and_then(|tail| tail.split(')').next())
        .map(|value| value.trim_matches(['"', '\'']).to_string())
        .filter(|value| !value.is_empty())
}

pub fn catalog_item(
    base_url: &str,
    key: String,
    title: String,
    cover: Option<String>,
    rating: &str,
) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|image| absolute_url(base_url, &image)),
        url: Some(absolute_url(base_url, &key)),
        language: Some("vi".into()),
        content_rating: Some(rating.into()),
        ..CatalogItem::default()
    }
}

pub fn image_page(index: usize, image: &str, referer: &str) -> MangaPage {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    MangaPage {
        content: PageContent::Url {
            url: image.to_string(),
            context: Some(headers.clone()),
        },
        headers,
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

pub fn image_pages(images: Vec<String>, referer: &str) -> Vec<MangaPage> {
    images
        .iter()
        .enumerate()
        .map(|(index, image)| image_page(index, image, referer))
        .collect()
}

pub fn text_page(text: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Text { text: text.into() },
        description: Some("Text page".into()),
        ..MangaPage::default()
    }
}

pub fn home_section(
    id: &str,
    title: &str,
    page: ExtensionResult<Paged<CatalogItem>>,
) -> ExtensionResult<HomeSection<CatalogItem>> {
    let page = page?;
    Ok(HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(HomeSectionStyle::Cover),
        has_more: page.has_next_page,
        entries: page.entries,
        ..HomeSection::default()
    })
}

pub fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

pub fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

pub fn status_from_vi(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("hoàn thành") || lower.contains("trọn bộ") || lower.contains("đã full")
    {
        ItemStatus::Completed
    } else if lower.contains("đang tiến hành")
        || lower.contains("đang cập nhật")
        || lower.contains("đang ra")
    {
        ItemStatus::Ongoing
    } else if lower.contains("tạm ngưng") || lower.contains("tạm dừng") || lower.contains("hiatus")
    {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

pub fn parse_dd_mm_yyyy(text: &str) -> Option<i64> {
    let date = text
        .split_whitespace()
        .find(|part| part.matches('/').count() == 2 || part.matches('-').count() == 2)?;
    let sep = if date.contains('/') { '/' } else { '-' };
    let mut parts = date.split(sep);
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

pub fn parse_dd_mm_yy(text: &str) -> Option<i64> {
    let date = text
        .split_whitespace()
        .find(|part| part.matches('/').count() == 2 || part.matches('-').count() == 2)?;
    let sep = if date.contains('/') { '/' } else { '-' };
    let mut parts = date.split(sep);
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let year = parts.next()?.parse::<u32>().ok()?;
    let year = if year < 100 { 2000 + year } else { year };
    dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

pub fn parse_vi_date(text: &str) -> Option<i64> {
    parse_dd_mm_yyyy(text)
        .or_else(|| parse_dd_mm_yy(text))
        .or_else(|| dates::parse_fixture_date(text))
}

pub fn title_from(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<a", "title")
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .or_else(|| html::text_between(chunk, "<h1", "</h1>"))
        .or_else(|| html::text_between(chunk, "<h2", "</h2>"))
        .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
        .or_else(|| html::text_between(chunk, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

pub fn has_next(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("rel=\"next\"")
        || lower.contains("page-numbers next")
        || lower.contains("pagination") && (lower.contains("&raquo;") || lower.contains("»"))
        || lower.contains("li.next:not")
        || lower.contains("title=\"next\"")
}

pub fn looks_like_image(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    !lower.starts_with("data:")
        && !lower.ends_with("/loading.webp")
        && !lower.ends_with("/page_logo.png")
        && [".jpg", ".jpeg", ".png", ".webp", ".avif", ".gif"]
            .iter()
            .any(|ext| lower.contains(ext))
}

pub fn collect_image_urls(base_url: &str, body: &str) -> Vec<String> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| looks_like_image(image))
        .fold(Vec::new(), |mut seen, image| {
            let image = absolute_url(base_url, &image);
            if !seen.contains(&image) {
                seen.push(image);
            }
            seen
        })
}

pub fn strip_small_thumbnail(input: String) -> String {
    for ext in [".jpg", ".jpeg", ".png", ".webp", ".avif"] {
        let needle = format!("-150x150{ext}");
        if input.ends_with(&needle) {
            return input.trim_end_matches(&needle).to_string() + ext;
        }
    }
    input
}

pub fn selected_array(request: &Value, id: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub fn decrypt_cryptojs_aes_sha512(
    passphrase: &str,
    encrypted_json_string: &str,
) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct EncryptedData {
        ciphertext: String,
        iv: String,
        salt: String,
    }

    let data = serde_json::from_str::<EncryptedData>(encrypted_json_string).ok()?;
    let mut ciphertext = STANDARD.decode(data.ciphertext).ok()?;
    let iv = decode_hex(&data.iv)?;
    let salt = decode_hex(&data.salt)?;
    let key = pbkdf2_hmac_sha512(passphrase.as_bytes(), &salt, 999, 32);
    let decryptor = cbc::Decryptor::<Aes256>::new_from_slices(&key, &iv).ok()?;
    let plaintext = decryptor
        .decrypt_padded_mut::<Pkcs7>(&mut ciphertext)
        .ok()?;
    String::from_utf8(plaintext.to_vec()).ok()
}

pub fn decode_hex(input: &str) -> Option<Vec<u8>> {
    if input.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(input.len() / 2);
    for pair in input.as_bytes().chunks(2) {
        let text = std::str::from_utf8(pair).ok()?;
        bytes.push(u8::from_str_radix(text, 16).ok()?);
    }
    Some(bytes)
}

fn pbkdf2_hmac_sha512(password: &[u8], salt: &[u8], iterations: u32, len: usize) -> Vec<u8> {
    let hlen = 64usize;
    let blocks = len.div_ceil(hlen);
    let mut output = Vec::with_capacity(blocks * hlen);
    for block_index in 1..=blocks {
        let mut mac = Hmac::<Sha512>::new_from_slice(password).expect("hmac key");
        mac.update(salt);
        mac.update(&(block_index as u32).to_be_bytes());
        let mut u = mac.finalize().into_bytes().to_vec();
        let mut t = u.clone();
        for _ in 1..iterations {
            let mut mac = Hmac::<Sha512>::new_from_slice(password).expect("hmac key");
            mac.update(&u);
            u = mac.finalize().into_bytes().to_vec();
            for (left, right) in t.iter_mut().zip(&u) {
                *left ^= *right;
            }
        }
        output.extend_from_slice(&t);
    }
    output.truncate(len);
    output
}
