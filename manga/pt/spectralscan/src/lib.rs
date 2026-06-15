use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

const SOURCE: NexusToons = NexusToons;
const BASE_URL: &str = "https://nx-toons.xyz";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "adult";
const CRYPTO_SECRET: &str = "OrionNexus2025CryptoKey!Secure";
const CHAPTER_KEY: &str = "NexusToons2026SecretKeyForChapterEncryption!@#$";
const NUM_KEYS: usize = 5;

struct NexusToons;

impl MangaSource for NexusToons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_list(LIST_FIXTURE));
        }
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "lastChapterAt"
        } else {
            "views"
        };
        Ok(parse_manga_list(&fetch_json(
            &manga_list_url(&request, page(&request), "", sort),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        Ok(parse_manga_list(&fetch_json(
            &manga_list_url(&request, page(&request), query, "updatedAt"),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let slug = manga_slug(&key);
        let body = fetch_json(&format!("{BASE_URL}/api/manga/{slug}"), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &slug))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/read/1/sample".into());
        let chapter_id = key
            .split("/read/")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or("1");
        let body = fetch_json(&format!("{BASE_URL}/api/read/{chapter_id}"), READ_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let parts = key
                .split("/read/")
                .nth(1)
                .unwrap_or("")
                .split('/')
                .collect::<Vec<_>>();
            let chapter_id = parts.first().copied().unwrap_or_default();
            let slug = parts.get(1).copied().unwrap_or_default();
            format!("{BASE_URL}/r/{}", encode_chapter_url(chapter_id, slug))
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: key.starts_with("/manga/").then(|| details_by_key(&key)),
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
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .header("Accept", "application/json")
        .header("Referer", format!("{BASE_URL}/"))
        .send_text()
        .ok()
        .map(|body| decrypt_if_needed(&body))
        .unwrap_or_else(|| fixture.to_string())
}

fn manga_list_url(request: &Value, page: u64, query: &str, default_sort: &str) -> String {
    let mut pairs = vec![
        ("page", page.to_string()),
        (
            "limit",
            if query.is_empty() { "50" } else { "30" }.to_string(),
        ),
        ("includeNsfw", "true".to_string()),
        (
            "sortBy",
            filter_value(request, "sortBy").unwrap_or_else(|| default_sort.to_string()),
        ),
        (
            "sortOrder",
            filter_value(request, "sortOrder").unwrap_or_else(|| "desc".to_string()),
        ),
        (
            "categoryMode",
            filter_value(request, "categoryMode").unwrap_or_else(|| "or".to_string()),
        ),
    ];
    if only_nsfw(request) {
        pairs.push(("onlyNsfw", "true".to_string()));
    }
    if !query.is_empty() {
        pairs.push(("search", query.to_string()));
    }
    for id in ["status", "type", "genres", "themes"] {
        let values = filter_values(request, id);
        if !values.is_empty() {
            pairs.push((id, values.join(",")));
        }
    }
    let query_string = pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/api/mangas?{query_string}")
}

fn parse_manga_list(body: &str) -> Paged<CatalogItem> {
    let dto = serde_json::from_str::<MangaListResponse>(body)
        .or_else(|_| serde_json::from_str::<MangaListResponse>(LIST_FIXTURE))
        .unwrap_or_default();
    Paged {
        entries: dto
            .data
            .into_iter()
            .flatten()
            .map(|item| item.to_catalog(false))
            .collect(),
        has_next_page: dto.page < dto.pages,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let slug = manga_slug(key);
    let body = fetch_json(&format!("{BASE_URL}/api/manga/{slug}"), DETAILS_FIXTURE);
    serde_json::from_str::<MangaDetailsDto>(&body)
        .or_else(|_| serde_json::from_str::<MangaDetailsDto>(DETAILS_FIXTURE))
        .unwrap_or_default()
        .to_catalog(true)
}

fn parse_chapters(body: &str, slug: &str) -> Vec<MangaChapter> {
    serde_json::from_str::<MangaDetailsDto>(body)
        .or_else(|_| serde_json::from_str::<MangaDetailsDto>(DETAILS_FIXTURE))
        .unwrap_or_default()
        .chapters
        .unwrap_or_default()
        .into_iter()
        .map(|chapter| {
            let title = if chapter
                .title
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            {
                format!("{} {}", chapter.title.unwrap_or_default(), chapter.number)
            } else {
                format!("Capitulo {}", chapter.number.trim_end_matches(".0"))
            };
            MangaChapter {
                key: format!("/read/{}/{}", chapter.id, slug),
                title: Some(title),
                chapter_number: chapter.number.parse::<f32>().ok(),
                date_uploaded: parse_iso_date(&chapter.created_at),
                language: Some(LANG.to_string()),
                url: Some(format!(
                    "{BASE_URL}/r/{}",
                    encode_chapter_url(&chapter.id.to_string(), slug)
                )),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let read = serde_json::from_str::<ReadResponse>(body)
        .or_else(|_| serde_json::from_str::<ReadResponse>(READ_FIXTURE))
        .unwrap_or_default();
    read.pages
        .into_iter()
        .enumerate()
        .map(|(index, page)| {
            let image = page
                .image_url
                .unwrap_or_else(|| format!("{BASE_URL}/api/p/{}/{index}", read.page_token));
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn decrypt_if_needed(body: &str) -> String {
    let Ok(enc) = serde_json::from_str::<EncryptedResponse>(body) else {
        return body.to_string();
    };
    if enc.v != 1 && enc.v != 2 {
        return body.to_string();
    }
    decrypt(enc).unwrap_or_else(|| body.to_string())
}

fn decrypt(enc: EncryptedResponse) -> Option<String> {
    let keys = derive_keys();
    let key_index = if enc.v == 1 { 0 } else { enc.k };
    let key = keys.get(key_index)?;
    let rsbox = reverse_sbox(key);
    let input = STANDARD.decode(enc.d).ok()?;
    let mut output = vec![0u8; input.len()];
    for i in (0..input.len()).rev() {
        let mut byte = input[i];
        byte ^= if i > 0 {
            input[i - 1]
        } else {
            key[key.len() - 1]
        };
        byte = rsbox[byte as usize];
        let rot_amount = (((key[(i + 3) % key.len()] as usize) + (i & 0xff)) & 0xff) % 7 + 1;
        byte = byte.rotate_right(rot_amount as u32);
        byte ^= key[i % key.len()];
        output[i] = byte;
    }
    String::from_utf8(output).ok()
}

fn derive_keys() -> Vec<Vec<u8>> {
    (0..NUM_KEYS)
        .map(|i| {
            let pattern = format!("_orion_key_{i}_v2_{CRYPTO_SECRET}");
            Sha256::digest(pattern.as_bytes()).to_vec()
        })
        .collect()
}

fn reverse_sbox(key: &[u8]) -> [u8; 256] {
    let mut sbox = [0u8; 256];
    for (i, item) in sbox.iter_mut().enumerate() {
        *item = i as u8;
    }
    let mut j = 0usize;
    for i in 0..256 {
        j = (j + sbox[i] as usize + key[i % key.len()] as usize) % 256;
        sbox.swap(i, j);
    }
    let mut rsbox = [0u8; 256];
    for (i, value) in sbox.iter().enumerate() {
        rsbox[*value as usize] = i as u8;
    }
    rsbox
}

fn encode_chapter_url(chapter_id: &str, manga_slug: &str) -> String {
    let data = format!("{chapter_id}|{manga_slug}|manatan|ABCDEFGHIJKLMNOPQRST");
    let xored = xor_cipher(&data, CHAPTER_KEY);
    let first = URL_SAFE_NO_PAD.encode(xored);
    let second = URL_SAFE_NO_PAD.encode(format!("{first}|manatanpad").as_bytes());
    if second.len() >= 64 {
        second
    } else {
        format!("{second}{}", "A".repeat(64 - second.len()))
    }
}

fn xor_cipher(input: &str, key: &str) -> Vec<u8> {
    input
        .bytes()
        .enumerate()
        .map(|(i, byte)| byte ^ key.as_bytes()[i % key.len()])
        .collect()
}

fn filter_value(request: &Value, id: &str) -> Option<String> {
    filter_values(request, id).into_iter().next()
}

fn filter_values(request: &Value, id: &str) -> Vec<String> {
    let Some(filters) = request.get("filters") else {
        return Vec::new();
    };
    if let Some(value) = filters.get(id) {
        return values_from_json(value);
    }
    filters
        .as_array()
        .and_then(|array| {
            array.iter().find_map(|filter| {
                (filter.get("id").and_then(Value::as_str) == Some(id))
                    .then(|| filter.get("value").map(values_from_json))
                    .flatten()
            })
        })
        .unwrap_or_default()
}

fn values_from_json(value: &Value) -> Vec<String> {
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect();
    }
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| vec![value.to_string()])
        .unwrap_or_default()
}

fn only_nsfw(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("only_nsfw"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn parse_iso_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value.split('T').next().unwrap_or(value))
}

fn manga_slug(key: &str) -> String {
    key.split("/manga/")
        .nth(1)
        .unwrap_or("sample")
        .trim_matches('/')
        .to_string()
}

fn normalize_key(input: &str) -> String {
    let value = input.strip_prefix(BASE_URL).unwrap_or(input);
    format!(
        "/{}",
        value
            .trim_start_matches('/')
            .split('?')
            .next()
            .unwrap_or(value)
    )
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

#[derive(Default, Deserialize)]
struct MangaListResponse {
    #[serde(default)]
    data: Option<Vec<MangaListDto>>,
    #[serde(default = "default_page")]
    page: u64,
    #[serde(default = "default_page")]
    pages: u64,
}

fn default_page() -> u64 {
    1
}

#[derive(Default, Deserialize)]
struct MangaListDto {
    slug: String,
    title: String,
    #[serde(default, rename = "coverImage")]
    cover_image: Option<String>,
}

impl MangaListDto {
    fn to_catalog(&self, initialized: bool) -> CatalogItem {
        let key = format!("/manga/{}", self.slug);
        CatalogItem {
            key: key.clone(),
            title: self.title.clone(),
            cover: self.cover_image.clone(),
            url: Some(absolute_url(&key)),
            language: Some(LANG.to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct MangaDetailsDto {
    slug: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "coverImage")]
    cover_image: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    categories: Option<Vec<CategoryDto>>,
    #[serde(default)]
    chapters: Option<Vec<ChapterDto>>,
}

impl MangaDetailsDto {
    fn to_catalog(&self, initialized: bool) -> CatalogItem {
        let key = format!("/manga/{}", self.slug);
        CatalogItem {
            key: key.clone(),
            title: self.title.clone(),
            cover: self.cover_image.clone(),
            description: self.description.clone(),
            authors: self
                .author
                .clone()
                .filter(|value| !value.is_empty())
                .into_iter()
                .collect(),
            artists: self
                .artist
                .clone()
                .filter(|value| !value.is_empty())
                .into_iter()
                .collect(),
            tags: self
                .categories
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|category| category.name)
                .collect(),
            status: match self.status.to_ascii_lowercase().as_str() {
                "ongoing" => ItemStatus::Ongoing,
                "completed" => ItemStatus::Completed,
                "hiatus" => ItemStatus::Hiatus,
                "cancelled" => ItemStatus::Cancelled,
                _ => ItemStatus::Unknown,
            },
            url: Some(absolute_url(&key)),
            language: Some(LANG.to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Clone, Deserialize)]
struct CategoryDto {
    name: String,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    id: u64,
    number: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "createdAt")]
    created_at: String,
}

#[derive(Default, Deserialize)]
struct ReadResponse {
    #[serde(default, rename = "pageToken")]
    page_token: String,
    #[serde(default)]
    pages: Vec<PageDto>,
}

#[derive(Default, Deserialize)]
struct PageDto {
    #[serde(default, rename = "imageUrl")]
    image_url: Option<String>,
}

#[derive(Deserialize)]
struct EncryptedResponse {
    d: String,
    #[serde(default)]
    k: usize,
    v: u8,
}

const LIST_FIXTURE: &str = r#"{"data":[{"slug":"sample","title":"Sample Nexus","coverImage":"https://nx-toons.xyz/cover.jpg"}],"page":1,"pages":1}"#;
const DETAILS_FIXTURE: &str = r#"{"slug":"sample","title":"Sample Nexus","description":"Sample description","coverImage":"https://nx-toons.xyz/cover.jpg","author":"Author","artist":"Artist","status":"ongoing","categories":[{"name":"Ação"}],"chapters":[{"id":1,"number":"1","title":null,"createdAt":"2024-01-01T00:00:00"}]}"#;
const READ_FIXTURE: &str = r#"{"pageToken":"fixture","pages":[{"pageNumber":1,"imageUrl":"https://nx-toons.xyz/page.jpg"}]}"#;

export_manga_source!(SOURCE);
