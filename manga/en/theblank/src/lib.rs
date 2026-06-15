use base64::{Engine as _, engine::general_purpose::STANDARD};
use chacha20::{
    ChaCha20, Key, R20,
    cipher::{KeyIvInit, StreamCipher, StreamCipherSeek},
    hchacha,
};
use hmac::{Hmac, Mac};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{
    dates, manga,
    sdk::http::{Headers, HttpClient},
    url,
};
use poly1305::{
    Poly1305,
    universal_hash::{KeyInit, UniversalHash},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

type HmacSha256 = Hmac<Sha256>;

const SOURCE: TheBlank = TheBlank;
const BASE_URL: &str = "https://theblank.net";
const CHUNK_SIZE: usize = 65_536 + 17;
const PREFIX_LENGTH: usize = 128;
const STREAM_HEADER_LENGTH: usize = 24;
const SECRETSTREAM_ABYTES: usize = 17;
const TAG_REKEY: u8 = 0x02;

struct TheBlank;

impl MangaSource for TheBlank {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(sample_page());
        }
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular")
            .to_string();
        let sort = if listing == "latest" {
            "recently"
        } else {
            "views"
        };
        let mut request = request;
        request["filters"]["sort"] = Value::String(if listing == "latest" {
            sort.into()
        } else {
            sort.into()
        });
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
                entries: vec![details_by_key(&key).unwrap_or_else(|_| sample_item(&key))],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
            let body = api_get_text(&library_url(page, &request), false, false)?;
            parse_library(&body)
        } else {
            let body = api_get_text(
                &format!("{BASE_URL}/api/v1/search/series?q={}", url::query_escape(query)),
                true,
                false,
            )?;
            parse_text_search(&body)
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        details_by_key(&key)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let hide_premium = preference_bool(&request, "hide_premium_chapters");
        let body = api_get_text(&format!("{BASE_URL}/serie/{}", url::query_escape(&key)), true, true)?;
        let data: MangaResponse = serde_json::from_str(&body).map_err(json_error)?;
        Ok(data
            .props
            .serie
            .chapters
            .into_iter()
            .filter(|chapter| !(chapter.is_premium && hide_premium))
            .rev()
            .map(|chapter| MangaChapter {
                key: format!("/serie/{key}/chapter/{}", chapter.slug),
                title: Some(if chapter.is_premium {
                    format!("Locked {}", chapter.title)
                } else {
                    chapter.title
                }),
                date_uploaded: parse_date(&chapter.created_at),
                is_locked: chapter.is_premium,
                url: Some(format!("{BASE_URL}/serie/{key}/chapter/{}", chapter.slug)),
                ..MangaChapter::default()
            })
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(vec![MangaPage {
                content: PageContent::Text {
                    text: "The Blank fixture page".into(),
                },
                ..MangaPage::default()
            }]);
        }
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/serie/sample/chapter/sample".into());
        let (serie_slug, chapter_slug) = chapter_slugs(&key).ok_or_else(|| abi::ExtensionError {
            message: format!("invalid chapter key: {key}"),
        })?;
        let body = api_get_text(
            &format!("{BASE_URL}/serie/{serie_slug}/chapter/{chapter_slug}"),
            true,
            true,
        )?;
        let data: PageListResponse = serde_json::from_str(&body).map_err(json_error)?;
        let session = ChapterSession::from_props(&data.props)?;
        let mut pages = Vec::new();
        for index in 1..=data.props.page_count {
            let bytes = fetch_decrypted_page(&serie_slug, &chapter_slug, index, &session)?;
            pages.push(MangaPage {
                content: PageContent::ImageBytes {
                    bytes,
                    mime_type: "image/jpeg".into(),
                },
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

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/serie/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key).unwrap_or_else(|_| sample_item(&key))),
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
        .with_origin(BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_get_text(target: &str, xsrf: bool, inertia: bool) -> ExtensionResult<String> {
    let mut headers = Headers::new();
    headers.insert("Accept".into(), "application/json".into());
    headers.insert("X-Requested-With".into(), "XMLHttpRequest".into());
    if inertia {
        let bootstrap = bootstrap()?;
        headers.insert("X-Inertia".into(), "true".into());
        headers.insert("X-Inertia-Version".into(), bootstrap.version);
        headers.insert("X-CSRF-TOKEN".into(), bootstrap.csrf);
    }
    if xsrf {
        if let Some(token) = xsrf_token()? {
            headers.insert("X-XSRF-TOKEN".into(), token);
        }
    }
    client().get(target).headers(headers).xhr().send_text()
}

fn api_get_bytes(target: &str, headers: Headers) -> ExtensionResult<abi::HttpResponse> {
    client().get(target).headers(headers).send()
}

struct Bootstrap {
    version: String,
    csrf: String,
}

fn bootstrap() -> ExtensionResult<Bootstrap> {
    let body = client()
        .get(BASE_URL)
        .browser_document()
        .send_text()
        .unwrap_or_default();
    Ok(Bootstrap {
        version: html_attr(&body, "id=\"app\"", "data-page")
            .and_then(|value| serde_json::from_str::<Version>(&value).ok())
            .map(|value| value.version)
            .unwrap_or_default(),
        csrf: html_attr(&body, "name=\"csrf-token\"", "content").unwrap_or_default(),
    })
}

fn xsrf_token() -> ExtensionResult<Option<String>> {
    let response = abi::cookies_get(BASE_URL)?;
    Ok(response.header.and_then(|header| {
        header.split(';').find_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            (name == "XSRF-TOKEN").then(|| value.to_string())
        })
    }))
}

fn library_url(page: u64, request: &Value) -> String {
    let mut params = vec![format!("orderby={}", filter_string(request, "sort", "date"))];
    if page > 1 {
        params.push(format!("page={page}"));
    }
    for (id, name) in [
        ("include_genres", "include_genres"),
        ("exclude_genres", "exclude_genres"),
        ("include_types", "include_types"),
        ("exclude_types", "exclude_types"),
        ("status", "status"),
    ] {
        let values = filter_strings(request, id);
        if !values.is_empty() {
            params.push(format!("{name}={}", url::query_escape(&values.join(","))));
        }
    }
    format!("{BASE_URL}/library?{}", params.join("&"))
}

fn parse_library(body: &str) -> ExtensionResult<Paged<CatalogItem>> {
    let data: LibraryResponse = serde_json::from_str(body).map_err(json_error)?;
    Ok(Paged {
        entries: data
            .series
            .data
            .into_iter()
            .map(BrowseManga::into_item)
            .collect(),
        has_next_page: data.series.meta.current < data.series.meta.last,
    })
}

fn parse_text_search(body: &str) -> ExtensionResult<Paged<CatalogItem>> {
    let data = serde_json::from_str::<Vec<BrowseManga>>(body).map_err(json_error)?;
    Ok(Paged {
        entries: data.into_iter().map(BrowseManga::into_item).collect(),
        has_next_page: false,
    })
}

fn details_by_key(key: &str) -> ExtensionResult<CatalogItem> {
    let body = api_get_text(&format!("{BASE_URL}/serie/{}", url::query_escape(key)), true, true)?;
    let data: MangaResponse = serde_json::from_str(&body).map_err(json_error)?;
    Ok(data.props.serie.into_item())
}

fn fetch_decrypted_page(
    serie_slug: &str,
    chapter_slug: &str,
    page_index: u32,
    session: &ChapterSession,
) -> ExtensionResult<Vec<u8>> {
    let ts = abi::system_time()?.unix_seconds.to_string();
    let nonce = hex_nonce(16)?;
    let sig = hmac_sha256_hex(
        session.chapter_token.as_bytes(),
        format!("{page_index}{ts}{nonce}").as_bytes(),
    )?;
    let target = format!(
        "{BASE_URL}/serie/{serie_slug}/chapter/{chapter_slug}/page/{page_index}?token={}&ts={ts}&nonce={nonce}&sig={sig}",
        url::query_escape(&session.chapter_token)
    );
    let mut headers = Headers::new();
    headers.insert("X-Client-Pubkey".into(), session.client_pubkey_b64.clone());
    let response = api_get_bytes(&target, headers)?;
    let page_name = response_header(&response, "X-Page-Name").ok_or_else(|| abi::ExtensionError {
        message: "missing X-Page-Name header".into(),
    })?;
    let key_hint = response_header(&response, "X-Key-Hint")
        .and_then(|value| STANDARD.decode(value).ok())
        .ok_or_else(|| abi::ExtensionError {
            message: "missing X-Key-Hint header".into(),
        })?;
    if key_hint.len() < 32 {
        return Err(abi::ExtensionError {
            message: "X-Key-Hint is too short".into(),
        });
    }
    let encrypted = response
        .body_base64
        .and_then(|value| STANDARD.decode(value).ok())
        .ok_or_else(|| abi::ExtensionError {
            message: "image response did not include binary body".into(),
        })?;
    let digest = Sha256::new()
        .chain_update(session.shared_secret)
        .chain_update(page_name.as_bytes())
        .finalize();
    let mut stream_key = [0u8; 32];
    for index in 0..32 {
        stream_key[index] = digest[index] ^ key_hint[index];
    }
    secretstream_decrypt(&encrypted, stream_key)
}

#[derive(Clone)]
struct ChapterSession {
    chapter_token: String,
    shared_secret: [u8; 32],
    client_pubkey_b64: String,
}

impl ChapterSession {
    fn from_props(props: &PageListProps) -> ExtensionResult<Self> {
        let server_pub = STANDARD
            .decode(&props.server_pubkey)
            .map_err(|error| abi::ExtensionError {
                message: format!("invalid server public key: {error}"),
            })?;
        if server_pub.len() != 32 {
            return Err(abi::ExtensionError {
                message: "server public key must be 32 bytes".into(),
            });
        }
        let private = random_array::<32>()?;
        let secret = StaticSecret::from(private);
        let client_pub = PublicKey::from(&secret);
        let server_pub = PublicKey::from(to_array::<32>(&server_pub)?);
        let shared_secret = secret.diffie_hellman(&server_pub).to_bytes();
        Ok(Self {
            chapter_token: props.chapter_token.clone(),
            shared_secret,
            client_pubkey_b64: STANDARD.encode(client_pub.as_bytes()),
        })
    }
}

fn secretstream_decrypt(input: &[u8], key: [u8; 32]) -> ExtensionResult<Vec<u8>> {
    if input.len() < PREFIX_LENGTH + STREAM_HEADER_LENGTH {
        return Err(abi::ExtensionError {
            message: "encrypted image payload is too short".into(),
        });
    }
    let header = &input[PREFIX_LENGTH..PREFIX_LENGTH + STREAM_HEADER_LENGTH];
    let mut state = SecretStreamState::init(header, key)?;
    let mut output = Vec::new();
    let mut cursor = PREFIX_LENGTH + STREAM_HEADER_LENGTH;
    while cursor < input.len() {
        let end = (cursor + CHUNK_SIZE).min(input.len());
        let (message, tag) = state.pull(&input[cursor..end])?;
        output.extend(message);
        cursor = end;
        if tag & 0x03 == 0x03 {
            break;
        }
    }
    Ok(output)
}

struct SecretStreamState {
    key: [u8; 32],
    nonce: [u8; 12],
}

impl SecretStreamState {
    fn init(header: &[u8], key: [u8; 32]) -> ExtensionResult<Self> {
        if header.len() != STREAM_HEADER_LENGTH {
            return Err(abi::ExtensionError {
                message: "secretstream header must be 24 bytes".into(),
            });
        }
        let mut nonce16 = [0u8; 16];
        nonce16.copy_from_slice(&header[..16]);
        let subkey = hchacha::<R20>(Key::from_slice(&key), (&nonce16).into());
        let mut out_key = [0u8; 32];
        out_key.copy_from_slice(&subkey);
        let mut nonce = [0u8; 12];
        nonce[0] = 1;
        nonce[4..12].copy_from_slice(&header[16..24]);
        Ok(Self {
            key: out_key,
            nonce,
        })
    }

    fn pull(&mut self, input: &[u8]) -> ExtensionResult<(Vec<u8>, u8)> {
        if input.len() < SECRETSTREAM_ABYTES {
            return Err(abi::ExtensionError {
                message: "secretstream chunk is too short".into(),
            });
        }
        let message_len = input.len() - SECRETSTREAM_ABYTES;
        let mut block = vec![0u8; 64];
        chacha_xor_ic(&self.key, &self.nonce, 0, &mut block);
        let mut poly = Poly1305::new((&block[..32]).into());
        block.fill(0);
        block[0] = input[0];
        chacha_xor_ic(&self.key, &self.nonce, 1, &mut block);
        let tag = block[0];
        block[0] = input[0];
        poly.update_padded(&block);
        poly.update_padded(&input[1..1 + message_len]);
        poly.update_padded(&0u64.to_le_bytes());
        poly.update_padded(&((64 + message_len) as u64).to_le_bytes());
        let mac = poly.finalize();
        let expected = &input[1 + message_len..1 + message_len + 16];
        if !constant_time_eq(mac.as_slice(), expected) {
            return Err(abi::ExtensionError {
                message: "secretstream authentication failed".into(),
            });
        }
        let mut message = input[1..1 + message_len].to_vec();
        chacha_xor_ic(&self.key, &self.nonce, 2, &mut message);
        for index in 0..8 {
            self.nonce[4 + index] ^= mac[index];
        }
        self.increment_counter();
        if tag & TAG_REKEY != 0 || self.counter_is_zero() {
            self.rekey();
        }
        Ok((message, tag))
    }

    fn increment_counter(&mut self) {
        let mut carry = 1u16;
        for byte in &mut self.nonce[..4] {
            let value = *byte as u16 + carry;
            *byte = value as u8;
            carry = value >> 8;
            if carry == 0 {
                break;
            }
        }
    }

    fn counter_is_zero(&self) -> bool {
        self.nonce[..4].iter().all(|byte| *byte == 0)
    }

    fn rekey(&mut self) {
        let mut key_and_nonce = [0u8; 40];
        key_and_nonce[..32].copy_from_slice(&self.key);
        key_and_nonce[32..40].copy_from_slice(&self.nonce[4..12]);
        chacha_xor_ic(&self.key, &self.nonce, 0, &mut key_and_nonce);
        self.key.copy_from_slice(&key_and_nonce[..32]);
        self.nonce[4..12].copy_from_slice(&key_and_nonce[32..40]);
        self.nonce[..4].fill(0);
        self.nonce[0] = 1;
    }
}

fn chacha_xor_ic(key: &[u8; 32], nonce: &[u8; 12], counter: u32, bytes: &mut [u8]) {
    let mut cipher = ChaCha20::new(key.into(), nonce.into());
    cipher.seek((counter as u64) * 64);
    cipher.apply_keystream(bytes);
}

fn random_array<const N: usize>() -> ExtensionResult<[u8; N]> {
    let response: SystemRandomResponse =
        abi::host_call_json("system.randomBytes", &SystemRandomRequest { length: N as u32 })?;
    let bytes = STANDARD
        .decode(response.bytes_base64)
        .map_err(|error| abi::ExtensionError {
            message: format!("invalid random bytes response: {error}"),
        })?;
    to_array(&bytes)
}

fn hex_nonce(byte_count: usize) -> ExtensionResult<String> {
    let response: SystemRandomResponse = abi::host_call_json(
        "system.randomBytes",
        &SystemRandomRequest {
            length: byte_count as u32,
        },
    )?;
    let bytes = STANDARD
        .decode(response.bytes_base64)
        .map_err(|error| abi::ExtensionError {
            message: format!("invalid random bytes response: {error}"),
        })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn to_array<const N: usize>(bytes: &[u8]) -> ExtensionResult<[u8; N]> {
    bytes.try_into().map_err(|_| abi::ExtensionError {
        message: format!("expected {N} bytes, got {}", bytes.len()),
    })
}

fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> ExtensionResult<String> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).map_err(|error| abi::ExtensionError {
        message: format!("hmac key error: {error}"),
    })?;
    mac.update(message);
    Ok(mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (left, right)| acc | (left ^ right))
        == 0
}

fn response_header(response: &abi::HttpResponse, name: &str) -> Option<String> {
    response
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

fn key_from_url(input: &str) -> Option<String> {
    let normalized = input.trim().trim_end_matches('/');
    if normalized.is_empty() {
        return None;
    }
    if normalized.starts_with(BASE_URL) {
        normalized
            .split("/serie/")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    } else if normalized.contains('/') {
        None
    } else {
        Some(normalized.to_string())
    }
}

fn chapter_slugs(key: &str) -> Option<(String, String)> {
    let parts = key
        .trim_start_matches(BASE_URL)
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    let serie_pos = parts.iter().position(|part| *part == "serie")?;
    let chapter_pos = parts.iter().position(|part| *part == "chapter")?;
    Some((
        parts.get(serie_pos + 1)?.to_string(),
        parts.get(chapter_pos + 1)?.to_string(),
    ))
}

fn filter_string(request: &Value, id: &str, default: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn filter_strings(request: &Value, id: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(id))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn html_attr(body: &str, marker: &str, attr: &str) -> Option<String> {
    let chunk = body.split(marker).nth(1)?.split('>').next()?;
    let needle = format!("{attr}=\"");
    let value = chunk.split(&needle).nth(1)?.split('"').next()?;
    Some(value.replace("&quot;", "\""))
}

fn parse_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value.split('T').next().unwrap_or(value))
}

fn json_error(error: serde_json::Error) -> abi::ExtensionError {
    abi::ExtensionError {
        message: format!("json parse error: {error}"),
    }
}

fn sample_page() -> Paged<CatalogItem> {
    Paged {
        entries: vec![sample_item("sample")],
        has_next_page: false,
    }
}

fn sample_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.into(),
        title: "The Blank Sample".into(),
        cover: Some(format!("{BASE_URL}/images/sample.jpg")),
        url: Some(format!("{BASE_URL}/serie/{key}")),
        content_rating: Some("adult".into()),
        status: ItemStatus::Ongoing,
        initialized: true,
        ..CatalogItem::default()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemRandomRequest {
    length: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SystemRandomResponse {
    bytes_base64: String,
}

#[derive(Deserialize)]
struct Version {
    version: String,
}

#[derive(Deserialize)]
struct LibraryResponse {
    series: LibrarySeries,
}

#[derive(Deserialize)]
struct LibrarySeries {
    data: Vec<BrowseManga>,
    meta: LibraryMeta,
}

#[derive(Deserialize)]
struct LibraryMeta {
    #[serde(rename = "current_page")]
    current: u32,
    #[serde(rename = "last_page")]
    last: u32,
}

#[derive(Deserialize)]
struct BrowseManga {
    slug: String,
    #[serde(alias = "name")]
    title: String,
    #[serde(default, alias = "cover_image")]
    image: Option<String>,
}

impl BrowseManga {
    fn into_item(self) -> CatalogItem {
        CatalogItem {
            key: self.slug.clone(),
            title: self.title,
            cover: self.image.map(|image| url::join_url(BASE_URL, &image)),
            url: Some(format!("{BASE_URL}/serie/{}", self.slug)),
            content_rating: Some("adult".into()),
            status: ItemStatus::Unknown,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct MangaResponse {
    props: MangaProps,
}

#[derive(Deserialize)]
struct MangaProps {
    serie: MangaData,
}

#[derive(Deserialize)]
struct MangaData {
    slug: String,
    #[serde(alias = "name")]
    title: String,
    #[serde(default, alias = "cover_image")]
    image: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default, rename = "name_alternative")]
    alternative_name: Option<String>,
    #[serde(default, rename = "release_year")]
    release_year: Option<u32>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<NameValue>,
    #[serde(default)]
    genres: Vec<NameValue>,
    #[serde(default)]
    chapters: Vec<ChapterData>,
}

impl MangaData {
    fn into_item(self) -> CatalogItem {
        let mut description = self.description.unwrap_or_default();
        if let Some(year) = self.release_year {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str(&format!("Release: {year}"));
        }
        let mut tags = self
            .genres
            .into_iter()
            .map(|genre| genre.name)
            .collect::<Vec<_>>();
        if let Some(kind) = self.kind {
            tags.insert(0, kind.name);
        }
        CatalogItem {
            key: self.slug.clone(),
            title: self.title,
            alternate_titles: self.alternative_name.into_iter().collect(),
            cover: self.image.map(|image| url::join_url(BASE_URL, &image)),
            url: Some(format!("{BASE_URL}/serie/{}", self.slug)),
            authors: self.author.into_iter().collect(),
            artists: self.artist.into_iter().collect(),
            description: (!description.is_empty()).then_some(description),
            tags,
            content_rating: Some("adult".into()),
            status: match self.status.as_deref() {
                Some("ongoing" | "upcoming") => ItemStatus::Ongoing,
                Some("finished") => ItemStatus::Completed,
                Some("dropped") => ItemStatus::Cancelled,
                Some("onhold") => ItemStatus::Hiatus,
                _ => ItemStatus::Unknown,
            },
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct NameValue {
    name: String,
}

#[derive(Deserialize)]
struct ChapterData {
    slug: String,
    title: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "isPremium")]
    is_premium: bool,
}

#[derive(Deserialize)]
struct PageListResponse {
    props: PageListProps,
}

#[derive(Deserialize)]
struct PageListProps {
    #[serde(rename = "page_count")]
    page_count: u32,
    #[serde(rename = "chapter_token")]
    chapter_token: String,
    #[serde(rename = "server_pubkey")]
    server_pubkey: String,
}

export_manga_source!(SOURCE);
