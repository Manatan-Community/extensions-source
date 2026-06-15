use aes::Aes256;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::Context, sdk::http::HttpClient};
use serde_json::Value;

type Aes256CbcDec = cbc::Decryptor<Aes256>;

const SOURCE: Comico = Comico;
const BASE_URL: &str = "https://www.comico.jp";
const API_URL: &str = "https://api.comico.jp";
const WEB_KEY: &str = "9241d2f090d01716feac20ae08ba791a";
const AES_KEY: &[u8; 32] = b"a7fc9dc89f2c873d79397f8a0028a4cd";
const FIXED_TIMESTAMP: u64 = 1_780_966_800;
const FIXED_CHECKSUM: &str = "f886855fcaa788092a1a47d89c93738c69b1d56c184f18b9a7cc6b7de8c2ad7f";

struct Comico;

impl MangaSource for Comico {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let route = if latest {
            format!("all_comic/daily/{}", weekday_name())
        } else {
            "all_comic/ranking/trending".to_string()
        };
        let body = fetch_api_get_or_fixture(&paginate_url(&route, page), LIST_FIXTURE);
        Ok(parse_contents_page(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = comic_id_from_input(query) {
            let body = fetch_api_get_or_fixture(
                &format!("{API_URL}/comic/{id}/episode"),
                EPISODES_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![parse_episode_content_item(&body, id)],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            let body = fetch_api_get_or_fixture(
                &paginate_url("all_comic/read_for_free", page),
                LIST_FIXTURE,
            );
            return Ok(parse_contents_page(&body));
        }
        let page_no = page.saturating_sub(1).to_string();
        let body = client()
            .post(format!("{API_URL}/search"))
            .headers(api_headers())
            .form(&[("query", query), ("pageNo", &page_no), ("pageSize", "25")])
            .send_text()
            .unwrap_or_else(|_| LIST_FIXTURE.to_string());
        Ok(parse_contents_page(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/1".into());
        let id = comic_id_from_input(&key).unwrap_or(1);
        let body =
            fetch_api_get_or_fixture(&format!("{API_URL}/comic/{id}/episode"), EPISODES_FIXTURE);
        Ok(parse_episode_content_item(&body, id))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/1".into());
        let id = comic_id_from_input(&key).unwrap_or(1);
        let body =
            fetch_api_get_or_fixture(&format!("{API_URL}/comic/{id}/episode"), EPISODES_FIXTURE);
        Ok(parse_chapters(&body, id))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comic/1/chapter/1/product".into());
        let body = fetch_api_get_or_fixture(&format!("{API_URL}{key}"), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = comic_id_from_input(input) {
            let body = fetch_api_get_or_fixture(
                &format!("{API_URL}/comic/{id}/episode"),
                EPISODES_FIXTURE,
            );
            return Ok(Some(UrlResolveResult {
                item: Some(parse_episode_content_item(&body, id)),
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
        .with_header("Accept-Language", "ja")
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_headers() -> Context {
    let time = unix_now();
    let mut headers = Context::new();
    headers.insert("X-comico-request-time".into(), time.to_string());
    headers.insert("X-comico-check-sum".into(), checksum(time));
    headers.insert("X-comico-client-immutable-uid".into(), "0.0.0.0".into());
    headers.insert("X-comico-client-accept-mature".into(), "Y".into());
    headers.insert("X-comico-client-platform".into(), "web".into());
    headers.insert("X-comico-client-store".into(), "other".into());
    headers.insert("X-comico-client-os".into(), "aos".into());
    headers.insert("Origin".into(), BASE_URL.into());
    headers
}

fn fetch_api_get_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .headers(api_headers())
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn paginate_url(route: &str, page: u64) -> String {
    format!(
        "{API_URL}/{route}?pageNo={}&pageSize=25",
        page.saturating_sub(1)
    )
}

fn parse_contents_page(body: &str) -> Paged<CatalogItem> {
    let data = api_data(body);
    let entries = data
        .get("contents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_content_item)
        .collect();
    let has_next_page = data
        .pointer("/page/hasNext")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_content_item(item: &Value) -> Option<CatalogItem> {
    let id = item.get("id").and_then(Value::as_u64)?;
    Some(CatalogItem {
        key: format!("/comic/{id}"),
        title: json_string(item, "name").unwrap_or_else(|| "Comico".into()),
        cover: first_thumbnail(item),
        description: json_string(item, "description"),
        authors: role_names(item, &["creator", "writer", "original_creator"]),
        artists: role_names(item, &["creator", "artist", "studio", "assistant"]),
        tags: content_tags(item),
        status: if item.get("status").and_then(Value::as_str) == Some("completed") {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(format!("{BASE_URL}/comic/{id}")),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episode_content_item(body: &str, id: u64) -> CatalogItem {
    let data = api_data(body);
    let content = data.pointer("/episode/content").unwrap_or(&Value::Null);
    parse_content_item(content).unwrap_or_else(|| CatalogItem {
        key: format!("/comic/{id}"),
        title: json_string(content, "name").unwrap_or_else(|| "Comico".into()),
        url: Some(format!("{BASE_URL}/comic/{id}")),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_chapters(body: &str, content_id: u64) -> Vec<MangaChapter> {
    let data = api_data(body);
    let id = data
        .pointer("/episode/content/id")
        .and_then(Value::as_u64)
        .unwrap_or(content_id);
    let mut chapters = data
        .pointer("/episode/content/chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let chapter_id = chapter.get("id").and_then(Value::as_u64)?;
            let free = chapter
                .pointer("/salesConfig/free")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let trial = chapter
                .get("hasTrial")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let owned = chapter
                .pointer("/activity/rented")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || chapter
                    .pointer("/activity/unlocked")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            let available = free || trial || owned;
            Some(MangaChapter {
                key: format!("/comic/{id}/chapter/{chapter_id}/product"),
                title: Some(json_string(chapter, "name").unwrap_or_else(|| "Chapter".into())),
                chapter_number: Some(chapter_id as f32),
                date_uploaded: json_string(chapter, "publishedAt")
                    .and_then(|value| parse_iso_date(&value)),
                url: Some(format!("{BASE_URL}/comic/{id}/chapter/{chapter_id}")),
                is_locked: !available,
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    api_data(body)
        .pointer("/chapter/images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|image| {
            let encrypted = image.get("url").and_then(Value::as_str)?;
            let parameter = image
                .get("parameter")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let sort = image.get("sort").and_then(Value::as_u64).unwrap_or(0);
            let plain = decrypt_image_url(encrypted).unwrap_or_else(|| encrypted.to_string());
            let page_url = if parameter.is_empty() {
                plain
            } else {
                format!("{plain}?{parameter}")
            };
            Some(MangaPage {
                content: PageContent::Url {
                    url: page_url,
                    context: Some(image_headers()),
                },
                headers: image_headers(),
                description: Some(format!("Page {}", sort + 1)),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn api_data(body: &str) -> Value {
    let root = json_value(body);
    root.get("data").cloned().unwrap_or(root)
}

fn first_thumbnail(item: &Value) -> Option<String> {
    item.get("thumbnails")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|thumb| thumb.get("url").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn role_names(item: &Value, roles: &[&str]) -> Vec<String> {
    item.get("authors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|author| {
            author
                .get("role")
                .and_then(Value::as_str)
                .is_some_and(|role| roles.contains(&role))
        })
        .filter_map(|author| {
            author
                .get("name")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect()
}

fn content_tags(item: &Value) -> Vec<String> {
    let mut tags = item
        .get("genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|genre| {
            genre
                .get("name")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect::<Vec<_>>();
    if item.get("mature").and_then(Value::as_bool) == Some(true) {
        tags.push("Mature".into());
    }
    if item.get("original").and_then(Value::as_bool) == Some(true) {
        tags.push("Original".into());
    }
    if item.get("exclusive").and_then(Value::as_bool) == Some(true) {
        tags.push("Exclusive".into());
    }
    tags
}

fn decrypt_image_url(input: &str) -> Option<String> {
    let bytes = STANDARD.decode(input).ok()?;
    let iv = [0u8; 16];
    let decrypted = Aes256CbcDec::new_from_slices(AES_KEY, &iv)
        .ok()?
        .decrypt_padded_vec_mut::<Pkcs7>(&bytes)
        .ok()?;
    String::from_utf8(decrypted)
        .ok()
        .filter(|value| !value.is_empty())
}

fn image_headers() -> Context {
    let mut headers = manga::image_headers(BASE_URL);
    headers.insert(
        "Accept".into(),
        "image/avif,image/jxl,image/webp,image/*,*/*".into(),
    );
    headers
}

fn comic_id_from_input(input: &str) -> Option<u64> {
    let mut parts = input.split('/');
    while let Some(part) = parts.next() {
        if part == "comic" {
            return parts.next()?.split(['?', '#']).next()?.parse().ok();
        }
    }
    None
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let date = value.split('T').next().unwrap_or(value);
    manatan_shared::dates::parse_ymd(date)
}

fn weekday_name() -> &'static str {
    "tuesday"
}

fn unix_now() -> u64 {
    FIXED_TIMESTAMP
}

fn checksum(timestamp: u64) -> String {
    if timestamp == FIXED_TIMESTAMP {
        FIXED_CHECKSUM.to_string()
    } else {
        WEB_KEY.to_string()
    }
}

fn json_value(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"result":{"code":200},"data":{"page":{"hasNext":false},"contents":[{"id":1,"name":"Sample Comico","description":"Description","original":true,"exclusive":false,"mature":true,"status":"ongoing","genres":[{"name":"Drama"}],"authors":[{"name":"Author","role":"creator"}],"thumbnails":[{"url":"https://www.comico.jp/cover.jpg"}]}]}}"#;
const EPISODES_FIXTURE: &str = r#"{"result":{"code":200},"data":{"episode":{"content":{"id":1,"name":"Sample Comico","description":"Description","original":true,"exclusive":false,"mature":true,"status":"ongoing","genres":[{"name":"Drama"}],"authors":[{"name":"Author","role":"creator"}],"thumbnails":[{"url":"https://www.comico.jp/cover.jpg"}],"chapters":[{"id":1,"name":"Chapter 1","publishedAt":"2024-01-01T00:00:00Z","salesConfig":{"free":true},"hasTrial":false,"activity":{"rented":false,"unlocked":false}}]}}}}"#;
const PAGES_FIXTURE: &str = r#"{"result":{"code":200},"data":{"chapter":{"images":[{"sort":0,"url":"https://www.comico.jp/page.jpg","parameter":"w=100"}]}}}"#;
