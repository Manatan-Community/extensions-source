use aes::Aes128;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::{
    Decryptor,
    cipher::{BlockDecryptMut, KeyIvInit, block_padding::NoPadding},
};
use image::{DynamicImage, GenericImage, GenericImageView, ImageFormat, RgbaImage};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi, abi::ExtensionResult,
    export_manga_source, source::MangaSource, webview,
};
use manatan_shared::{
    html, manga,
    sdk::http::{Headers, HttpClient},
    url,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Cursor};

type Aes128CbcDec = Decryptor<Aes128>;

const SOURCE: Mangago = Mangago;
const BASE_URL: &str = "https://www.mangago.me";
const DOMAIN: &str = "mangago.me";

struct Mangago;

impl MangaSource for Mangago {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: vec![sample_item("sample")],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update_date"
        } else {
            "view"
        };
        let body = fetch_document(&format!("{BASE_URL}/genre/all/{page}/?f=1&o=1&sortby={sort}&e="), &request)?;
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key, &request)?],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            search_url(&request)
        } else {
            format!(
                "{BASE_URL}/r/l_search?name={}&page={}",
                url::query_escape(query),
                page(&request)
            )
        };
        Ok(parse_listing(&fetch_document(&target, &request)?))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/read-manga/sample".into());
        details_by_key(&key, &request)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/read-manga/sample".into());
        let body = fetch_document(&absolute_url(&key), &request)?;
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(vec![MangaPage {
                content: PageContent::Text { text: "Mangago fixture page".into() },
                ..MangaPage::default()
            }]);
        }
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/read-manga/sample".into());
        let chapter_url = absolute_url(&key);
        let body = fetch_document(&chapter_url, &request)?;
        parse_pages(&body, &chapter_url, &request)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let preferences = request.get("preferences").cloned().unwrap_or(Value::Null);
        let popular = self.list(json!({"page": 1, "listingId": "popular", "preferences": preferences.clone()}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest", "preferences": preferences}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Compact),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = request
            .get("imageBase64")
            .or_else(|| request.get("image_base64"))
            .and_then(Value::as_str)
        else {
            return Ok(ProcessedImage::default());
        };
        let extra = request
            .get("page")
            .and_then(|page| page.get("extra"))
            .cloned()
            .unwrap_or(Value::Null);
        let Some(key) = extra.get("mangagoKey").and_then(Value::as_str) else {
            return Ok(ProcessedImage {
                image_base64: input.into(),
                mime_type: request
                    .get("mimeType")
                    .or_else(|| request.get("mime_type"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                ..ProcessedImage::default()
            });
        };
        let cols = extra.get("mangagoCols").and_then(Value::as_u64).unwrap_or(0) as u32;
        Ok(ProcessedImage {
            image_base64: descramble_base64(input, key, cols).unwrap_or_else(|| input.to_string()),
            mime_type: Some("image/jpeg".into()),
            ..ProcessedImage::default()
        })
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
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key, &request).unwrap_or_else(|_| sample_item(&key))),
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

fn client(request: &Value) -> HttpClient {
    let mut client = HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback();
    if let Some(ua) = request
        .get("preferences")
        .and_then(|preferences| preferences.get("custom_user_agent"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        client = client.with_header("User-Agent", ua.trim());
    }
    client
}

fn fetch_document(target: &str, request: &Value) -> ExtensionResult<String> {
    let mut headers = Headers::new();
    headers.insert("Cookie".into(), "_m_superu=1".into());
    client(request)
        .get(target)
        .headers(headers)
        .browser_document()
        .send_text()
}

fn fetch_text(target: &str, request: &Value) -> ExtensionResult<String> {
    let mut headers = Headers::new();
    headers.insert("Cookie".into(), "_m_superu=1".into());
    client(request).get(target).headers(headers).send_text()
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let mut entries = Vec::new();
    for block in body.split("updatesli").chain(body.split("pic_list")).skip(1) {
        let Some(href) = html::attr_after(block, "thm-effect", "href")
            .or_else(|| html::attr_after(block, "<a", "href"))
        else {
            continue;
        };
        let title = html::attr_after(block, "thm-effect", "title")
            .or_else(|| html::attr_after(block, "<a", "title"))
            .unwrap_or_else(|| html::strip_tags(block));
        if title.trim().is_empty() {
            continue;
        }
        let cover = html::attr_after(block, "<img", "data-src").or_else(|| html::attr_after(block, "<img", "src"));
        entries.push(CatalogItem {
            key: key_path(&href),
            title: title.trim().into(),
            cover: cover.map(|value| url::join_url(BASE_URL, &value)),
            url: Some(absolute_url(&href)),
            content_rating: Some("adult".into()),
            status: ItemStatus::Unknown,
            ..CatalogItem::default()
        });
    }
    entries.dedup_by(|left, right| left.key == right.key);
    Paged {
        entries,
        has_next_page: body.contains("current+li") || body.contains("to-next") || body.contains("class=\"next\""),
    }
}

fn details_by_key(key: &str, request: &Value) -> ExtensionResult<CatalogItem> {
    let body = fetch_document(&absolute_url(key), request)?;
    Ok(parse_details(&body, key, remove_title_version(request)))
}

fn parse_details(body: &str, key: &str, clean_title: bool) -> CatalogItem {
    let title = html::text_between(body, "w-title", "</h1>")
        .or_else(|| html::text_between(body, "<h1", "</h1>"))
        .map(|value| clean_title_tag(&html::strip_tags(&value), clean_title))
        .unwrap_or_else(|| key.trim_matches('/').into());
    let info = body.split("id=\"information\"").nth(1).unwrap_or(body);
    let cover = html::attr_after(info, "<img", "src");
    let description = html::text_between(info, "manga_summary", "</div>")
        .map(|value| html::strip_tags(&value));
    let tags = text_links_after(info, "Genre").into_iter().chain(text_links_after(info, "genre")).collect();
    let authors = text_links_after(info, "Author");
    let status = if info.to_ascii_lowercase().contains("completed") {
        ItemStatus::Completed
    } else if info.to_ascii_lowercase().contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    };
    CatalogItem {
        key: key_path(key),
        title,
        cover: cover.map(|value| url::join_url(BASE_URL, &value)),
        url: Some(absolute_url(key)),
        authors,
        description,
        tags,
        content_rating: Some("adult".into()),
        status,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<tr")
        .filter(|chunk| chunk.contains("chico"))
        .enumerate()
        .filter_map(|(index, chunk)| {
            let href = html::attr_after(chunk, "chico", "href")?;
            let title = html::text_between(chunk, "chico", "</a>").map(|value| html::strip_tags(&value));
            Some(MangaChapter {
                key: key_path(&href),
                title,
                date_uploaded: None,
                source_order: Some(index as i32),
                url: Some(absolute_url(&href)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, chapter_url: &str, request: &Value) -> ExtensionResult<Vec<MangaPage>> {
    let images = encrypted_images(body, chapter_url, request)?;
    Ok(images
        .into_iter()
        .enumerate()
        .map(|(index, image)| {
            let mut extra = BTreeMap::new();
            let mut url = image.url;
            if let Some(key) = image.descramble_key {
                extra.insert("mangagoKey".into(), Value::String(key));
                extra.insert("mangagoCols".into(), Value::from(image.cols));
            }
            if url.contains('_') && url.starts_with("https://") {
                url = url.replacen("https://", "http://", 1);
            }
            MangaPage {
                content: PageContent::Url {
                    url,
                    context: Some(manga::image_headers(chapter_url)),
                },
                headers: manga::image_headers(chapter_url),
                extra,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect())
}

struct PageImage {
    url: String,
    descramble_key: Option<String>,
    cols: u32,
}

fn encrypted_images(body: &str, chapter_url: &str, request: &Value) -> ExtensionResult<Vec<PageImage>> {
    let encoded = script_data(body, "imgsrcs")
        .and_then(|script| imgsrcs_value(&script))
        .ok_or_else(|| abi::ExtensionError { message: "could not find imgsrcs".into() })?;
    let encrypted = STANDARD.decode(encoded).map_err(|error| abi::ExtensionError {
        message: format!("invalid imgsrcs base64: {error}"),
    })?;
    let chapter_js_url = script_src(body, "chapter.js")
        .map(|value| url::join_url(chapter_url, &value))
        .ok_or_else(|| abi::ExtensionError { message: "missing chapter.js".into() })?;
    let chapter_js = sojson_v4_decode(&fetch_text(&chapter_js_url, request)?)?;
    let key = hex_bytes(&find_hex_var(&chapter_js, "key")).ok_or_else(|| abi::ExtensionError {
        message: "missing AES key".into(),
    })?;
    let iv = hex_bytes(&find_hex_var(&chapter_js, "iv")).ok_or_else(|| abi::ExtensionError {
        message: "missing AES iv".into(),
    })?;
    let mut decrypted = Aes128CbcDec::new_from_slices(&key, &iv)
        .map_err(|error| abi::ExtensionError { message: format!("AES setup error: {error}") })?
        .decrypt_padded_vec_mut::<NoPadding>(&encrypted)
        .map_err(|error| abi::ExtensionError { message: format!("AES decrypt error: {error}") })?;
    while decrypted.last() == Some(&0) {
        decrypted.pop();
    }
    let list = String::from_utf8(decrypted).map_err(|error| abi::ExtensionError {
        message: format!("image list utf8 error: {error}"),
    })?;
    let image_list = unscramble_image_list(&list, &chapter_js);
    let cols = cols(&chapter_js);
    let urls = image_list
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let keys = descramble_keys(chapter_url, &chapter_js, &urls).unwrap_or_default();
    Ok(urls
        .into_iter()
        .enumerate()
        .map(|(index, raw)| {
            let descramble_key = raw
                .contains("cspiclink")
                .then(|| keys.get(index).cloned().unwrap_or_default())
                .filter(|value| !value.is_empty());
            PageImage {
                url: raw,
                descramble_key,
                cols,
            }
        })
        .collect())
}

fn descramble_keys(chapter_url: &str, chapter_js: &str, urls: &[String]) -> Option<Vec<String>> {
    let body = chapter_js
        .split("var renImg = function(img,width,height,id){")
        .nth(1)?
        .split("key = key.split(")
        .next()?
        .lines()
        .filter(|line| {
            !["jQuery", "document", "getContext", "toDataURL", "getImageData", "width", "height"]
                .iter()
                .any(|needle| line.contains(needle))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .replace("img.src", "url");
    let urls_json = serde_json::to_string(urls).ok()?;
    let script = format!(
        r#"
        const urls = {urls_json};
        function replacePos(strObj, pos, replacetext) {{
            return strObj.substr(0, pos) + replacetext + strObj.substring(pos + 1, strObj.length);
        }}
        function getDescramblingKey(url) {{ {body}; return key; }}
        JSON.stringify(urls.map((url) => url.indexOf("cspiclink") >= 0 ? String(getDescramblingKey(url) || "") : ""));
        "#
    );
    webview::extract_text(webview::ExtractRequest::new(chapter_url, script).timeout_ms(20_000))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
}

fn descramble_base64(input: &str, key: &str, cols: u32) -> Option<String> {
    if cols == 0 {
        return None;
    }
    let bytes = STANDARD.decode(input).ok()?;
    let source = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (width, height) = source.dimensions();
    let unit_width = width / cols;
    let unit_height = height / cols;
    if unit_width == 0 || unit_height == 0 {
        return None;
    }
    let mut target = RgbaImage::new(width, height);
    let keys = key
        .split('a')
        .map(|value| value.parse::<u32>().unwrap_or(0))
        .collect::<Vec<_>>();
    for index in 0..(cols * cols) {
        let key_value = keys.get(index as usize).copied().unwrap_or(0);
        let dx = (key_value % cols) * unit_width;
        let dy = (key_value / cols) * unit_height;
        let sx = (index % cols) * unit_width;
        let sy = (index / cols) * unit_height;
        let view = source.view(sx, sy, unit_width, unit_height).to_image();
        let _ = target.copy_from(&view, dx, dy);
    }
    let mut out = Vec::new();
    DynamicImage::ImageRgba8(target)
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Jpeg)
        .ok()?;
    Some(STANDARD.encode(out))
}

fn search_url(request: &Value) -> String {
    let page = page(request);
    let genres = filter_strings(request, "genres");
    let excluded = filter_strings(request, "exclude_genres");
    let path = if genres.is_empty() { "all".into() } else { genres.join(",") };
    let completed = filter_bool(request, "completed", true);
    let ongoing = filter_bool(request, "ongoing", true);
    let sort = filter_string(request, "sortby", "view");
    format!(
        "{BASE_URL}/genre/{}/{page}/?f={}&o={}&sortby={}&e={}",
        url::query_escape(&path),
        if completed { 1 } else { 0 },
        if ongoing { 1 } else { 0 },
        url::query_escape(&sort),
        url::query_escape(&excluded.join(","))
    )
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_string(request: &Value, id: &str, default: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn filter_bool(request: &Value, id: &str, default: bool) -> bool {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn filter_strings(request: &Value, id: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

fn remove_title_version(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("remove_title_version"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn clean_title_tag(title: &str, enabled: bool) -> String {
    if !enabled {
        return title.trim().into();
    }
    let mut out = title.trim().to_string();
    loop {
        let trimmed = out.trim();
        if let Some(rest) = trimmed.strip_prefix('(').and_then(|value| value.split_once(')').map(|(_, rest)| rest)) {
            out = rest.trim().to_string();
        } else {
            break;
        }
    }
    out
}

fn script_data(body: &str, marker: &str) -> Option<String> {
    body.split("<script")
        .find(|chunk| chunk.contains(marker))
        .and_then(|chunk| chunk.split('>').nth(1))
        .and_then(|chunk| chunk.split("</script>").next())
        .map(ToOwned::to_owned)
}

fn script_src(body: &str, marker: &str) -> Option<String> {
    body.split("<script").find_map(|chunk| {
        chunk.contains(marker).then(|| html::attr(chunk, "src")).flatten()
    })
}

fn imgsrcs_value(script: &str) -> Option<String> {
    script
        .split("imgsrcs")
        .nth(1)?
        .split(['"', '\''])
        .nth(1)
        .map(ToOwned::to_owned)
}

fn find_hex_var(input: &str, variable: &str) -> String {
    let needle = format!("var {variable}");
    input
        .split(&needle)
        .nth(1)
        .and_then(|rest| rest.split("CryptoJS.enc.Hex.parse").nth(1))
        .and_then(|rest| rest.split('"').nth(1))
        .unwrap_or_default()
        .to_string()
}

fn hex_bytes(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

fn sojson_v4_decode(input: &str) -> ExtensionResult<String> {
    if !input.starts_with("['sojson.v4']") || input.len() < 300 {
        return Ok(input.to_string());
    }
    let end = input.len().saturating_sub(59);
    let slice = input.get(240..end).unwrap_or_default();
    let decoded = slice
        .split(|ch: char| ch.is_ascii_alphabetic())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u32>().ok().and_then(char::from_u32))
        .collect::<String>();
    if decoded.is_empty() {
        Err(abi::ExtensionError { message: "sojson decode failed".into() })
    } else {
        Ok(decoded)
    }
}

fn unscramble_image_list(image_list: &str, js: &str) -> String {
    let locations = js
        .split("str.charAt(")
        .skip(1)
        .filter_map(|rest| rest.split(')').next()?.trim().parse::<usize>().ok())
        .collect::<Vec<_>>();
    let mut out = image_list.to_string();
    let mut keys = Vec::new();
    for location in &locations {
        let Some(ch) = out.chars().nth(*location) else {
            return out;
        };
        let Some(value) = ch.to_digit(10).map(|value| value as usize) else {
            return out;
        };
        keys.push(value);
    }
    for (offset, location) in locations.iter().enumerate() {
        let adjusted = location.saturating_sub(offset);
        if adjusted < out.len() {
            out.remove(adjusted);
        }
    }
    for key in keys.into_iter().rev() {
        let mut chars = out.chars().collect::<Vec<_>>();
        for index in (key..chars.len()).rev() {
            if index % 2 != 0 && index >= key {
                chars.swap(index - key, index);
            }
        }
        out = chars.into_iter().collect();
    }
    out
}

fn cols(js: &str) -> u32 {
    js.split("widthnum")
        .nth(1)
        .and_then(|rest| rest.split('=').nth(2).or_else(|| rest.split('=').nth(1)))
        .and_then(|rest| {
            rest.chars()
                .filter(|ch| ch.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok()
        })
        .unwrap_or(0)
}

fn text_links_after(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("</li>")
        .next()
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>").or_else(|| Some(html::strip_tags(chunk))))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn key_from_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.starts_with(BASE_URL) || trimmed.contains(DOMAIN) {
        let without_base = trimmed
            .split(DOMAIN)
            .nth(1)
            .unwrap_or(trimmed)
            .trim_start_matches('/');
        Some(format!("/{without_base}"))
    } else if trimmed.starts_with("/read-manga/") || trimmed.starts_with("/manga/") {
        Some(trimmed.into())
    } else {
        None
    }
}

fn key_path(input: &str) -> String {
    if input.starts_with("http") {
        input
            .split(DOMAIN)
            .nth(1)
            .map(|value| format!("/{value}"))
            .unwrap_or_else(|| input.to_string())
    } else {
        format!("/{}", input.trim_start_matches('/'))
    }
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn sample_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key_path(key),
        title: "Mangago Sample".into(),
        url: Some(absolute_url(key)),
        content_rating: Some("adult".into()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

export_manga_source!(SOURCE);
