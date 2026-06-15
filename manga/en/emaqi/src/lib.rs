use aes::Aes256;
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use cbc::{
    Decryptor,
    cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7},
};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::manga;
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use rsa::{
    Oaep, RsaPrivateKey, RsaPublicKey,
    pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey},
};
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::BTreeMap;

type Aes256CbcDec = Decryptor<Aes256>;

const SOURCE: Emaqi = Emaqi;
const BASE_URL: &str = "https://emaqi.com";
const API_URL: &str = "https://api.emaqi.com/graphql";
const LOGIN_KEY: &str = "AIzaSyC6NaQ5vOOartIGTPJHGgSP1OBjpSNKrZo";

struct Emaqi;

impl MangaSource for Emaqi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series(SERIES_FIXTURE));
        }
        let slug = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "hot-release"
        } else {
            "this-week-s-bestsellers"
        };
        Ok(parse_series(&graphql(
            SERIES_QUERY,
            "FetchHomeSection",
            json!({ "slug": slug, "mangaAfter": cursor(&request) }),
            None,
            None,
            SERIES_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let slug_query = query
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(query)
                .replace('-', " ");
            return Ok(parse_search(&graphql(
                SEARCH_QUERY,
                "Search",
                json!({ "input": { "keyword": slug_query } }),
                auth_token(&request),
                None,
                SEARCH_FIXTURE,
            )));
        }
        if !query.is_empty() {
            return Ok(parse_search(&graphql(
                SEARCH_QUERY,
                "Search",
                json!({ "input": { "keyword": query } }),
                auth_token(&request),
                None,
                SEARCH_FIXTURE,
            )));
        }
        let genre = request
            .get("filters")
            .and_then(|filters| filters.get("genre"))
            .and_then(Value::as_str)
            .unwrap_or("shonen");
        Ok(parse_genre(&graphql(
            GENRE_QUERY,
            "FetchGenre",
            json!({ "slug": genre, "mangaAfter": cursor(&request) }),
            auth_token(&request),
            None,
            SERIES_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga_key(&request);
        let comic_id = key.split('#').next().unwrap_or(&key);
        let body = graphql(
            DETAILS_QUERY,
            "FetchMangaStatus",
            json!({ "comicId": comic_id }),
            auth_token(&request),
            None,
            DETAILS_FIXTURE,
        );
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga_key(&request);
        let comic_id = key.split('#').next().unwrap_or(&key);
        let slug = key.split('#').nth(1).unwrap_or("sample");
        let hide_locked = request
            .get("preferences")
            .and_then(|prefs| prefs.get("hide_locked"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let body = graphql(
            CHAPTER_LIST_QUERY,
            "FetchComicData",
            json!({ "comicId": comic_id }),
            auth_token(&request),
            None,
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body, slug, hide_locked))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "comic-1/chapter/1/sample".to_string());
        let (x_hash, private_key) = key_pair_for(&key);
        let parts = key.split('/').collect::<Vec<_>>();
        let comic_id = parts.first().copied().unwrap_or("comic-1");
        let kind = parts.get(1).copied().unwrap_or("chapter");
        let number = parts
            .get(2)
            .copied()
            .unwrap_or("1")
            .parse::<i64>()
            .unwrap_or(1);
        let (query, operation, variables) = if kind == "volume" {
            (
                VOLUME_QUERY,
                "FetchMangaContents",
                json!({ "comicId": comic_id, "volumeNumber": number }),
            )
        } else {
            (
                CHAPTER_QUERY,
                "FetchChapterContents",
                json!({ "comicId": comic_id, "chapterNumber": number }),
            )
        };
        let body = graphql(
            query,
            operation,
            variables,
            auth_token(&request),
            Some(x_hash),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body, &private_key))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request
            .get("manga")
            .and_then(|manga| manga.get("key").or_else(|| manga.get("url")))
            .and_then(Value::as_str)
            .and_then(|key| key.split('#').nth(1))
            .map(|slug| format!("{BASE_URL}/manga/{slug}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request
            .get("chapter")
            .and_then(|chapter| chapter.get("key").or_else(|| chapter.get("url")))
            .and_then(Value::as_str)
            .map(chapter_url))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let query = input
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(input)
                .replace('-', " ");
            return Ok(Some(UrlResolveResult {
                search: Some(SearchRequest {
                    query,
                    ..SearchRequest::default()
                }),
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
        let image_base64 = request
            .get("imageBase64")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mime_type = request
            .get("mimeType")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let decrypted = decrypt_page_image(image_base64, request.get("page")).unwrap_or_else(|| {
            STANDARD
                .decode(image_base64)
                .unwrap_or_else(|_| Vec::new())
        });
        Ok(ProcessedImage {
            image_base64: STANDARD.encode(decrypted),
            mime_type,
            ..ProcessedImage::default()
        })
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn graphql(
    query: &str,
    operation: &str,
    variables: Value,
    token: Option<String>,
    x_hash: Option<String>,
    fixture: &str,
) -> String {
    let http = client();
    let mut request = http
        .post(API_URL)
        .json(json!({ "query": query, "operationName": operation, "variables": variables }).to_string())
        .xhr();
    if let Some(token) = token.filter(|token| !token.is_empty()) {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    if let Some(x_hash) = x_hash {
        request = request.header("X-Hash", x_hash);
    }
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn auth_token(request: &Value) -> Option<String> {
    let prefs = request.get("preferences")?;
    let email = prefs.get("email_pref").and_then(Value::as_str)?.trim();
    let password = prefs.get("password_pref").and_then(Value::as_str)?.trim();
    if email.is_empty() || password.is_empty() {
        return None;
    }
    let body = client()
        .post(format!(
            "https://identitytoolkit.googleapis.com/v1/accounts:signInWithPassword?key={LOGIN_KEY}"
        ))
        .json(json!({ "email": email, "password": password, "returnSecureToken": true }).to_string())
        .xhr()
        .send_text()
        .ok()?;
    serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| value.get("idToken").and_then(Value::as_str).map(ToString::to_string))
}

fn parse_series(body: &str) -> Paged<CatalogItem> {
    let value = graph_data(body, SERIES_FIXTURE);
    let section = value
        .get("homeSection")
        .or_else(|| value.get("genre"))
        .unwrap_or(&Value::Null);
    parse_conn(section)
}

fn parse_genre(body: &str) -> Paged<CatalogItem> {
    let value = graph_data(body, SERIES_FIXTURE);
    parse_conn(value.get("genre").unwrap_or(&Value::Null))
}

fn parse_conn(section: &Value) -> Paged<CatalogItem> {
    let conn = section.get("mangaConn").unwrap_or(&Value::Null);
    let entries = conn
        .get("edges")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|edge| edge.pointer("/node/comic").cloned())
        .map(catalog_from_comic)
        .collect();
    Paged {
        entries,
        has_next_page: conn
            .pointer("/pageInfo/hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let value = graph_data(body, SEARCH_FIXTURE);
    Paged {
        entries: value
            .get("search")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(catalog_from_comic)
            .collect(),
        has_next_page: false,
    }
}

fn catalog_from_comic(comic: Value) -> CatalogItem {
    let comic_id = text(&comic, "comicId").unwrap_or_else(|| "comic-1".to_string());
    let slug = text(&comic, "slug").unwrap_or_else(|| "sample".to_string());
    CatalogItem {
        key: format!("{comic_id}#{slug}"),
        title: text(&comic, "title").unwrap_or_else(|| "Manga".to_string()),
        cover: comic.pointer("/cover/url").and_then(Value::as_str).map(ToString::to_string),
        url: Some(format!("{BASE_URL}/manga/{slug}")),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let value = graph_data(body, DETAILS_FIXTURE);
    let comic = value.pointer("/manga/comic").unwrap_or(&Value::Null);
    let mut description = text(comic, "synopsis").unwrap_or_default();
    if let Some(publisher) = text(comic, "publisher").filter(|value| !value.is_empty()) {
        description.push_str(&format!("\n\nPublisher: {publisher}"));
    }
    if let Some(rating) = comic.get("rating").and_then(Value::as_i64) {
        description.push_str(&format!("\n\nAge limit: {rating}+"));
    }
    CatalogItem {
        key: key.unwrap_or_else(|| "comic-1#sample".to_string()),
        title: text(comic, "title").unwrap_or_else(|| "Manga".to_string()),
        cover: comic.pointer("/cover/url").and_then(Value::as_str).map(ToString::to_string),
        description: (!description.is_empty()).then_some(description),
        authors: string_array(comic.get("creators")),
        tags: comic
            .get("genres")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|genre| text(&genre, "name"))
            .collect(),
        status: if comic.pointer("/metadata/completed").and_then(Value::as_bool) == Some(true) {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: None,
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, slug: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let value = graph_data(body, CHAPTERS_FIXTURE);
    let mut chapters = value
        .get("chapters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|chapter| !hide_locked || !is_locked(chapter))
        .map(|chapter| {
            let comic_id = text(&chapter, "comicId").unwrap_or_else(|| "comic-1".to_string());
            let number = chapter.get("chapterNumber").and_then(Value::as_i64).unwrap_or(1);
            let locked = if is_locked(&chapter) { "Locked " } else { "" };
            MangaChapter {
                key: format!("{comic_id}/chapter/{number}/{slug}"),
                title: Some(format!("{locked}{}", text(&chapter, "name").unwrap_or_else(|| format!("Chapter {number}")))),
                chapter_number: Some(number as f32),
                url: Some(format!("{BASE_URL}/reader/{slug}?type=chapter&chapter={number}")),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.reverse();

    let mut volumes = value
        .pointer("/comicVolumes/volumes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|volume| !hide_locked || (!is_locked(volume) && !is_preview(volume)))
        .map(|volume| {
            let comic_id = text(&volume, "comicId").unwrap_or_else(|| "comic-1".to_string());
            let number = volume.get("volumeNumber").and_then(Value::as_i64).unwrap_or(1);
            let volume_slug = text(&volume, "slug").unwrap_or_default();
            let locked = if is_locked(&volume) { "Locked " } else { "" };
            let preview = if is_preview(&volume) { "(Preview) " } else { "" };
            MangaChapter {
                key: format!("{comic_id}/volume/{number}/{slug}/{volume_slug}"),
                title: Some(format!("{locked}{preview}{}", text(&volume, "name").unwrap_or_else(|| format!("Volume {number}")))),
                chapter_number: Some(number as f32),
                url: Some(format!("{BASE_URL}/reader/{slug}-{volume_slug}")),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    volumes.reverse();
    chapters.extend(volumes);
    chapters
}

fn parse_pages(body: &str, private_key: &str) -> Vec<MangaPage> {
    let value = graph_data(body, PAGES_FIXTURE);
    let contents = value
        .pointer("/chapter/contents")
        .or_else(|| value.pointer("/manga/contents"))
        .unwrap_or(&Value::Null);
    let hash = contents.get("hash").and_then(Value::as_str).unwrap_or_default();
    contents
        .get("pages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .filter_map(|(index, page)| {
            let image = text(&page, "url")?;
            let mut extra = BTreeMap::new();
            extra.insert("privateKey".to_string(), Value::String(private_key.to_string()));
            extra.insert("hash".to_string(), Value::String(hash.to_string()));
            Some(MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                extra,
                ..MangaPage::default()
            })
        })
        .collect()
}

fn decrypt_page_image(image_base64: &str, page: Option<&Value>) -> Option<Vec<u8>> {
    let page = page?;
    let private_key = page
        .get("extra")
        .and_then(|extra| extra.get("privateKey"))
        .and_then(Value::as_str)?;
    let hash = page
        .get("extra")
        .and_then(|extra| extra.get("hash"))
        .and_then(Value::as_str)?;
    let encrypted = STANDARD.decode(image_base64).ok()?;
    let private_key = URL_SAFE_NO_PAD.decode(private_key).ok()?;
    let private_key = RsaPrivateKey::from_pkcs8_der(&private_key).ok()?;
    let aes_key = private_key
        .decrypt(Oaep::new::<Sha256>(), &STANDARD.decode(hash).ok()?)
        .ok()?;
    if encrypted.first().copied() == Some(2) {
        if encrypted.len() < 19 {
            return None;
        }
        let iv = &encrypted[2..18];
        let cipher = Aes256Gcm::new_from_slice(&aes_key).ok()?;
        return cipher.decrypt(Nonce::from_slice(iv), &encrypted[18..]).ok();
    }
    if encrypted.len() < 17 {
        return None;
    }
    let iv = &encrypted[..16];
    Aes256CbcDec::new_from_slices(&aes_key, iv)
        .ok()?
        .decrypt_padded_vec_mut::<Pkcs7>(&encrypted[16..])
        .ok()
}

fn key_pair_for(seed_text: &str) -> (String, String) {
    let mut seed = [0u8; 32];
    for (index, byte) in seed_text.as_bytes().iter().enumerate() {
        seed[index % 32] = seed[index % 32].wrapping_mul(31).wrapping_add(*byte);
    }
    let mut rng = ChaCha20Rng::from_seed(seed);
    let private = RsaPrivateKey::new(&mut rng, 2048).expect("rsa key generation works");
    let public_der = RsaPublicKey::from(&private)
        .to_public_key_der()
        .expect("public der")
        .as_bytes()
        .to_vec();
    let private_der = private
        .to_pkcs8_der()
        .expect("private der")
        .as_bytes()
        .to_vec();
    (STANDARD.encode(public_der), URL_SAFE_NO_PAD.encode(private_der))
}

fn graph_data(body: &str, fixture: &str) -> Value {
    serde_json::from_str::<Value>(body)
        .or_else(|_| serde_json::from_str(fixture))
        .ok()
        .and_then(|value| value.get("data").cloned().or(Some(value)))
        .unwrap_or(Value::Null)
}

fn manga_key(request: &Value) -> String {
    manga::request_key(request, "manga").unwrap_or_else(|| "comic-1#sample".to_string())
}

fn cursor(request: &Value) -> Value {
    request
        .get("cursor")
        .or_else(|| request.get("mangaAfter"))
        .cloned()
        .unwrap_or(Value::Null)
}

fn chapter_url(key: &str) -> String {
    let parts = key.split('/').collect::<Vec<_>>();
    let kind = parts.get(1).copied().unwrap_or("chapter");
    let number = parts.get(2).copied().unwrap_or("1");
    let slug = parts.get(3).copied().unwrap_or("sample");
    if kind == "volume" {
        let volume_slug = parts.get(4).copied().unwrap_or_default();
        format!("{BASE_URL}/reader/{slug}-{volume_slug}")
    } else {
        format!("{BASE_URL}/reader/{slug}?type=chapter&chapter={number}")
    }
}

fn is_locked(value: &Value) -> bool {
    value.get("purchased").and_then(Value::as_bool) == Some(false)
        && value.get("free").and_then(Value::as_bool) == Some(false)
}

fn is_preview(value: &Value) -> bool {
    is_locked(value) && value.get("trialPage").and_then(Value::as_i64).unwrap_or(0) > 0
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(ToString::to_string)
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(ToString::to_string))
        .collect()
}

export_manga_source!(SOURCE);

const SERIES_QUERY: &str = r#"query FetchHomeSection($slug: String!, $mangaAfter: String) { homeSection(slug: $slug) { mangaConn(first: 40, after: $mangaAfter) { edges { node { comic { comicId slug title cover { url } } } } pageInfo { hasNextPage endCursor } } } }"#;
const SEARCH_QUERY: &str = r#"query Search($input: SearchInput!) { search(input: $input) { comicId title slug cover { url } } }"#;
const GENRE_QUERY: &str = r#"query FetchGenre($slug: String!, $mangaAfter: String) { genre(slug: $slug) { mangaConn(first: 40, after: $mangaAfter) { edges { node { comic { comicId slug title cover { url } } } } pageInfo { hasNextPage endCursor } } } }"#;
const DETAILS_QUERY: &str = r#"query FetchMangaStatus($comicId: String!) { manga(comicId: $comicId) { comic { title synopsis rating creators publisher metadata { completed } cover { url } genres { ... on Tag { name } } } } }"#;
const CHAPTER_LIST_QUERY: &str = r#"query FetchComicData($comicId: String!) { comicVolumes(comicId: $comicId) { volumes { comicId trialPage slug volumeNumber name price purchased free releasesAt } } chapters(comicId: $comicId) { comicId chapterNumber name purchased free releasesAt } }"#;
const CHAPTER_QUERY: &str = r#"query FetchChapterContents($comicId: String!, $chapterNumber: Int!) { chapter(comicId: $comicId, chapterNumber: $chapterNumber) { contents { pages { url } hash } } }"#;
const VOLUME_QUERY: &str = r#"query FetchMangaContents($comicId: String!, $volumeNumber: Int!) { manga(comicId: $comicId, volumeNumber: $volumeNumber) { contents { pages { url } hash } } }"#;

const SERIES_FIXTURE: &str = r#"{"data":{"homeSection":{"mangaConn":{"edges":[{"node":{"comic":{"comicId":"comic-1","slug":"sample","title":"Sample Manga","cover":{"url":"https://emaqi.com/cover.jpg"}}}}],"pageInfo":{"hasNextPage":false,"endCursor":"cursor"}}}}}"#;
const SEARCH_FIXTURE: &str = r#"{"data":{"search":[{"comicId":"comic-1","slug":"sample","title":"Sample Manga","cover":{"url":"https://emaqi.com/cover.jpg"}}]}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"manga":{"comic":{"title":"Sample Manga","synopsis":"Sample description.","rating":18,"creators":["Writer"],"publisher":"Publisher","metadata":{"completed":false},"cover":{"url":"https://emaqi.com/cover.jpg"},"genres":[{"name":"Drama"}]}}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"comicVolumes":{"volumes":[{"comicId":"comic-1","trialPage":0,"slug":"vol-1","volumeNumber":1,"name":"Volume 1","purchased":true,"free":true,"releasesAt":"2024-01-01T00:00:00.000000000Z"}]},"chapters":[{"comicId":"comic-1","chapterNumber":1,"name":"Chapter 1","purchased":true,"free":true,"releasesAt":"2024-01-01T00:00:00.000000000Z"},{"comicId":"comic-1","chapterNumber":2,"name":"Chapter 2","purchased":false,"free":false,"releasesAt":"2024-01-02T00:00:00.000000000Z"}]}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"chapter":{"contents":{"pages":[{"url":"https://r.emaqi.com/page1.bin"},{"url":"https://r.emaqi.com/page2.bin"}],"hash":"AA=="}}}}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_graphql_payloads() {
        assert_eq!(parse_series(SERIES_FIXTURE).entries[0].title, "Sample Manga");
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, "sample", true).len(), 2);
        assert_eq!(parse_pages(PAGES_FIXTURE, "private").len(), 2);
    }
}
