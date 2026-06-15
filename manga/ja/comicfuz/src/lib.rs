use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ComicFuz = ComicFuz;
const BASE_URL: &str = "https://comic-fuz.com";
const API_URL: &str = "https://api.comic-fuz.com/v1";
const CDN_URL: &str = "https://img.comic-fuz.com";

struct ComicFuz;

impl MangaSource for ComicFuz {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            let bytes = post_proto_or_fixture(
                "mangas_by_day_of_week",
                day_of_week_request(0),
                LATEST_FIXTURE,
            );
            return Ok(Paged {
                entries: decode_manga_list(&bytes, 1),
                has_next_page: false,
            });
        }
        self.search(request)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_for_key(&key)],
                has_next_page: false,
            });
        }
        let tag = filter_string(&request, "tag").and_then(|value| value.parse::<u64>().ok());
        if query.is_empty() {
            if let Some(tag_id) = tag {
                let bytes =
                    post_proto_or_fixture("manga_list", manga_list_request(tag_id), LIST_FIXTURE);
                return Ok(Paged {
                    entries: decode_manga_list(&bytes, 1),
                    has_next_page: false,
                });
            }
        }
        let page = request
            .get("page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1);
        let bytes = post_proto_or_fixture("search", search_request(query, page), SEARCH_FIXTURE);
        let (entries, page_count) = decode_search(&bytes);
        Ok(Paged {
            entries,
            has_next_page: page_count > page,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/1".into());
        Ok(details_for_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/1".into());
        let manga_id = trailing_id(&key).unwrap_or(1);
        let bytes = post_proto_or_fixture(
            "manga_detail",
            manga_details_request(manga_id),
            DETAILS_FIXTURE,
        );
        Ok(decode_chapters(&bytes))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/viewer/1".into());
        let chapter_id = trailing_id(&key).unwrap_or(1);
        let bytes = post_proto_or_fixture(
            "manga_viewer",
            manga_viewer_request(chapter_id),
            VIEWER_FIXTURE,
        );
        Ok(decode_pages(&bytes, &key))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_for_key(&key)),
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
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn post_proto_or_fixture(endpoint: &str, payload: Vec<u8>, fixture_hex: &str) -> Vec<u8> {
    let mut headers = manatan_shared::sdk::Context::new();
    headers.insert("Content-Type".into(), "application/protobuf".into());
    headers.insert("Accept".into(), "application/protobuf".into());
    let target = format!("{API_URL}/{endpoint}");
    client()
        .fetch("POST", target, Some(payload), headers)
        .ok()
        .and_then(|response| {
            response
                .body_base64
                .as_deref()
                .and_then(base64_decode)
                .or_else(|| response.text.map(|text| text.into_bytes()))
        })
        .filter(|bytes| !bytes.is_empty())
        .unwrap_or_else(|| hex_decode(fixture_hex))
}

fn details_for_key(key: &str) -> CatalogItem {
    let manga_id = trailing_id(key).unwrap_or(1);
    let bytes = post_proto_or_fixture(
        "manga_detail",
        manga_details_request(manga_id),
        DETAILS_FIXTURE,
    );
    let mut item = decode_detail(&bytes);
    if item.key.is_empty() {
        item.key = format!("/manga/{manga_id}");
        item.title = "COMIC FUZ".into();
    }
    item.initialized = true;
    item
}

fn decode_manga_list(bytes: &[u8], field: u64) -> Vec<CatalogItem> {
    ProtoReader::new(bytes)
        .filter_map(|field_value| match field_value {
            ProtoField::Bytes(number, data) if number == field => Some(decode_manga(data)),
            _ => None,
        })
        .collect()
}

fn decode_search(bytes: &[u8]) -> (Vec<CatalogItem>, u64) {
    let mut entries = Vec::new();
    let mut page_count = 0;
    for field in ProtoReader::new(bytes) {
        match field {
            ProtoField::Bytes(2, data) => entries.push(decode_manga(data)),
            ProtoField::Varint(6, value) => page_count = value,
            _ => {}
        }
    }
    (entries, page_count)
}

fn decode_detail(bytes: &[u8]) -> CatalogItem {
    let mut item = CatalogItem::default();
    for field in ProtoReader::new(bytes) {
        match field {
            ProtoField::Bytes(2, data) => item = decode_manga(data),
            ProtoField::Bytes(4, data) => {
                for author in ProtoReader::new(data) {
                    if let ProtoField::Bytes(1, name) = author {
                        if let Some(value) = decode_name(name) {
                            item.authors.push(value);
                        }
                    }
                }
            }
            ProtoField::Bytes(7, data) => {
                if let Some(value) = decode_name(data) {
                    item.tags.push(value);
                }
            }
            _ => {}
        }
    }
    item.initialized = true;
    item
}

fn decode_manga(bytes: &[u8]) -> CatalogItem {
    let mut id = 0;
    let mut title = String::new();
    let mut cover = None;
    let mut description = None;
    for field in ProtoReader::new(bytes) {
        match field {
            ProtoField::Varint(1, value) => id = value,
            ProtoField::Bytes(2, value) => title = String::from_utf8_lossy(value).into_owned(),
            ProtoField::Bytes(4, value) => {
                cover = Some(format!("{CDN_URL}{}", String::from_utf8_lossy(value)))
            }
            ProtoField::Bytes(14, value) => {
                description = Some(String::from_utf8_lossy(value).into_owned())
            }
            _ => {}
        }
    }
    let key = format!("/manga/{id}");
    CatalogItem {
        key: key.clone(),
        title: if title.is_empty() {
            "COMIC FUZ".into()
        } else {
            title
        },
        cover,
        description,
        url: Some(format!("{BASE_URL}{key}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn decode_name(bytes: &[u8]) -> Option<String> {
    for field in ProtoReader::new(bytes) {
        if let ProtoField::Bytes(2, value) = field {
            return Some(String::from_utf8_lossy(value).into_owned());
        }
    }
    None
}

fn decode_chapters(bytes: &[u8]) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    for field in ProtoReader::new(bytes) {
        if let ProtoField::Bytes(3, group) = field {
            for nested in ProtoReader::new(group) {
                if let ProtoField::Bytes(2, chapter) = nested {
                    if let Some(item) = decode_chapter(chapter) {
                        chapters.push(item);
                    }
                }
            }
        }
    }
    chapters
}

fn decode_chapter(bytes: &[u8]) -> Option<MangaChapter> {
    let mut id = None;
    let mut title = None;
    let mut amount = 0;
    let mut date = None;
    for field in ProtoReader::new(bytes) {
        match field {
            ProtoField::Varint(1, value) => id = Some(value),
            ProtoField::Bytes(2, value) => {
                title = Some(String::from_utf8_lossy(value).into_owned())
            }
            ProtoField::Bytes(5, point) => {
                for nested in ProtoReader::new(point) {
                    if let ProtoField::Varint(2, value) = nested {
                        amount = value;
                    }
                }
            }
            ProtoField::Bytes(8, value) => date = Some(String::from_utf8_lossy(value).into_owned()),
            _ => {}
        }
    }
    let id = id?;
    let key = format!("/manga/viewer/{id}");
    let locked = amount > 0;
    let mut title = title.unwrap_or_else(|| "Chapter".into());
    if locked {
        title = format!("[Locked] {title}");
    }
    Some(MangaChapter {
        key: key.clone(),
        title: Some(title),
        date_uploaded: date.as_deref().and_then(manatan_shared::dates::parse_ymd),
        is_locked: locked,
        url: Some(format!("{BASE_URL}{key}")),
        ..MangaChapter::default()
    })
}

fn decode_pages(bytes: &[u8], chapter_key: &str) -> Vec<MangaPage> {
    let mut pages = Vec::new();
    for field in ProtoReader::new(bytes) {
        if let ProtoField::Bytes(3, page) = field {
            for nested in ProtoReader::new(page) {
                if let ProtoField::Bytes(1, image) = nested {
                    if let Some(page) = decode_page_image(image, pages.len(), chapter_key) {
                        pages.push(page);
                    }
                }
            }
        }
    }
    pages
}

fn decode_page_image(bytes: &[u8], index: usize, chapter_key: &str) -> Option<MangaPage> {
    let mut image_url = None;
    let mut iv = String::new();
    let mut key = String::new();
    let mut extra = false;
    for field in ProtoReader::new(bytes) {
        match field {
            ProtoField::Bytes(1, value) => {
                image_url = Some(String::from_utf8_lossy(value).into_owned())
            }
            ProtoField::Bytes(3, value) => iv = String::from_utf8_lossy(value).into_owned(),
            ProtoField::Bytes(4, value) => key = String::from_utf8_lossy(value).into_owned(),
            ProtoField::Varint(7, value) => extra = value != 0,
            _ => {}
        }
    }
    if extra {
        return None;
    }
    let mut image = format!("{CDN_URL}{}", image_url?);
    if !key.is_empty() || !iv.is_empty() {
        image.push_str(&format!(
            "?key={}&iv={}",
            url::query_escape(&key),
            url::query_escape(&iv)
        ));
    }
    Some(MangaPage {
        content: PageContent::Url {
            url: image,
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(&format!("{BASE_URL}{chapter_key}")),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    })
}

fn device_info() -> Vec<u8> {
    let mut out = Vec::new();
    write_varint_field(&mut out, 3, 2);
    out
}

fn search_request(query: &str, page: u64) -> Vec<u8> {
    let mut out = Vec::new();
    write_bytes_field(&mut out, 1, &device_info());
    write_bytes_field(&mut out, 2, query.as_bytes());
    write_varint_field(&mut out, 3, page);
    write_varint_field(&mut out, 4, 1);
    out
}

fn manga_list_request(tag_id: u64) -> Vec<u8> {
    let mut out = Vec::new();
    write_bytes_field(&mut out, 1, &device_info());
    write_varint_field(&mut out, 2, tag_id);
    out
}

fn day_of_week_request(day: u64) -> Vec<u8> {
    let mut out = Vec::new();
    write_bytes_field(&mut out, 1, &device_info());
    write_varint_field(&mut out, 2, day);
    out
}

fn manga_details_request(id: u64) -> Vec<u8> {
    let mut out = Vec::new();
    write_bytes_field(&mut out, 1, &device_info());
    write_varint_field(&mut out, 2, id);
    out
}

fn manga_viewer_request(id: u64) -> Vec<u8> {
    let mut out = Vec::new();
    write_bytes_field(&mut out, 1, &device_info());
    write_varint_field(&mut out, 2, id);
    write_varint_field(&mut out, 3, 0);
    let mut point = Vec::new();
    write_varint_field(&mut point, 1, 0);
    write_varint_field(&mut point, 2, 0);
    write_bytes_field(&mut out, 4, &point);
    let mut mode = Vec::new();
    write_varint_field(&mut mode, 1, 1);
    write_bytes_field(&mut out, 5, &mode);
    out
}

fn write_key(out: &mut Vec<u8>, field: u64, wire_type: u64) {
    write_varint(out, (field << 3) | wire_type);
}

fn write_varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
    write_key(out, field, 0);
    write_varint(out, value);
}

fn write_bytes_field(out: &mut Vec<u8>, field: u64, value: &[u8]) {
    write_key(out, field, 2);
    write_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

enum ProtoField<'a> {
    Varint(u64, u64),
    Bytes(u64, &'a [u8]),
    Other,
}

struct ProtoReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> ProtoReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn read_varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        let mut shift = 0;
        loop {
            let byte = *self.bytes.get(self.position)?;
            self.position += 1;
            value |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
            shift += 7;
            if shift > 63 {
                return None;
            }
        }
    }
}

impl<'a> Iterator for ProtoReader<'a> {
    type Item = ProtoField<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.position >= self.bytes.len() {
            return None;
        }
        let key = self.read_varint()?;
        let number = key >> 3;
        match key & 7 {
            0 => self
                .read_varint()
                .map(|value| ProtoField::Varint(number, value)),
            1 => {
                self.position = self.position.saturating_add(8).min(self.bytes.len());
                Some(ProtoField::Other)
            }
            2 => {
                let len = self.read_varint()? as usize;
                let start = self.position;
                let end = start.checked_add(len)?.min(self.bytes.len());
                self.position = end;
                Some(ProtoField::Bytes(number, &self.bytes[start..end]))
            }
            5 => {
                self.position = self.position.saturating_add(4).min(self.bytes.len());
                Some(ProtoField::Other)
            }
            _ => None,
        }
    }
}

fn trailing_id(key: &str) -> Option<u64> {
    key.trim_end_matches('/').rsplit('/').next()?.parse().ok()
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    if let Some(index) = path.find("/manga/") {
        format!(
            "/{}",
            path[index + 1..]
                .split('?')
                .next()
                .unwrap_or_default()
                .trim_end_matches('/')
        )
    } else {
        format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn key_from_url(input: &str) -> Option<String> {
    if input.contains(BASE_URL) || input.starts_with("/manga/") {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn hex_decode(input: &str) -> Vec<u8> {
    input
        .as_bytes()
        .chunks(2)
        .filter_map(|pair| {
            let hi = hex_value(*pair.first()?)?;
            let lo = hex_value(*pair.get(1)?)?;
            Some((hi << 4) | lo)
        })
        .collect()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0;
    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if byte == b'=' {
            break;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str =
    "0a1f0801120a53616d706c652046555a220a2f636f7665722e6a7067720b4465736372697074696f6e";
const SEARCH_FIXTURE: &str =
    "121f0801120a53616d706c652046555a220a2f636f7665722e6a7067720b4465736372697074696f6e3001";
const LATEST_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = "121f0801120a53616d706c652046555a220a2f636f7665722e6a7067720b4465736372697074696f6e1a15121308011a0f0a0d457069736f64652031205469746c65220b0a091207417574686f722a0a120846616e74617379";
const VIEWER_FIXTURE: &str = "1a170a150a0a2f70616765312e6a7067180022003a00";
