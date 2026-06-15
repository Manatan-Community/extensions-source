use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, GenericImage, ImageFormat, RgbaImage};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ImageRequest, ItemStatus, MangaChapter, MangaPage,
    PageContent, Paged, ProcessedImage, SearchRequest, UrlResolveResult,
    abi::{ExtensionError, ExtensionResult, system_time},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{
    html, manga,
    sdk::http::{Headers, HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Cursor};

const SOURCE: ReaderStore = ReaderStore;
const BASE_URL: &str = "https://ebookstore.sony.jp";
const API_URL: &str = "https://ebookstore.sony.jp/front-api";
const VIEWER_URL: &str = "https://viewer.ebookstore.sony.jp/viewer";
const QUALITY_HIGH: &str = "high";
const ACCEPT_FORMATS: &str = "webp,jpeg,png";
const TYPE_COMIC: &str = "comic";
const FIXTURE_SEARCH: &str = r#"{"response":{"numFound":1,"start":0,"docs":[{"iid":"sample-title","first_thumbnail_s":"https://ebookstore.sony.jp/sample/SMALL.jpg","thumbnail_sm":["https://ebookstore.sony.jp/sample/LARGE.jpg"],"name_su":"Reader Store Sample"}]}}"#;

struct ReaderStore;

impl MangaSource for ReaderStore {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return parse_search(FIXTURE_SEARCH);
        }
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "newArrival"
        } else {
            "popularRank"
        };
        search_page("", page(&request), Some(sort), &request)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)?],
                has_next_page: false,
            });
        }
        search_page(query, page(&request), None, &request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample-title".into());
        details_by_key(&key)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample-title".into());
        let hide_locked = preference_bool(&request, "hide_locked", false);
        let mut chapters = details_items(&key)?
            .into_iter()
            .filter(|item| !hide_locked || (!item.is_locked() && !item.is_preview()))
            .filter_map(MangaResponseItem::to_chapter)
            .collect::<Vec<_>>();
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample-aid#1".into());
        let (aid, is_sample) = chapter_parts(&key);
        let token = match fetch_token(&aid, is_sample) {
            Ok(token) => token,
            Err(_) => {
                return Ok(vec![manga::text_page(
                    "Log in via WebView and purchase this product to read.",
                )]);
            }
        };
        let nmr = nonce()?;
        let auth_headers = reader_headers(&nmr, &token.auth_token, &token.uuid);
        let base = format!("{VIEWER_URL}/{}", token.browser_contents_id);
        let meta: MetaResponse =
            fetch_json_headers(&format!("{base}/meta"), auth_headers.clone(), "meta")?;
        let max_index = meta
            .data
            .page
            .all
            .ok_or_else(|| err("Reader Store novels are not supported by this manga source"))?
            .saturating_sub(1);
        let worker = fetch_text_headers(&format!("{base}/decrypt"), auth_headers.clone())?;
        let cipher_key = extract_cipher_key(&worker)
            .ok_or_else(|| err("Reader Store decrypt worker did not expose a cipher key"))?;
        let mut pages = Vec::new();
        for index in 0..=max_index {
            let target = format!(
                "{base}/image_url?indices={}&code={QUALITY_HIGH}&accept={}",
                index,
                url::query_escape(ACCEPT_FORMATS)
            );
            let mut headers = auth_headers.clone();
            add_page_headers(&mut headers, &[index], max_index, false);
            let extra = json!({
                "index": index,
                "nmr": nmr,
                "token": token.auth_token,
                "uuid": token.uuid,
                "contentId": token.browser_contents_id,
                "maxIndex": max_index,
                "cipherKey": cipher_key,
                "contentType": meta.data.kind
            });
            pages.push(MangaPage {
                content: PageContent::Request {
                    request: ImageRequest {
                        url: target,
                        method: Some("GET".into()),
                        headers: headers.clone(),
                        referrer: Some(BASE_URL.into()),
                        extra: object_from_value(extra.clone()),
                        ..ImageRequest::default()
                    },
                },
                headers,
                extra: BTreeMap::from([("readerstore".into(), extra)]),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            });
        }
        Ok(pages)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
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
        process_readerstore_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/title/{}/", key.trim_matches('/'))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (aid, is_sample) = chapter_parts(&key);
            format!("{API_URL}/viewer/?aid={aid}&isSample={is_sample}&redirectPathForReadEnd=")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)?),
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
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_header("safeSearch", r#"{"safeAdultGenreFlg":false,"safeNonCherryFlg":false,"safeBLGenreFlg":false,"safeTLGenreFlg":false,"safeBikiniGenreFlg":false}"#)
        .with_header("agelimit_auth", "true")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn search_page(
    query: &str,
    page: u64,
    forced_sort: Option<&str>,
    request: &Value,
) -> ExtensionResult<Paged<CatalogItem>> {
    let mut params = vec![
        ("q".to_string(), query.to_string()),
        ("page".to_string(), page.to_string()),
        ("cs".to_string(), "search_keyword".to_string()),
        ("safeAdult".to_string(), "false".to_string()),
    ];
    let sort = forced_sort
        .map(ToOwned::to_owned)
        .or_else(|| filter_string(request, "sort"))
        .unwrap_or_else(|| "match".into());
    params.push(("sort".into(), sort));
    for id in ["release", "priceMin", "priceMax"] {
        if let Some(value) = filter_string(request, id).filter(|value| !value.is_empty()) {
            params.push((id.into(), value));
        }
    }
    for id in ["genre", "sale", "saleStatus", "exclude"] {
        for value in filter_array(request, id) {
            params.push((id.into(), value));
        }
    }
    let target = format!("{API_URL}/search/detail/?{}", query_pairs(&params));
    parse_search(&fetch_text(&target, FIXTURE_SEARCH))
}

fn details_by_key(key: &str) -> ExtensionResult<CatalogItem> {
    let mut items = details_items(key)?;
    items
        .pop()
        .map(|item| item.to_item(true))
        .ok_or_else(|| err("Reader Store title details were missing"))
}

fn details_items(key: &str) -> ExtensionResult<Vec<MangaResponseItem>> {
    let target = format!(
        "{API_URL}/contents/title/{}/?sort=desc&page=1&count=1000&fields=detail&fields=title&fields=authors&fields=floor&fields=price&fields=point&fields=browserView",
        url::query_escape(key.trim_matches('/'))
    );
    fetch_json(&target, "details")
}

fn fetch_token(aid: &str, is_sample: bool) -> ExtensionResult<Token> {
    let target = format!(
        "{API_URL}/viewer/?aid={}&isSample={}&redirectPathForReadEnd=",
        url::query_escape(aid),
        is_sample
    );
    let response: TokenResponse = fetch_json(&target, "viewer token")?;
    Ok(response.token)
}

fn fetch_text(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json<T: serde::de::DeserializeOwned>(target: &str, label: &str) -> ExtensionResult<T> {
    let body = client().get(target).xhr().send_text()?;
    serde_json::from_str(&body)
        .map_err(|error| err(&format!("Reader Store {label} JSON decode failed: {error}")))
}

fn fetch_json_headers<T: serde::de::DeserializeOwned>(
    target: &str,
    headers: Headers,
    label: &str,
) -> ExtensionResult<T> {
    let body = fetch_text_headers(target, headers)?;
    serde_json::from_str(&body)
        .map_err(|error| err(&format!("Reader Store {label} JSON decode failed: {error}")))
}

fn fetch_text_headers(target: &str, headers: Headers) -> ExtensionResult<String> {
    client().fetch_text("GET", target, None, headers)
}

fn fetch_bytes_headers(target: &str, headers: Headers) -> ExtensionResult<Vec<u8>> {
    let response = client().fetch("GET", target, None, headers)?;
    let Some(body) = response.body_base64 else {
        return Ok(response.text.unwrap_or_default().into_bytes());
    };
    STANDARD.decode(body).map_err(|error| {
        err(&format!(
            "Reader Store binary base64 decode failed: {error}"
        ))
    })
}

fn parse_search(body: &str) -> ExtensionResult<Paged<CatalogItem>> {
    let result: SearchResponse = serde_json::from_str(body)
        .or_else(|_| serde_json::from_str(FIXTURE_SEARCH))
        .map_err(|error| err(&format!("Reader Store search JSON decode failed: {error}")))?;
    Ok(Paged {
        has_next_page: result.response.has_next_page(),
        entries: result.response.docs.into_iter().map(Doc::to_item).collect(),
    })
}

fn process_readerstore_image(request: Value) -> ExtensionResult<ProcessedImage> {
    let text = image_text(&request)?;
    let image_response: ImageResponse = serde_json::from_str(&text).map_err(|error| {
        err(&format!(
            "Reader Store image_url JSON decode failed: {error}"
        ))
    })?;
    let meta = page_meta(&request)?;
    let index = meta.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
    let max_index = meta
        .get("maxIndex")
        .and_then(Value::as_u64)
        .unwrap_or(index as u64) as u32;
    let nmr = meta
        .get("nmr")
        .and_then(Value::as_str)
        .ok_or_else(|| err("Reader Store page nmr missing"))?;
    let token = meta
        .get("token")
        .and_then(Value::as_str)
        .ok_or_else(|| err("Reader Store page token missing"))?;
    let uuid = meta
        .get("uuid")
        .and_then(Value::as_str)
        .ok_or_else(|| err("Reader Store page UUID missing"))?;
    let content_id = meta
        .get("contentId")
        .and_then(Value::as_str)
        .ok_or_else(|| err("Reader Store content id missing"))?;
    let cipher_key = meta
        .get("cipherKey")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let is_comic = meta.get("contentType").and_then(Value::as_str) == Some(TYPE_COMIC);
    let cdn_url = image_response.data.url;
    let batch = query_param(&cdn_url, "indices")
        .map(|value| {
            value
                .split(',')
                .filter_map(|part| part.parse::<u32>().ok())
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| vec![index]);
    let offset = batch.iter().position(|value| *value == index).unwrap_or(0);
    let page_meta = image_response
        .data
        .meta
        .get(offset)
        .ok_or_else(|| err("Reader Store image metadata missing"))?;
    let env_v = query_param(&cdn_url, "v")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(1);
    let mut auth_headers = reader_headers(nmr, token, uuid);
    add_page_headers(&mut auth_headers, &batch, max_index, true);
    let raw = read_framed_page(
        &fetch_bytes_headers(&cdn_url, auth_headers.clone())?,
        offset,
    )?;
    let (image, side, order) = if page_meta.is_crypted {
        let header_key = decode_cipher_key(cipher_key)?;
        let header = fetch_encrypted_header(content_id, &batch, max_index, offset, auth_headers)?;
        let decrypted_header = ReaderDecoder::decrypt(&header_key, &[0, 1, 2, 3], &header);
        let image_key = read_image_key(&decrypted_header)?;
        let (side, order) = parse_scramble_order(&decrypted_header);
        (
            ReaderDecoder::decrypt(&image_key, &[0, 1, 2, 3], &raw),
            side,
            order,
        )
    } else if page_meta.is_scrambled {
        let (side, order) = compute_scramble_order(page_meta, content_id, is_comic, env_v);
        (raw, side, order)
    } else {
        (raw, 0, Vec::new())
    };
    if !page_meta.is_scrambled || order.is_empty() || side == 0 {
        return Ok(ProcessedImage {
            image_base64: STANDARD.encode(image),
            mime_type: Some(page_meta.mimetype.clone()),
            ..ProcessedImage::default()
        });
    }
    let decoded = image::load_from_memory(&image)
        .map_err(|error| err(&format!("Reader Store image decode failed: {error}")))?
        .to_rgba8();
    let descrambled = unscramble(decoded, &order, side, page_meta)?;
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(descrambled)
        .write_to(&mut out, ImageFormat::WebP)
        .map_err(|error| err(&format!("Reader Store WebP encode failed: {error}")))?;
    Ok(ProcessedImage {
        image_base64: STANDARD.encode(out.into_inner()),
        mime_type: Some("image/webp".into()),
        ..ProcessedImage::default()
    })
}

fn image_text(request: &Value) -> ExtensionResult<String> {
    let encoded = request
        .get("imageBase64")
        .or_else(|| request.get("image_base64"))
        .and_then(Value::as_str)
        .ok_or_else(|| err("Reader Store image processing did not receive image_url bytes"))?;
    let bytes = STANDARD.decode(encoded).map_err(|error| {
        err(&format!(
            "Reader Store image_url base64 decode failed: {error}"
        ))
    })?;
    String::from_utf8(bytes).map_err(|error| {
        err(&format!(
            "Reader Store image_url text decode failed: {error}"
        ))
    })
}

fn page_meta(request: &Value) -> ExtensionResult<&Value> {
    request
        .get("page")
        .and_then(|page| page.get("extra"))
        .and_then(|extra| extra.get("readerstore"))
        .ok_or_else(|| err("Reader Store page metadata missing"))
}

fn fetch_encrypted_header(
    content_id: &str,
    indices: &[u32],
    max_index: u32,
    target_offset: usize,
    mut headers: Headers,
) -> ExtensionResult<Vec<u8>> {
    add_page_headers(&mut headers, indices, max_index, true);
    let target = format!(
        "{VIEWER_URL}/{content_id}/header?indices={}&code={QUALITY_HIGH}&accept={}",
        indices
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(","),
        url::query_escape(ACCEPT_FORMATS)
    );
    let bytes = fetch_bytes_headers(&target, headers)?;
    read_header_page(&bytes, target_offset)
}

fn reader_headers(nmr: &str, token: &str, uuid: &str) -> Headers {
    BTreeMap::from([
        ("X-Nmr".into(), nmr.into()),
        ("X-Token".into(), token.into()),
        ("X-Uuid".into(), uuid.into()),
        ("X-Use-Cache".into(), "false".into()),
    ])
}

fn add_page_headers(
    headers: &mut Headers,
    indices: &[u32],
    max_index: u32,
    include_exclude_ranges: bool,
) {
    headers.insert(
        "X-Indices".into(),
        format!(
            "[{}]",
            indices
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
    );
    headers.insert("X-Max-Index".into(), max_index.to_string());
    headers.insert("X-Quality".into(), QUALITY_HIGH.into());
    if include_exclude_ranges {
        headers.insert("X-Exclude-Ranges".into(), "[]".into());
    }
}

fn read_framed_page(bytes: &[u8], target_offset: usize) -> ExtensionResult<Vec<u8>> {
    let mut cursor = 0usize;
    for _ in 0..target_offset {
        let size = le_u32(bytes, cursor)
            .ok_or_else(|| err("Reader Store image frame was truncated"))?
            as usize;
        cursor = cursor.saturating_add(4).saturating_add(size);
    }
    let size =
        le_u32(bytes, cursor).ok_or_else(|| err("Reader Store image frame size missing"))? as usize;
    let start = cursor + 4;
    let end = start + size;
    if end > bytes.len() {
        return Err(err("Reader Store image frame payload was truncated"));
    }
    Ok(bytes[start..end].to_vec())
}

fn read_header_page(bytes: &[u8], target_offset: usize) -> ExtensionResult<Vec<u8>> {
    let mut cursor = 0usize;
    for _ in 0..target_offset {
        let meta_size = le_u32(bytes, cursor)
            .ok_or_else(|| err("Reader Store header metadata was truncated"))?
            as usize;
        cursor = cursor.saturating_add(4).saturating_add(meta_size);
        let page_size = le_u32(bytes, cursor)
            .ok_or_else(|| err("Reader Store header page was truncated"))?
            as usize;
        cursor = cursor.saturating_add(4).saturating_add(page_size);
    }
    let meta_size =
        le_u32(bytes, cursor).ok_or_else(|| err("Reader Store header metadata missing"))? as usize;
    cursor = cursor.saturating_add(4).saturating_add(meta_size);
    let page_size =
        le_u32(bytes, cursor).ok_or_else(|| err("Reader Store header page size missing"))? as usize;
    let start = cursor + 4;
    let end = start + page_size;
    if end > bytes.len() {
        return Err(err("Reader Store header page payload was truncated"));
    }
    slice_encrypted_header(&bytes[start..end])
}

fn slice_encrypted_header(page: &[u8]) -> ExtensionResult<Vec<u8>> {
    let header_size =
        le_u32(page, 0).ok_or_else(|| err("Reader Store encrypted header size missing"))? as usize;
    let start = 4 + 20;
    let end = 4 + header_size;
    if start > page.len() || end > page.len() || start > end {
        return Err(err("Reader Store encrypted header was truncated"));
    }
    Ok(page[start..end].to_vec())
}

fn read_image_key(header: &[u8]) -> ExtensionResult<[u32; 4]> {
    if header.len() < 20 {
        return Err(err(
            "Reader Store decrypted header did not contain an image key",
        ));
    }
    Ok([
        le_u32(header, 4).unwrap_or(0),
        le_u32(header, 8).unwrap_or(0),
        le_u32(header, 12).unwrap_or(0),
        le_u32(header, 16).unwrap_or(0),
    ])
}

fn parse_scramble_order(header: &[u8]) -> (u32, Vec<u32>) {
    if header.len() < 24 {
        return (0, Vec::new());
    }
    let tile_count = le_u16(header, 20).unwrap_or(0) as usize / 2;
    let side = le_u16(header, 22).unwrap_or(0) as u32;
    let mut order = Vec::with_capacity(tile_count);
    for index in 0..tile_count {
        let offset = 24 + index * 2;
        if let Some(value) = le_u16(header, offset) {
            order.push(value as u32);
        }
    }
    (side, order)
}

fn compute_scramble_order(
    meta: &Meta,
    content_id: &str,
    is_comic: bool,
    env_v: u32,
) -> (u32, Vec<u32>) {
    let side = if meta.mimetype == "image/webp" && is_comic {
        152
    } else {
        48
    };
    let h_blocks = meta.width.div_ceil(side);
    let v_blocks = meta.height.div_ceil(side);
    if h_blocks == 0 || v_blocks == 0 {
        return (side, Vec::new());
    }
    let mut main = Vec::new();
    let mut h_tail = Vec::new();
    for row in 0..v_blocks.saturating_sub(1) {
        let start = row * h_blocks;
        for col in 0..h_blocks.saturating_sub(1) {
            main.push(start + col);
        }
        h_tail.push(start + h_blocks - 1);
    }
    let mut v_tail = Vec::new();
    let last_row_start = (v_blocks - 1) * h_blocks;
    for col in 0..h_blocks.saturating_sub(1) {
        v_tail.push(last_row_start + col);
    }
    let main = shuffle_order(&main, content_id, env_v);
    let h_tail = shuffle_order(&h_tail, content_id, env_v);
    let v_tail = shuffle_order(&v_tail, content_id, env_v);
    let mut result = Vec::new();
    let mut main_offset = 0usize;
    for row in 0..v_blocks.saturating_sub(1) as usize {
        let take = h_blocks.saturating_sub(1) as usize;
        result.extend_from_slice(&main[main_offset..main_offset + take]);
        main_offset += take;
        if let Some(value) = h_tail.get(row) {
            result.push(*value);
        }
    }
    result.extend(v_tail);
    result.push(h_blocks * v_blocks - 1);
    (side, result)
}

fn shuffle_order(indices: &[u32], content_id: &str, env_v: u32) -> Vec<u32> {
    const PRE_SHARED: [u32; 20] = [
        19, 20, 14, 1, 5, 2, 4, 15, 9, 17, 8, 16, 18, 11, 10, 7, 12, 6, 13, 3,
    ];
    let seeds = content_id
        .chars()
        .filter_map(|ch| ch.to_digit(10).filter(|value| *value > 0))
        .collect::<Vec<_>>();
    if seeds.is_empty() {
        return indices.to_vec();
    }
    let mut pi = env_v as usize % seeds.len();
    let mut si = 0usize;
    let mut keyed = indices
        .iter()
        .map(|idx| {
            let key = PRE_SHARED[si % PRE_SHARED.len()] + seeds[pi % seeds.len()];
            si += 1;
            pi += 1;
            (key, *idx)
        })
        .collect::<Vec<_>>();
    keyed.sort_by_key(|(key, idx)| (*key, *idx));
    keyed.into_iter().map(|(_, idx)| idx).collect()
}

fn unscramble(
    source: RgbaImage,
    order: &[u32],
    side: u32,
    meta: &Meta,
) -> ExtensionResult<RgbaImage> {
    let dst_w = meta.width;
    let dst_h = meta.height;
    let cols = dst_w.div_ceil(side);
    let rows = dst_h.div_ceil(side);
    if cols == 0 || rows == 0 {
        return Ok(source);
    }
    let last_col_w = if dst_w % side != 0 {
        dst_w % side
    } else {
        side
    };
    let last_row_h = if dst_h % side != 0 {
        dst_h % side
    } else {
        side
    };
    let last_col = cols - 1;
    let last_row = rows - 1;
    let col_pad = if source.width() == dst_w || last_col == 0 {
        0.0
    } else {
        (source.width().saturating_sub(dst_w)) as f32 / last_col as f32
    };
    let row_pad = if source.height() == dst_h || last_row == 0 {
        0.0
    } else {
        (source.height().saturating_sub(dst_h)) as f32 / last_row as f32
    };
    let mut result = RgbaImage::new(dst_w, dst_h);
    for (src_tile, dst_tile) in order.iter().enumerate() {
        let src_tile = src_tile as u32;
        let src_col = src_tile % cols;
        let src_row = src_tile / cols;
        let dst_col = *dst_tile % cols;
        let dst_row = *dst_tile / cols;
        let tile_w = if dst_col == last_col {
            last_col_w
        } else {
            side
        };
        let tile_h = if dst_row == last_row {
            last_row_h
        } else {
            side
        };
        let sx = ((side as f32 + col_pad) * src_col as f32) as u32;
        let sy = ((side as f32 + row_pad) * src_row as f32) as u32;
        let dx = side * dst_col;
        let dy = side * dst_row;
        if sx >= source.width() || sy >= source.height() || dx >= dst_w || dy >= dst_h {
            continue;
        }
        let tile = image::imageops::crop_imm(
            &source,
            sx,
            sy,
            tile_w.min(source.width() - sx),
            tile_h.min(source.height() - sy),
        )
        .to_image();
        result
            .copy_from(&tile, dx, dy)
            .map_err(|_| err("Reader Store tile copy failed"))?;
    }
    Ok(result)
}

fn decode_cipher_key(hex: &str) -> ExtensionResult<[u32; 4]> {
    if hex.len() != 32 {
        return Err(err("Reader Store cipher key length was invalid"));
    }
    let bytes = decode_hex(hex)?;
    Ok([
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
        u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
    ])
}

fn decode_hex(input: &str) -> ExtensionResult<Vec<u8>> {
    if input.len() % 2 != 0 {
        return Err(err("hex string has an odd length"));
    }
    let mut out = Vec::with_capacity(input.len() / 2);
    let bytes = input.as_bytes();
    for index in (0..bytes.len()).step_by(2) {
        let high = hex_nibble(bytes[index]).ok_or_else(|| err("invalid hex digit"))?;
        let low = hex_nibble(bytes[index + 1]).ok_or_else(|| err("invalid hex digit"))?;
        out.push((high << 4) | low);
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn extract_cipher_key(worker: &str) -> Option<String> {
    let arrays = four_int_arrays(worker);
    let selected = arrays.iter().find(|array| **array != [0, 1, 2, 3])?;
    Some(
        selected
            .iter()
            .map(|value| format!("{value:08x}"))
            .collect::<String>(),
    )
}

fn four_int_arrays(input: &str) -> Vec<[u32; 4]> {
    let mut arrays = Vec::new();
    for chunk in input.split('[').skip(1) {
        let Some((inner, _)) = chunk.split_once(']') else {
            continue;
        };
        let values = inner
            .split(',')
            .filter_map(|part| part.trim().parse::<u32>().ok())
            .collect::<Vec<_>>();
        if values.len() == 4 {
            arrays.push([values[0], values[1], values[2], values[3]]);
        }
    }
    arrays
}

fn nonce() -> ExtensionResult<String> {
    let millis = system_time().map(|time| time.unix_millis).unwrap_or(0);
    Ok(format!("00000000-0000-4000-8000-{millis:012x}"))
}

fn query_pairs(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .or_else(|| request.get("filterValues"))
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .or_else(|| value.as_i64().map(|number| number.to_string()))
        })
}

fn filter_array(request: &Value, id: &str) -> Vec<String> {
    request
        .get("filters")
        .or_else(|| request.get("filterValues"))
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn preference_bool(request: &Value, id: &str, default: bool) -> bool {
    request
        .get("preferences")
        .or_else(|| request.get("prefs"))
        .and_then(Value::as_object)
        .and_then(|prefs| prefs.get(id))
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_str().map(|text| text == "true"))
        })
        .unwrap_or(default)
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(&format!("{BASE_URL}/title/"))
        .map(|value| value.trim_matches('/').to_string())
}

fn chapter_parts(key: &str) -> (String, bool) {
    let mut parts = key.split('#');
    let aid = parts.next().unwrap_or(key).trim_matches('/').to_string();
    let is_sample = parts.next() == Some("1");
    (aid, is_sample)
}

fn query_param(input: &str, name: &str) -> Option<String> {
    let query = input
        .split('?')
        .nth(1)?
        .split('#')
        .next()
        .unwrap_or_default();
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == name {
            return Some(value.to_string());
        }
    }
    None
}

fn object_from_value(value: Value) -> BTreeMap<String, Value> {
    value
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn le_u16(buffer: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes([
        *buffer.get(offset)?,
        *buffer.get(offset + 1)?,
    ]))
}

fn le_u32(buffer: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *buffer.get(offset)?,
        *buffer.get(offset + 1)?,
        *buffer.get(offset + 2)?,
        *buffer.get(offset + 3)?,
    ]))
}

fn err(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.to_string(),
    }
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    response: SearchInner,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchInner {
    num_found: u64,
    start: u64,
    docs: Vec<Doc>,
}

impl SearchInner {
    fn has_next_page(&self) -> bool {
        self.start + (self.docs.len() as u64) < self.num_found
    }
}

#[derive(Debug, Deserialize)]
struct Doc {
    iid: String,
    #[serde(rename = "first_thumbnail_s")]
    first_thumbnail_s: Option<String>,
    #[serde(rename = "thumbnail_sm")]
    thumbnail_sm: Option<Vec<String>>,
    #[serde(rename = "name_su")]
    name_su: String,
}

impl Doc {
    fn to_item(self) -> CatalogItem {
        CatalogItem {
            key: self.iid.clone(),
            title: self.name_su,
            cover: self
                .thumbnail_sm
                .and_then(|values| values.into_iter().next())
                .or(self.first_thumbnail_s)
                .map(best_thumbnail),
            url: Some(format!("{BASE_URL}/title/{}/", self.iid)),
            language: Some("ja".into()),
            content_rating: Some("adult".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

fn best_thumbnail(raw: String) -> String {
    const SIZES: [&str; 4] = ["XLARGE.jpg", "LARGE.jpg", "MIDDLE.jpg", "SMALL.jpg"];
    let base = raw.split(',').next().unwrap_or(&raw);
    if SIZES.iter().any(|size| base.contains(size)) {
        base.to_string()
    } else if let Some(size) = SIZES.into_iter().find(|size| raw.contains(size)) {
        format!("{base}{size}")
    } else {
        base.to_string()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaResponseItem {
    aid: String,
    detail: Detail,
    title: Title,
    authors: Option<Vec<Author>>,
    floor: Option<Floor>,
    price: Option<Price>,
    browser_view: Option<BrowserView>,
}

impl MangaResponseItem {
    fn to_item(self, initialized: bool) -> CatalogItem {
        CatalogItem {
            key: self.aid.clone(),
            title: self.title.title_nm,
            cover: self
                .detail
                .rs_thumbnail_ll
                .clone()
                .or(self.detail.rs_thumbnail_l.clone())
                .or(self.detail.rs_thumbnail_m.clone())
                .or(self.detail.rs_thumbnail_s.clone())
                .map(|path| url::join_url(BASE_URL, &path)),
            authors: self
                .authors
                .unwrap_or_default()
                .into_iter()
                .map(|author| author.author_nm)
                .collect(),
            description: Some(self.detail.description()),
            tags: self
                .floor
                .and_then(|floor| floor.genres)
                .unwrap_or_default()
                .into_iter()
                .map(|genre| genre.genre_nm)
                .collect(),
            status: ItemStatus::Unknown,
            url: Some(format!("{BASE_URL}/title/{}/", self.aid)),
            language: Some("ja".into()),
            content_rating: if self.detail.adult_level == Some(1) {
                Some("adult".into())
            } else {
                Some("safe".into())
            },
            initialized,
            ..CatalogItem::default()
        }
    }

    fn is_locked(&self) -> bool {
        self.price
            .as_ref()
            .and_then(|price| price.price_include_tax)
            .unwrap_or(0)
            != 0
            && self
                .browser_view
                .as_ref()
                .and_then(|view| view.is_browse_sample)
                == Some(false)
            && self.detail.paid_version_aid.is_none()
    }

    fn is_preview(&self) -> bool {
        self.price
            .as_ref()
            .and_then(|price| price.price_include_tax)
            .unwrap_or(0)
            != 0
            && self
                .browser_view
                .as_ref()
                .and_then(|view| view.is_browse_sample)
                == Some(true)
            && self.detail.paid_version_aid.is_none()
    }

    fn to_chapter(self) -> Option<MangaChapter> {
        let locked = self.is_locked();
        let preview = self.is_preview();
        let chapter_key = self.detail.paid_version_aid.clone().unwrap_or_else(|| {
            if preview {
                format!("{}#1", self.aid)
            } else {
                self.aid.clone()
            }
        });
        let title = format!(
            "{}{}{}",
            if locked { "Locked " } else { "" },
            if preview { "Preview " } else { "" },
            self.detail.contents_nm
        );
        Some(MangaChapter {
            key: chapter_key.clone(),
            title: Some(title),
            url: Some(format!(
                "{API_URL}/viewer/?aid={chapter_key}&redirectPathForReadEnd="
            )),
            date_uploaded: self
                .detail
                .original_public_dt
                .as_deref()
                .and_then(parse_readerstore_date),
            chapter_number: self.title.title_index_seq_no.map(|value| value as f32),
            is_locked: locked,
            ..MangaChapter::default()
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Detail {
    contents_nm: String,
    magazine_nm: Option<String>,
    label_nm: Option<String>,
    publisher_nm: Option<String>,
    rs_thumbnail_ll: Option<String>,
    rs_thumbnail_l: Option<String>,
    rs_thumbnail_m: Option<String>,
    rs_thumbnail_s: Option<String>,
    adult_level: Option<u32>,
    paid_version_aid: Option<String>,
    title_explanation_long: Option<String>,
    original_public_dt: Option<String>,
}

impl Detail {
    fn description(&self) -> String {
        let mut parts = Vec::new();
        if let Some(text) = self.title_explanation_long.as_deref() {
            parts.push(html::strip_tags(text));
        }
        if let Some(value) = self.magazine_nm.as_deref() {
            parts.push(format!("Magazine: {value}"));
        }
        if let Some(value) = self.label_nm.as_deref() {
            parts.push(format!("Label: {value}"));
        }
        if let Some(value) = self.publisher_nm.as_deref() {
            parts.push(format!("Publisher: {value}"));
        }
        if self.adult_level == Some(1) {
            parts.push("Rating: 18+".into());
        }
        parts.join("\n\n")
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Title {
    title_nm: String,
    title_index_seq_no: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Author {
    author_nm: String,
}

#[derive(Debug, Deserialize)]
struct Floor {
    genres: Option<Vec<Genre>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Genre {
    genre_nm: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Price {
    price_include_tax: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserView {
    is_browse_sample: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    token: Token,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Token {
    auth_token: String,
    uuid: String,
    browser_contents_id: String,
}

#[derive(Debug, Deserialize)]
struct MetaResponse {
    data: MetaData,
}

#[derive(Debug, Deserialize)]
struct MetaData {
    #[serde(rename = "type")]
    kind: String,
    page: PageInfo,
}

#[derive(Debug, Deserialize)]
struct PageInfo {
    all: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ImageResponse {
    data: ImageData,
}

#[derive(Debug, Deserialize)]
struct ImageData {
    url: String,
    meta: Vec<Meta>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    is_crypted: bool,
    is_scrambled: bool,
    mimetype: String,
    width: u32,
    height: u32,
}

fn parse_readerstore_date(value: &str) -> Option<i64> {
    manatan_shared::dates::parse_ymd(value.split('T').next()?)
}

struct ReaderDecoder;

impl ReaderDecoder {
    fn decrypt(key: &[u32; 4], iv: &[u32; 4], data: &[u8]) -> Vec<u8> {
        let tables = Tables::new();
        let schedule = Self::schedule(key, iv, &tables);
        let mut words = to_le_words(data);
        Self::decrypt_words(&schedule, &tables, &mut words);
        words_to_bytes(&words, data.len())
    }

    fn schedule(key: &[u32; 4], iv: &[u32; 4], tables: &Tables) -> [u32; 20] {
        let mut o = key[0];
        let mut f = key[1];
        let mut v = key[2];
        let mut q = key[3];
        let x = (q >> 24) ^ (q << 8);
        let mut z = tables.t0[idx(x)]
            ^ tables.t1[idx(x >> 8)]
            ^ tables.t2[idx(x >> 16)]
            ^ tables.t3[idx(x >> 24)]
            ^ o
            ^ 16_777_216;
        let mut c = z ^ f;
        let mut f_acc = c ^ v;
        let mut g = f_acc ^ q;
        let h = (g >> 24) ^ (g << 8);
        let mut i = tables.t0[idx(h)]
            ^ tables.t1[idx(h >> 8)]
            ^ tables.t2[idx(h >> 16)]
            ^ tables.t3[idx(h >> 24)]
            ^ z
            ^ 33_554_432;
        let mut j = i ^ c;
        let mut k = j ^ f_acc;
        let mut n = k ^ g;
        let mut q2 = iv[0];
        let mut r = iv[1];
        let mut s = iv[2];
        let mut x_acc = iv[3];
        let mut y = 0u32;
        let mut z2 = 0u32;
        let mut d = 0u32;
        let mut rr = 0u32;
        for _ in 0..24 {
            let e = rr.wrapping_add(f_acc) ^ d ^ z;
            let t = z2.wrapping_add(k) ^ y ^ o;
            let nr = i.wrapping_add(z2);
            let er = c.wrapping_add(rr);
            z2 = tables.t0[idx(y)]
                ^ tables.t1[idx(y >> 8)]
                ^ tables.t2[idx(y >> 16)]
                ^ tables.t3[idx(y >> 24)];
            rr = tables.t0[idx(d)]
                ^ tables.t1[idx(d >> 8)]
                ^ tables.t2[idx(d >> 16)]
                ^ tables.t3[idx(d >> 24)];
            y = tables.t0[idx(er)]
                ^ tables.t1[idx(er >> 8)]
                ^ tables.t2[idx(er >> 16)]
                ^ tables.t3[idx(er >> 24)];
            d = tables.t0[idx(nr)]
                ^ tables.t1[idx(nr >> 8)]
                ^ tables.t2[idx(nr >> 16)]
                ^ tables.t3[idx(nr >> 24)];
            let tr = (z << 8) ^ tables.t4[idx(z >> 24)] ^ f ^ t;
            let carry = (v >> 22) & 256;
            let mask = 255 * ((v >> 31) & 1);
            let ir = ((k >> 24) & 255) | carry;
            let ur = (g << (8 & mask)) ^ tables.t6[(g >> 24 & mask) as usize];
            let vr = tables.t5[ir as usize] ^ (k << 8) ^ ur ^ n ^ s ^ e;
            z = q;
            q = v;
            v = f;
            f = o;
            o = tr;
            k = n;
            n = q2;
            q2 = r;
            r = i;
            i = j;
            j = s;
            s = x_acc;
            x_acc = g;
            g = c;
            c = f_acc;
            f_acc = vr;
        }
        [
            z, q, v, f, o, k, n, q2, r, i, j, s, x_acc, g, c, f_acc, y, z2, d, rr,
        ]
    }

    fn decrypt_words(schedule: &[u32; 20], tables: &Tables, words: &mut [u32]) {
        let mut a = schedule[0];
        let mut n = schedule[1];
        let mut e = schedule[2];
        let mut t = schedule[3];
        let mut o = schedule[4];
        let mut f = schedule[5];
        let mut v = schedule[6];
        let mut w = schedule[7];
        let mut q = schedule[8];
        let mut x = schedule[9];
        let mut z = schedule[10];
        let mut c = schedule[11];
        let mut f_acc = schedule[12];
        let mut g = schedule[13];
        let mut h = schedule[14];
        let mut i = schedule[15];
        let mut j = schedule[16];
        let mut k = schedule[17];
        let mut n2 = schedule[18];
        let mut q2 = schedule[19];
        let mut pos = 0usize;
        while pos < words.len() {
            words[pos] ^= q2.wrapping_add(i) ^ n2 ^ a;
            if pos + 1 < words.len() {
                words[pos + 1] ^= k.wrapping_add(f) ^ j ^ o;
            }
            let er = x.wrapping_add(k);
            let tr = h.wrapping_add(q2);
            k = tables.t0[idx(j)]
                ^ tables.t1[idx(j >> 8)]
                ^ tables.t2[idx(j >> 16)]
                ^ tables.t3[idx(j >> 24)];
            q2 = tables.t0[idx(n2)]
                ^ tables.t1[idx(n2 >> 8)]
                ^ tables.t2[idx(n2 >> 16)]
                ^ tables.t3[idx(n2 >> 24)];
            j = tables.t0[idx(tr)]
                ^ tables.t1[idx(tr >> 8)]
                ^ tables.t2[idx(tr >> 16)]
                ^ tables.t3[idx(tr >> 24)];
            n2 = tables.t0[idx(er)]
                ^ tables.t1[idx(er >> 8)]
                ^ tables.t2[idx(er >> 16)]
                ^ tables.t3[idx(er >> 24)];
            let mixed = (a << 8) ^ tables.t4[idx(a >> 24)] ^ t;
            let carry = (e >> 22) & 256;
            let mask = 255 * ((e >> 31) & 1);
            let ur = ((f >> 24) & 255) | carry;
            let vr = (g << (8 & mask)) ^ tables.t6[(g >> 24 & mask) as usize];
            let yr = tables.t5[ur as usize] ^ (f << 8) ^ vr ^ v ^ c;
            a = n;
            n = e;
            e = t;
            t = o;
            o = mixed;
            f = v;
            v = w;
            w = q;
            q = x;
            x = z;
            z = c;
            c = f_acc;
            f_acc = g;
            g = h;
            h = i;
            i = yr;
            pos += 2;
        }
    }
}

struct Tables {
    t0: [u32; 256],
    t1: [u32; 256],
    t2: [u32; 256],
    t3: [u32; 256],
    t4: [u32; 256],
    t5: [u32; 512],
    t6: [u32; 256],
}

impl Tables {
    fn new() -> Self {
        let mut tables = Self {
            t0: [0; 256],
            t1: [0; 256],
            t2: [0; 256],
            t3: [0; 256],
            t4: [0; 256],
            t5: [0; 512],
            t6: [0; 256],
        };
        let r = [
            3054005530, 2937117236, 2636150632, 4181782224, 830480227, 1656962758, 3292888143,
            1267398814,
        ];
        let a = [
            2700475438, 1841812828, 3668150200, 2573935453, 534136506, 1048677465, 2083468722,
            4166937161,
        ];
        let n = [
            1543012243, 3065904747, 557298134, 1114517473, 2229034639, 1173732435, 2326214054,
            1493395969,
        ];
        let e = [
            1163482763, 2326965363, 1895906790, 3791813289, 2701456439, 654871918, 1309704156,
            2619408093,
        ];
        for row in 0..256usize {
            for bit in 0..8usize {
                if (row & (1 << bit)) != 0 {
                    tables.t4[row] ^= r[bit];
                    tables.t5[row] ^= n[bit];
                    tables.t5[256 + row] ^= a[bit];
                    tables.t6[row] ^= e[bit];
                }
            }
        }
        let mut pow = [0u32; 256];
        for row in 1..256usize {
            let mut bits = [0u32; 8];
            bits[0] = row as u32;
            for bit in 1..8usize {
                bits[bit] = bits[bit - 1] << 1;
                if (bits[bit] & 256) != 0 {
                    bits[bit] ^= 283;
                }
            }
            let mut acc = 1u32;
            for _ in 0..254 {
                let mut next = 0u32;
                for bit in 0..8usize {
                    if (acc & (1 << bit)) != 0 {
                        next ^= bits[bit];
                    }
                }
                acc = next;
            }
            pow[row] = acc;
        }
        let mut sbox = [0u32; 256];
        for row in 0..256usize {
            let mut value =
                pow[row] ^ (pow[row] << 1) ^ (pow[row] << 2) ^ (pow[row] << 3) ^ (pow[row] << 4);
            value = (value >> 8) ^ (value & 255) ^ 99;
            sbox[row] = value;
        }
        for row in 0..256usize {
            let k = sbox[row];
            let mut l = k << 1;
            if (l & 256) != 0 {
                l ^= 283;
            }
            let dv = l ^ k;
            tables.t0[row] = (dv << 24) ^ (k << 16) ^ (k << 8) ^ l;
            tables.t1[row] = (k << 24) ^ (k << 16) ^ (l << 8) ^ dv;
            tables.t2[row] = (k << 24) ^ (l << 16) ^ (dv << 8) ^ k;
            tables.t3[row] = (l << 24) ^ (dv << 16) ^ (k << 8) ^ k;
        }
        tables
    }
}

fn idx(value: u32) -> usize {
    (value & 255) as usize
}

fn to_le_words(data: &[u8]) -> Vec<u32> {
    let mut padded = data.to_vec();
    padded.resize(data.len().div_ceil(4) * 4, 0);
    padded
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn words_to_bytes(words: &[u32], byte_size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(words.len() * 4);
    for word in words {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.truncate(byte_size);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_worker_cipher_key() {
        assert_eq!(
            extract_cipher_key("var x=[0,1,2,3]; var e = [1, 2, 3, 4];").as_deref(),
            Some("00000001000000020000000300000004")
        );
    }

    #[test]
    fn reads_framed_page() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(b"one");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(b"two");
        assert_eq!(read_framed_page(&bytes, 1).unwrap(), b"two");
    }
}

export_manga_source!(SOURCE);
