use aes::Aes128;
use base64::{Engine, engine::general_purpose::STANDARD};
use cbc::{
    Decryptor,
    cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7},
};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source, http,
    source::MangaSource,
};
use prost::Message;
use serde_json::Value;
use std::collections::BTreeMap;

const BASE_URL: &str = "https://global.manga-up.com";
const API_URL: &str = "https://global-api.manga-up.com/api";
const IMG_URL: &str = "https://global-img.manga-up.com";
const SOURCE: MangaUp = MangaUp;

struct MangaUp;

impl MangaSource for MangaUp {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let lang = source_lang(&request);
        let endpoint = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "home_v2"
        } else {
            "search"
        };
        let bytes =
            fetch_proto_or_fixture(&api_url(endpoint, &[("lang", lang)]), fixture_popular());
        if endpoint == "home_v2" {
            Ok(parse_home(&bytes, lang))
        } else {
            Ok(parse_popular(&bytes, lang))
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let lang = source_lang(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = deeplink_id(query) {
            let bytes = fetch_proto_or_fixture(
                &api_url(
                    "manga/detail_v2",
                    &[("title_id", &id), ("quality", "high"), ("ui_lang", lang)],
                ),
                fixture_details(),
            );
            return Ok(Paged {
                entries: vec![parse_details_proto(&bytes, &id, lang)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let genre = filter_string(filters, "genre").unwrap_or_default();
        let (endpoint, params) = if !query.is_empty() {
            ("manga/search", vec![("lang", lang), ("word", query)])
        } else if genre == "favorites" || genre == "history" {
            ("my_page", vec![("lang", lang)])
        } else if !genre.is_empty() {
            ("manga/tag", vec![("lang", lang), ("tag_id", genre)])
        } else {
            ("search", vec![("lang", lang)])
        };
        let bytes = fetch_proto_or_fixture(&api_url(endpoint, &params), fixture_popular());
        if genre == "favorites" || genre == "history" {
            Ok(parse_my_page(&bytes, genre, lang))
        } else if endpoint == "manga/search" || endpoint == "manga/tag" {
            Ok(parse_search(&bytes, lang))
        } else {
            Ok(parse_popular(&bytes, lang))
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let lang = source_lang(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "/manga/100".into());
        let id = key.rsplit('/').next().unwrap_or("100");
        let bytes = fetch_proto_or_fixture(
            &api_url(
                "manga/detail_v2",
                &[("title_id", id), ("quality", "high"), ("ui_lang", lang)],
            ),
            fixture_details(),
        );
        Ok(parse_details_proto(&bytes, id, lang))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let lang = source_lang(&request);
        let hide_paid = preference_bool(&request, "hidePaidChapters", false);
        let key = request_key(&request, "manga").unwrap_or_else(|| "/manga/100".into());
        let id = key.rsplit('/').next().unwrap_or("100");
        let bytes = fetch_proto_or_fixture(
            &api_url(
                "manga/detail_v2",
                &[("title_id", id), ("quality", "high"), ("ui_lang", lang)],
            ),
            fixture_details(),
        );
        Ok(parse_chapters(&bytes, id, lang, hide_paid))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let lang = source_lang(&request);
        let key = request_key(&request, "chapter").unwrap_or_else(|| "/manga/100/200".into());
        let chapter_id = key.rsplit('/').next().unwrap_or("200");
        let bytes = fetch_proto_or_fixture(
            &api_url(
                "manga/viewer_v2",
                &[
                    ("chapter_id", chapter_id),
                    ("quality", "high"),
                    ("lang", lang),
                ],
            ),
            fixture_viewer(),
        );
        Ok(parse_pages(&bytes))
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let image_base64 = request
            .get("imageBase64")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(page) = request.get("page") else {
            return Ok(ProcessedImage {
                image_base64: image_base64.into(),
                mime_type: request
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                ..ProcessedImage::default()
            });
        };
        let key = page
            .get("extra")
            .and_then(|extra| extra.get("key"))
            .and_then(Value::as_str);
        let iv = page
            .get("extra")
            .and_then(|extra| extra.get("iv"))
            .and_then(Value::as_str);
        let (Some(key), Some(iv)) = (key, iv) else {
            return Ok(ProcessedImage {
                image_base64: image_base64.into(),
                mime_type: request
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                ..ProcessedImage::default()
            });
        };
        let image = STANDARD.decode(image_base64).unwrap_or_default();
        let decrypted = aes_cbc_decrypt(&image, key, iv).unwrap_or(image);
        Ok(ProcessedImage {
            image_base64: STANDARD.encode(decrypted),
            mime_type: request
                .get("mimeType")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            ..ProcessedImage::default()
        })
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let lang = source_lang(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = deeplink_id(input) {
            let bytes = fetch_proto_or_fixture(
                &api_url(
                    "manga/detail_v2",
                    &[("title_id", &id), ("quality", "high"), ("ui_lang", lang)],
                ),
                fixture_details(),
            );
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details_proto(&bytes, &id, lang)),
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

export_manga_source!(SOURCE);

#[derive(Clone, PartialEq, Message)]
struct PopularResponse {
    #[prost(message, repeated, tag = "2")]
    titles: Vec<MangaTitle>,
}

#[derive(Clone, PartialEq, Message)]
struct SearchResponse {
    #[prost(message, repeated, tag = "1")]
    titles: Vec<MangaTitle>,
}

#[derive(Clone, PartialEq, Message)]
struct HomeResponse {
    #[prost(string, tag = "6")]
    kind: String,
    #[prost(message, repeated, tag = "7")]
    updates: Vec<MangaTitle>,
    #[prost(message, repeated, tag = "11")]
    new_series: Vec<MangaTitle>,
}

#[derive(Clone, PartialEq, Message)]
struct MyPageResponse {
    #[prost(message, repeated, tag = "1")]
    favorites: Vec<MangaTitle>,
    #[prost(message, repeated, tag = "2")]
    history: Vec<MangaTitle>,
}

#[derive(Clone, PartialEq, Message)]
struct MangaTitle {
    #[prost(int32, tag = "1")]
    id: i32,
    #[prost(string, tag = "2")]
    name: String,
    #[prost(string, optional, tag = "3")]
    thumbnail: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct MangaDetailResponse {
    #[prost(string, tag = "3")]
    title: String,
    #[prost(string, optional, tag = "4")]
    author: Option<String>,
    #[prost(string, optional, tag = "5")]
    copyright: Option<String>,
    #[prost(string, optional, tag = "6")]
    schedule: Option<String>,
    #[prost(string, optional, tag = "7")]
    warning: Option<String>,
    #[prost(string, optional, tag = "8")]
    description: Option<String>,
    #[prost(message, repeated, tag = "10")]
    tags: Vec<GenreDto>,
    #[prost(string, optional, tag = "11")]
    thumbnail: Option<String>,
    #[prost(message, repeated, tag = "13")]
    chapters: Vec<MangaUpChapter>,
}

#[derive(Clone, PartialEq, Message)]
struct GenreDto {
    #[prost(string, tag = "2")]
    name: String,
}

#[derive(Clone, PartialEq, Message)]
struct MangaUpChapter {
    #[prost(int32, tag = "1")]
    id: i32,
    #[prost(string, tag = "2")]
    name: String,
    #[prost(string, optional, tag = "3")]
    subtitle: Option<String>,
    #[prost(int32, optional, tag = "6")]
    price: Option<i32>,
    #[prost(string, optional, tag = "9")]
    date_str: Option<String>,
    #[prost(int32, optional, tag = "12")]
    status: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
struct ViewerResponse {
    #[prost(message, repeated, tag = "3")]
    page_blocks: Vec<PageBlock>,
}

#[derive(Clone, PartialEq, Message)]
struct PageBlock {
    #[prost(message, repeated, tag = "3")]
    pages: Vec<MangaUpPage>,
}

#[derive(Clone, PartialEq, Message)]
struct MangaUpPage {
    #[prost(string, tag = "1")]
    url: String,
    #[prost(string, optional, tag = "5")]
    key: Option<String>,
    #[prost(string, optional, tag = "6")]
    iv: Option<String>,
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn api_url(endpoint: &str, params: &[(&str, &str)]) -> String {
    let mut url = format!("{API_URL}/{endpoint}?app_ver=0&os_ver=0");
    if let Some(secret) = fetch_secret() {
        url.push_str("&secret=");
        url.push_str(&http::url_encode(&secret));
    }
    for (key, value) in params {
        url.push('&');
        url.push_str(&http::url_encode(key));
        url.push('=');
        url.push_str(&http::url_encode(value));
    }
    url
}

fn fetch_secret() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        manatan_extension::webview::extract_text(
            manatan_extension::webview::ExtractRequest::new(
                BASE_URL,
                "window.localStorage.getItem('secret')",
            )
            .timeout_ms(10_000)
            .cookies(true),
        )
        .ok()
        .map(|value| value.trim_matches('"').to_string())
        .filter(|value| !value.is_empty() && value != "null")
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

fn fetch_proto_or_fixture(url: &str, fixture: Vec<u8>) -> Vec<u8> {
    match client().get(url).xhr().send() {
        Ok(response) if matches!(response.status, 200..=299) => response
            .body_base64
            .and_then(|body| STANDARD.decode(body).ok())
            .unwrap_or(fixture),
        _ => fixture,
    }
}

fn parse_popular(bytes: &[u8], lang: &str) -> Paged<CatalogItem> {
    let response = PopularResponse::decode(bytes).unwrap_or_else(|_| {
        PopularResponse::decode(fixture_popular().as_slice()).expect("fixture is valid")
    });
    Paged {
        entries: response
            .titles
            .iter()
            .map(|title| title_to_item(title, lang))
            .collect(),
        has_next_page: false,
    }
}

fn parse_search(bytes: &[u8], lang: &str) -> Paged<CatalogItem> {
    let response = SearchResponse::decode(bytes).unwrap_or_else(|_| {
        SearchResponse::decode(fixture_search().as_slice()).expect("fixture is valid")
    });
    Paged {
        entries: response
            .titles
            .iter()
            .map(|title| title_to_item(title, lang))
            .collect(),
        has_next_page: false,
    }
}

fn parse_home(bytes: &[u8], lang: &str) -> Paged<CatalogItem> {
    let response = HomeResponse::decode(bytes).unwrap_or_else(|_| {
        HomeResponse::decode(fixture_home().as_slice()).expect("fixture is valid")
    });
    let titles = if response.kind == "Updates for you" {
        response.updates
    } else {
        response.new_series
    };
    Paged {
        entries: titles
            .iter()
            .map(|title| title_to_item(title, lang))
            .collect(),
        has_next_page: false,
    }
}

fn parse_my_page(bytes: &[u8], kind: &str, lang: &str) -> Paged<CatalogItem> {
    let response = MyPageResponse::decode(bytes).unwrap_or_default();
    let titles = if kind == "favorites" {
        response.favorites
    } else {
        response.history
    };
    Paged {
        entries: titles
            .iter()
            .map(|title| title_to_item(title, lang))
            .collect(),
        has_next_page: false,
    }
}

fn parse_details_proto(bytes: &[u8], id: &str, lang: &str) -> CatalogItem {
    let response = MangaDetailResponse::decode(bytes).unwrap_or_else(|_| {
        MangaDetailResponse::decode(fixture_details().as_slice()).expect("fixture is valid")
    });
    detail_to_item(&response, id, lang)
}

fn parse_chapters(bytes: &[u8], manga_id: &str, lang: &str, hide_paid: bool) -> Vec<MangaChapter> {
    let response = MangaDetailResponse::decode(bytes).unwrap_or_else(|_| {
        MangaDetailResponse::decode(fixture_details().as_slice()).expect("fixture is valid")
    });
    response
        .chapters
        .into_iter()
        .filter(|chapter| !hide_paid || chapter.price.is_none())
        .map(|chapter| chapter_to_manga_chapter(&chapter, manga_id, lang))
        .collect()
}

fn parse_pages(bytes: &[u8]) -> Vec<MangaPage> {
    let response = ViewerResponse::decode(bytes).unwrap_or_else(|_| {
        ViewerResponse::decode(fixture_viewer().as_slice()).expect("fixture is valid")
    });
    response
        .page_blocks
        .into_iter()
        .flat_map(|block| block.pages)
        .filter(|page| !page.url.contains("tutorial"))
        .enumerate()
        .map(|(index, page)| page_to_manga_page(index, page))
        .collect()
}

fn title_to_item(title: &MangaTitle, lang: &str) -> CatalogItem {
    CatalogItem {
        key: format!("/manga/{}", title.id),
        title: title.name.clone(),
        cover: title
            .thumbnail
            .as_ref()
            .map(|thumbnail| format!("{IMG_URL}{thumbnail}")),
        url: Some(format!("{BASE_URL}/manga/{}", title.id)),
        language: Some(lang.into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn detail_to_item(detail: &MangaDetailResponse, id: &str, lang: &str) -> CatalogItem {
    let description = [
        &detail.description,
        &detail.copyright,
        &detail.schedule,
        &detail.warning,
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.trim().is_empty())
    .cloned()
    .collect::<Vec<_>>()
    .join("\n\n");
    CatalogItem {
        key: format!("/manga/{id}"),
        title: detail.title.clone(),
        authors: detail.author.clone().into_iter().collect(),
        description: (!description.is_empty()).then_some(description),
        tags: detail.tags.iter().map(|tag| tag.name.clone()).collect(),
        cover: detail
            .thumbnail
            .as_ref()
            .map(|thumbnail| format!("{IMG_URL}{thumbnail}")),
        status: if detail
            .chapters
            .iter()
            .any(|chapter| chapter.status == Some(1))
        {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(format!("{BASE_URL}/manga/{id}")),
        language: Some(lang.into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn chapter_to_manga_chapter(chapter: &MangaUpChapter, manga_id: &str, lang: &str) -> MangaChapter {
    let mut title = chapter.name.clone();
    if let Some(subtitle) = chapter.subtitle.as_ref().filter(|value| !value.is_empty()) {
        title.push_str(" - ");
        title.push_str(subtitle);
    }
    if chapter.status == Some(1) {
        title.push_str(" [Final]");
    }
    if chapter.price.is_some() {
        title = format!("Locked: {title}");
    }
    MangaChapter {
        key: format!("/manga/{manga_id}/{}", chapter.id),
        title: Some(title),
        language: Some(lang.into()),
        url: Some(format!("{BASE_URL}/manga/{manga_id}/{}", chapter.id)),
        is_locked: chapter.price.is_some(),
        ..MangaChapter::default()
    }
}

fn page_to_manga_page(index: usize, page: MangaUpPage) -> MangaPage {
    let mut headers = BTreeMap::new();
    headers.insert("Referer".into(), format!("{BASE_URL}/"));
    let mut extra = BTreeMap::new();
    if let Some(key) = page.key {
        extra.insert("key".into(), Value::String(key));
    }
    if let Some(iv) = page.iv {
        extra.insert("iv".into(), Value::String(iv));
    }
    let url = format!("{IMG_URL}{}", page.url);
    MangaPage {
        content: PageContent::Url {
            url,
            context: Some(headers.clone()),
        },
        headers,
        description: Some(format!("Page {}", index + 1)),
        extra,
        ..MangaPage::default()
    }
}

fn aes_cbc_decrypt(bytes: &[u8], key_hex: &str, iv_hex: &str) -> Option<Vec<u8>> {
    let key = decode_hex(key_hex)?;
    let iv = decode_hex(iv_hex)?;
    Decryptor::<Aes128>::new_from_slices(&key, &iv)
        .ok()?
        .decrypt_padded_vec_mut::<Pkcs7>(bytes)
        .ok()
}

fn decode_hex(input: &str) -> Option<Vec<u8>> {
    if !input.len().is_multiple_of(2) {
        return None;
    }
    (0..input.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&input[index..index + 2], 16).ok())
        .collect()
}

fn source_lang(request: &Value) -> &str {
    request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .and_then(|id| id.rsplit('-').next())
        .unwrap_or("en")
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("id")))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn filter_string<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .or_else(|| filters.get("values").and_then(|values| values.get(key)))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn preference_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn deeplink_id(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) && !input.starts_with("https://www.global.manga-up.com") {
        return None;
    }
    input
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .map(ToOwned::to_owned)
}

fn fixture_popular() -> Vec<u8> {
    PopularResponse {
        titles: vec![sample_title()],
    }
    .encode_to_vec()
}

fn fixture_search() -> Vec<u8> {
    SearchResponse {
        titles: vec![sample_title()],
    }
    .encode_to_vec()
}

fn fixture_home() -> Vec<u8> {
    HomeResponse {
        kind: "New series".into(),
        updates: Vec::new(),
        new_series: vec![sample_title()],
    }
    .encode_to_vec()
}

fn fixture_details() -> Vec<u8> {
    MangaDetailResponse {
        title: "Sample UP".into(),
        author: Some("Author One".into()),
        copyright: Some("Copyright".into()),
        schedule: Some("Weekly".into()),
        warning: None,
        description: Some("Sample description.".into()),
        tags: vec![GenreDto {
            name: "Action".into(),
        }],
        thumbnail: Some("/cover.jpg".into()),
        chapters: vec![
            MangaUpChapter {
                id: 200,
                name: "Chapter 1".into(),
                subtitle: Some("Start".into()),
                price: None,
                date_str: None,
                status: None,
            },
            MangaUpChapter {
                id: 201,
                name: "Chapter 2".into(),
                subtitle: None,
                price: Some(10),
                date_str: None,
                status: Some(1),
            },
        ],
    }
    .encode_to_vec()
}

fn fixture_viewer() -> Vec<u8> {
    ViewerResponse {
        page_blocks: vec![PageBlock {
            pages: vec![MangaUpPage {
                url: "/page-1.jpg".into(),
                key: Some("000102030405060708090a0b0c0d0e0f".into()),
                iv: Some("0f0e0d0c0b0a09080706050403020100".into()),
            }],
        }],
    }
    .encode_to_vec()
}

fn sample_title() -> MangaTitle {
    MangaTitle {
        id: 100,
        name: "Sample UP".into(),
        thumbnail: Some("/cover.jpg".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cbc::cipher::BlockEncryptMut;

    #[test]
    fn parses_protobuf_catalog_details_chapters_pages() {
        assert_eq!(
            parse_popular(&fixture_popular(), "en").entries[0].title,
            "Sample UP"
        );
        assert_eq!(
            parse_search(&fixture_search(), "en").entries[0].title,
            "Sample UP"
        );
        assert_eq!(
            parse_home(&fixture_home(), "en").entries[0].title,
            "Sample UP"
        );
        let item = parse_details_proto(&fixture_details(), "100", "en");
        assert_eq!(item.authors, vec!["Author One"]);
        assert_eq!(
            parse_chapters(&fixture_details(), "100", "en", true).len(),
            1
        );
        assert_eq!(parse_pages(&fixture_viewer()).len(), 1);
    }

    #[test]
    fn decrypts_aes_cbc_pages() {
        let key = "000102030405060708090a0b0c0d0e0f";
        let iv = "0f0e0d0c0b0a09080706050403020100";
        let plain = b"sample image bytes";
        let encrypted = cbc::Encryptor::<Aes128>::new_from_slices(
            &decode_hex(key).unwrap(),
            &decode_hex(iv).unwrap(),
        )
        .unwrap()
        .encrypt_padded_vec_mut::<Pkcs7>(plain);
        assert_eq!(aes_cbc_decrypt(&encrypted, key, iv).unwrap(), plain);
    }
}
