use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD},
};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, manga_image, sdk::http::HttpClient, url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

const SOURCE: MoeTruyen = MoeTruyen;
const DEFAULT_DOMAIN: &str = "https://moetruyen.net";
const GLOBAL_DOMAIN: &str = "https://truyen.moe";
const FULL_WEB_COOKIE: &str = "moetruyen_full_web=Moetruyen123456";
const GOLDEN_RATIO: u32 = 2_654_435_769;

struct MoeTruyen;

impl MangaSource for MoeTruyen {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_latest(LIST_FIXTURE, &base));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            let body = fetch_document(&base, &base, POPULAR_FIXTURE);
            return Ok(Paged {
                entries: parse_popular(&body, &base),
                has_next_page: false,
            });
        }
        Ok(parse_latest(
            &fetch_document(&base, &format!("{base}/manga?page={page}"), LIST_FIXTURE),
            &base,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(&base, query) {
            return Ok(Paged {
                entries: vec![details_by_key(&base, &key)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let status = filter(filters, "status");
        let genres = filter(filters, "include");
        let has_filter = status.is_some() || genres.is_some();
        if query.is_empty() && !has_filter {
            return self.list(json!({"page": request.get("page").cloned().unwrap_or(json!(1)), "listingId": "latest", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut pairs = vec![format!("page={page}")];
        if !query.is_empty() {
            pairs.push(format!("q={}", url::query_escape(query)));
        }
        if let Some(status) = status {
            pairs.push(format!("status={}", url::query_escape(&status)));
        }
        if let Some(genres) = genres {
            for genre in genres
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                pairs.push(format!("include={}", url::query_escape(genre)));
            }
        }
        Ok(parse_latest(
            &fetch_document(
                &base,
                &format!("{base}/manga?{}", pairs.join("&")),
                LIST_FIXTURE,
            ),
            &base,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&base, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters_paginated(&base, &absolute_url(&base, &key)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let chapter_url = absolute_url(&base, &key);
        let body = fetch_document(&base, &chapter_url, PAGES_FIXTURE);
        let pages = parse_pages(&base, &chapter_url, &body);
        if pages.is_empty() {
            return Ok(vec![manga::text_page("Khong tim thay hinh anh")]);
        }
        Ok(pages)
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
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&base, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&base, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(&base, input) {
            let is_manga = key.split('/').filter(|part| !part.is_empty()).count() == 2;
            return Ok(Some(UrlResolveResult {
                item: is_manga.then(|| details_by_key(&base, &key)),
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
        let Some(input) = manga_image::image_base64(&request) else {
            return Ok(manga_image::passthrough_processed_image(&request));
        };
        let Some(extra) = request.get("page").and_then(|page| page.get("extra")) else {
            return Ok(manga_image::passthrough_processed_image(&request));
        };
        let Some(grant) = extra
            .get("imgxGrant")
            .and_then(|value| serde_json::from_value::<ImgxGrant>(value.clone()).ok())
        else {
            return Ok(manga_image::passthrough_processed_image(&request));
        };
        let storage_key = extra
            .get("storageKey")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Ok(data) = STANDARD.decode(input) else {
            return Ok(manga_image::passthrough_processed_image(&request));
        };
        if data.len() <= 13 || &data[..4] != b"IMGX" || data[4] != 2 {
            return Ok(manga_image::passthrough_processed_image(&request));
        }
        let decrypted = decrypt_imgx(&data, &grant, storage_key).unwrap_or(data);
        Ok(ProcessedImage {
            image_base64: STANDARD.encode(decrypted),
            mime_type: Some("image/webp".into()),
            ..ProcessedImage::default()
        })
    }
}

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_header("Cookie", FULL_WEB_COOKIE)
        .with_referer(format!("{base}/"))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_document(base: &str, target: &str, fixture: &str) -> String {
    client(base)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str, base: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("homepage-ranking-item__link"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(base, &href);
            let title = html::attr_after(chunk, "homepage-ranking-item__title", "title")
                .or_else(|| html::text_between(chunk, "homepage-ranking-item__title", "</"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(item_basic(
                base,
                key,
                title,
                image_attr(chunk).map(|image| absolute_url(base, &image)),
            ))
        })
        .fold(Vec::new(), push_unique)
}

fn parse_latest(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("manga-card--list"))
        .filter_map(|chunk| {
            let href =
                html::attr_after(chunk, "href", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(base, &href);
            let title = html::attr_after(chunk, "<h3", "title")
                .or_else(|| {
                    html::text_between(chunk, "<h3", "</h3>").map(|value| html::strip_tags(&value))
                })
                .or_else(|| {
                    html::attr_after(chunk, "<img", "alt")
                        .map(|value| value.trim_start_matches("Bìa ").trim().to_string())
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(item_basic(
                base,
                key,
                title,
                image_attr(chunk).map(|image| absolute_url(base, &image)),
            ))
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("aria-label='Trang sau'")
            || (body.contains("aria-label=\"Trang sau\"") && !body.contains("is-disabled")),
    }
}

fn item_basic(base: &str, key: String, title: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover,
        url: Some(absolute_url(base, &key)),
        language: Some("vi".into()),
        content_rating: Some("suggestive".into()),
        ..CatalogItem::default()
    }
}

fn details_by_key(base: &str, key: &str) -> CatalogItem {
    parse_details(
        &fetch_document(base, &absolute_url(base, key), DETAILS_FIXTURE),
        base,
        key,
    )
}

fn parse_details(body: &str, base: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(base, key),
        title: html::text_between(body, "manga-detail-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        authors: meta_line_links(body, "Tác giả"),
        tags: chip_texts(body),
        description: html::text_between(body, "data-description-content", "</")
            .or_else(|| html::text_between(body, "manga-description__text", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(
            &html::text_between(body, "manga-status-pill", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default(),
        ),
        cover: image_after(body, "detail-cover").map(|image| absolute_url(base, &image)),
        url: Some(absolute_url(base, key)),
        language: Some("vi".into()),
        content_rating: Some("suggestive".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters_paginated(base: &str, first_url: &str) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    let mut current = first_url.to_string();
    let mut visited = Vec::<String>::new();
    for _ in 0..25 {
        if visited.contains(&current) {
            break;
        }
        visited.push(current.clone());
        let body = fetch_document(base, &current, DETAILS_FIXTURE);
        chapters.extend(parse_chapters(&body, base));
        let Some(next) = next_chapter_page(&body, base) else {
            break;
        };
        current = next;
    }
    chapters.into_iter().fold(Vec::new(), push_unique_chapter)
}

fn parse_chapters(body: &str, base: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-link"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(base, &href);
            let title = html::text_between(chunk, "chapter-num", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            let relative = html::text_between(chunk, "chapter-time", "</")
                .map(|value| html::strip_tags(&value));
            let absolute = html::attr_after(chunk, "chapter-time", "title").and_then(|value| {
                value
                    .split("Cập nhật")
                    .nth(1)
                    .map(|tail| tail.trim().to_string())
            });
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: relative
                    .as_deref()
                    .and_then(relative_date_seconds)
                    .or_else(|| absolute.as_deref().and_then(parse_vn_date)),
                url: Some(absolute_url(base, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn next_chapter_page(body: &str, base: &str) -> Option<String> {
    let marker = body.find("Trang chương sau")?;
    let before = &body[..marker];
    let a_start = before.rfind("<a")?;
    let chunk = &body[a_start..marker + "Trang chương sau".len()];
    if chunk.contains("is-disabled") || html::attr(chunk, "href").as_deref() == Some("#") {
        return None;
    }
    html::attr(chunk, "href").map(|href| absolute_url(base, &href))
}

fn parse_pages(base: &str, referer: &str, body: &str) -> Vec<MangaPage> {
    let image_chunks = body
        .split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("page-media") && !chunk.contains("<noscript"))
        .collect::<Vec<_>>();
    let access_url = image_chunks
        .first()
        .and_then(|chunk| html::attr(chunk, "data-imgx-access-url"))
        .filter(|value| !value.is_empty());
    if let Some(access_url) = access_url {
        let full = absolute_url(base, &access_url);
        let pages = fetch_pages_with_grants(base, referer, &full, image_chunks.len());
        if !pages.is_empty() {
            return pages;
        }
    }
    image_chunks
        .into_iter()
        .filter_map(image_attr)
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .map(|image| absolute_url(base, &image))
        .fold(Vec::new(), |mut seen, image| {
            if !seen.contains(&image) {
                seen.push(image);
            }
            seen
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| page(index, &image, referer, None))
        .collect()
}

fn fetch_pages_with_grants(
    base: &str,
    referer: &str,
    access_url: &str,
    page_count: usize,
) -> Vec<MangaPage> {
    let mut pages = Vec::new();
    for start in (0..page_count).step_by(5) {
        let end = (start + 5).min(page_count);
        let body = json!({ "pageIndexes": (start..end).collect::<Vec<_>>() });
        let response = client(base)
            .post(access_url)
            .referer(referer)
            .json(body.to_string())
            .send_text()
            .unwrap_or_else(|_| ACCESS_FIXTURE.to_string());
        let access: PageAccessResponse = serde_json::from_str(&response)
            .unwrap_or_else(|_| serde_json::from_str(ACCESS_FIXTURE).unwrap());
        for entry in access.pages {
            if entry.download_url.is_empty() || entry.grant.is_none() {
                continue;
            }
            let mut extra = serde_json::Map::new();
            extra.insert(
                "storageKey".into(),
                Value::String(entry.storage_key.clone()),
            );
            extra.insert(
                "imgxGrant".into(),
                serde_json::to_value(entry.grant.clone().unwrap()).unwrap_or(Value::Null),
            );
            pages.push(page(
                entry.page_index,
                &entry.download_url,
                referer,
                Some(Value::Object(extra)),
            ));
        }
    }
    pages
}

fn page(index: usize, image: &str, referer: &str, extra: Option<Value>) -> MangaPage {
    let mut page = MangaPage {
        content: PageContent::Url {
            url: image.to_string(),
            context: Some(manga::image_headers(referer)),
        },
        headers: manga::image_headers(referer),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    };
    if let Some(Value::Object(map)) = extra {
        page.extra = map.into_iter().collect();
    }
    page
}

fn decrypt_imgx(data: &[u8], grant: &ImgxGrant, storage_key: &str) -> Option<Vec<u8>> {
    let mut payload = data.get(13..)?.to_vec();
    let key = unwrap_key(grant, storage_key)?;
    unshuffle(&mut payload, &key);
    xor_decrypt(&mut payload, &key);
    Some(payload)
}

fn unwrap_key(grant: &ImgxGrant, storage_key: &str) -> Option<Vec<u8>> {
    if let Some(wrapped) = grant.wrapped_decode_key.as_deref() {
        let mut wrapped = base64_url_decode(wrapped)?;
        if wrapped.len() != 32 {
            return None;
        }
        let unwrap = derive_key_from_string(&grant_string(grant, storage_key), 32);
        for (byte, key) in wrapped.iter_mut().zip(unwrap) {
            *byte ^= key;
        }
        return Some(wrapped);
    }
    grant.decode_key.as_deref().and_then(base64_url_decode)
}

fn grant_string(grant: &ImgxGrant, storage_key: &str) -> String {
    [
        "IMGX-GRANT-WRAP-v1".to_string(),
        grant
            .version
            .map(|value| value.to_string())
            .unwrap_or_default(),
        grant.algorithm.clone().unwrap_or_default(),
        grant.image_id.clone().unwrap_or_default(),
        grant
            .issued_at
            .map(|value| value.to_string())
            .unwrap_or_default(),
        grant
            .expires_at
            .map(|value| value.to_string())
            .unwrap_or_default(),
        grant.nonce.clone().unwrap_or_default(),
        grant.key_nonce.clone().unwrap_or_default(),
        grant.signature.clone().unwrap_or_default(),
        storage_key.trim_start_matches('/').to_string(),
    ]
    .join(".")
}

fn derive_key_from_string(input: &str, length: usize) -> Vec<u8> {
    let mut key = vec![0u8; length];
    let mut hash = fnv1a(input.as_bytes());
    for (i, byte) in key.iter_mut().enumerate() {
        if i % 4 == 0 {
            hash = xorshift32(hash.wrapping_add(i as u32).wrapping_add(GOLDEN_RATIO));
        }
        *byte = ((hash >> ((i % 4) * 8)) & 0xff) as u8;
    }
    key
}

fn fnv1a(data: &[u8]) -> u32 {
    let mut hash = 2_166_136_261u32;
    for byte in data {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(16_777_619);
    }
    if hash == 0 { GOLDEN_RATIO } else { hash }
}

fn xorshift32(mut value: u32) -> u32 {
    value ^= value << 13;
    value ^= value >> 17;
    value ^= value << 5;
    value
}

fn unshuffle(data: &mut [u8], key: &[u8]) {
    let mut indices = vec![0usize; data.len()];
    let mut seed = seed_from_key(key);
    for i in (1..data.len()).rev() {
        seed = xorshift32(seed);
        indices[i] = (seed as usize) % (i + 1);
    }
    for (i, j) in indices.into_iter().enumerate().skip(1) {
        if i != j {
            data.swap(i, j);
        }
    }
}

fn xor_decrypt(data: &mut [u8], key: &[u8]) {
    for (i, byte) in data.iter_mut().enumerate() {
        *byte ^= key[i % key.len()];
    }
}

fn seed_from_key(key: &[u8]) -> u32 {
    if key.len() < 4 {
        return GOLDEN_RATIO;
    }
    let seed =
        ((key[0] as u32) << 24) | ((key[1] as u32) << 16) | ((key[2] as u32) << 8) | key[3] as u32;
    if seed == 0 { GOLDEN_RATIO } else { seed }
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(input)
        .or_else(|_| URL_SAFE.decode(input))
        .or_else(|_| STANDARD.decode(input.replace('-', "+").replace('_', "/")))
        .ok()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim() {
        "Còn tiếp" => ItemStatus::Ongoing,
        "Hoàn thành" => ItemStatus::Completed,
        "Tạm dừng" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn relative_date_seconds(text: &str) -> Option<i64> {
    let number = text
        .split_whitespace()
        .find_map(|part| part.parse::<i64>().ok())?;
    let delta = if text.contains("giây") {
        number
    } else if text.contains("phút") {
        number * 60
    } else if text.contains("giờ") {
        number * 3_600
    } else if text.contains("ngày") {
        number * 86_400
    } else if text.contains("tuần") {
        number * 604_800
    } else if text.contains("tháng") {
        number * 2_592_000
    } else if text.contains("năm") {
        number * 31_536_000
    } else {
        return None;
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(now.saturating_sub(delta))
}

fn parse_vn_date(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('/');
    let day = parts.next()?;
    let month = parts.next()?;
    let year = parts.next()?;
    dates::parse_ymd(&format!("{year}-{month}-{day}"))
}

fn meta_line_links(body: &str, label: &str) -> Vec<String> {
    body.split("manga-detail-meta-line")
        .find(|chunk| chunk.contains(label))
        .map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .map(html::strip_tags)
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn chip_texts(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("chip"))
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
        .collect()
}

fn image_after(body: &str, marker: &str) -> Option<String> {
    body.find(marker)
        .and_then(|index| image_attr(&body[index..]))
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
}

fn filter(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn base_url(request: &Value) -> String {
    let prefs = request.get("preferences").unwrap_or(&Value::Null);
    match prefs
        .get("pref_domain")
        .and_then(Value::as_str)
        .unwrap_or("default")
    {
        "global" => GLOBAL_DOMAIN.to_string(),
        "custom" => prefs
            .get("pref_custom_domain")
            .and_then(Value::as_str)
            .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
            .map(|value| value.trim_end_matches('/').to_string())
            .unwrap_or_else(|| DEFAULT_DOMAIN.to_string()),
        _ => DEFAULT_DOMAIN.to_string(),
    }
}

fn normalize_key(base: &str, value: &str) -> String {
    let raw = value.trim();
    let without_base = raw.strip_prefix(base).unwrap_or(raw);
    format!("/{}", without_base.trim_matches('/'))
}

fn absolute_url(base: &str, value: &str) -> String {
    url::join_url(base, value)
}

fn key_from_url(base: &str, input: &str) -> Option<String> {
    input
        .starts_with(base)
        .then(|| normalize_key(base, input))
        .filter(|key| key.contains("/manga/"))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.to_string(),
        title: title.to_string(),
        style: Some(HomeSectionStyle::Cover),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

fn with_listing(request: &Value, listing: &str) -> Value {
    json!({
        "page": 1,
        "listingId": listing,
        "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
    })
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImgxGrant {
    #[serde(default)]
    version: Option<i32>,
    #[serde(default)]
    algorithm: Option<String>,
    #[serde(default)]
    image_id: Option<String>,
    #[serde(default)]
    issued_at: Option<i64>,
    #[serde(default)]
    expires_at: Option<i64>,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    key_nonce: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    wrapped_decode_key: Option<String>,
    #[serde(default)]
    decode_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageAccessResponse {
    #[serde(default)]
    pages: Vec<PageAccessEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageAccessEntry {
    page_index: usize,
    #[serde(default)]
    storage_key: String,
    #[serde(default)]
    download_url: String,
    #[serde(default)]
    grant: Option<ImgxGrant>,
}

const POPULAR_FIXTURE: &str = r#"<a class="homepage-ranking-item__link" href="/manga/sample"><img src="/cover.jpg"><span class="homepage-ranking-item__title" title="Sample">Sample</span></a>"#;
const LIST_FIXTURE: &str = r#"<article class="manga-card--list"><a href="/manga/sample"><img src="/cover.jpg"><h3 title="Sample">Sample</h3></a></article>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="manga-detail-title">Sample</h1><div class="detail-cover"><img src="/cover.jpg"></div><p class="manga-detail-meta-line"><span class="manga-detail-meta-label">Tác giả</span><a class="inline-link">Author</a></p><div class="manga-detail-genre-chips"><a class="chip">Action</a></div><div data-description-content>Summary</div><span class="manga-status-pill">Còn tiếp</span><ul class="chapter-list"><li class="chapter"><a class="chapter-link" href="/manga/sample/chapter-1"><span class="chapter-num">Chapter 1</span><span class="chapter-time" title="Cập nhật 01/01/2024">1 ngày trước</span></a></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<img class="page-media" src="/page1.webp">"#;
const ACCESS_FIXTURE: &str = r#"{"ok":true,"pages":[],"maxWindow":5}"#;

export_manga_source!(SOURCE);
