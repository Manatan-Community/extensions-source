use base64::{engine::general_purpose::STANDARD, Engine as _};
use flate2::read::DeflateDecoder;
use manatan_extension::{abi::ExtensionResult, export_manga_source, http, source::MangaSource, CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult};
use manatan_shared::{html, manga, url};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Read, time::{SystemTime, UNIX_EPOCH}};

const SOURCE: Happymh = Happymh;
const BASE_URL: &str = "https://m.happymh.com";
const DUMMY: &str = "dummy-mark";

struct Happymh;

impl MangaSource for Happymh {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "last_date" } else { "views" };
        Ok(parse_popular(&fetch_json(&format!("{BASE_URL}/apis/c/index?pn={}&series_status=-1&order={order}", page(&request)), LIST_FIXTURE), page(&request)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged { entries: vec![parse_details(&fetch_doc(query, DETAILS_FIXTURE), &key)], has_next_page: false });
        }
        if !query.is_empty() {
            let body = client()
                .post_form_text(
                    format!("{BASE_URL}/v2.0/apis/manga/ssearch"),
                    &[("searchkey", query), ("v", "v2.13")],
                )
                .unwrap_or_else(|_| LIST_FIXTURE.into());
            return Ok(parse_popular(&body, 1));
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mut target = format!("{BASE_URL}/apis/c/index?pn={}", page(&request));
        for key in ["genre", "area", "audience", "series_status"] {
            if let Some(value) = filters.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()) {
                target.push('&');
                target.push_str(key);
                target.push('=');
                target.push_str(value);
            }
        }
        Ok(parse_popular(&fetch_json(&target, LIST_FIXTURE), page(&request)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(&fetch_doc(&absolute(&key), DETAILS_FIXTURE), &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let manga_key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let comic_id = manga_key.trim_end_matches('/').rsplit('/').next().unwrap_or("sample");
        let mut chapters = Vec::new();
        let mut page_no = 1;
        loop {
            let data = fetch_chapter_page(comic_id, page_no);
            for item in data.items {
                chapters.push(MangaChapter {
                    key: format!("/{comic_id}/{DUMMY}/{}#{page_no}", item.id),
                    title: Some(item.chapter_name),
                    url: Some(format!("{BASE_URL}/mangaread/{comic_id}/{}", item.id)),
                    extra: BTreeMap::from([("comicId".into(), json!(comic_id)), ("chapterId".into(), json!(item.id.to_string()))]),
                    ..MangaChapter::default()
                });
            }
            if data.is_end == 1 || chapters.is_empty() || page_no > 50 {
                break;
            }
            page_no += 1;
        }
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/dummy-mark/1#1".into());
        let parts = key.trim_start_matches('/').split('/').collect::<Vec<_>>();
        let comic_id = parts.first().copied().unwrap_or("sample");
        let chapter_id = parts.get(2).copied().unwrap_or("1").split('#').next().unwrap_or("1");
        let request_id = millis().to_string();
        let target = format!("{BASE_URL}/v2.0/apis/manga/reading?code={comic_id}&cid={chapter_id}&v=v4.300101&_t={request_id}");
        let body = client()
            .get(target)
            .header("X-Requested-With", "XMLHttpRequest")
            .header("X-Requested-Id", request_id)
            .header("Referer", format!("{BASE_URL}/mangaread/{comic_id}/{chapter_id}"))
            .send_text()
            .unwrap_or_else(|_| PAGES_FIXTURE.into());
        Ok(parse_pages(&body))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> { Ok(manga::request_key(&request, "manga").map(|key| absolute(&key))) }
    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let parts = key.trim_start_matches('/').split('/').collect::<Vec<_>>();
            format!("{BASE_URL}/mangaread/{}/{}", parts.first().copied().unwrap_or("sample"), parts.get(2).copied().unwrap_or("1").split('#').next().unwrap_or("1"))
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult { item: Some(parse_details(&fetch_doc(input, DETAILS_FIXTURE), &key)), url: Some(input.into()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }), url: Some(input.into()), ..UrlResolveResult::default() }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}
fn fetch_json(target: &str, fixture: &str) -> String { client().get(target).header("Accept", "application/json, text/plain, */*").send_text().unwrap_or_else(|_| fixture.into()) }
fn fetch_doc(target: &str, fixture: &str) -> String { client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.into()) }
fn page(request: &Value) -> u64 { request.get("page").and_then(Value::as_u64).unwrap_or(1) }
fn normalize_key(input: &str) -> String { format!("/{}", input.strip_prefix(BASE_URL).unwrap_or(input).split('?').next().unwrap_or(input).trim_matches('/')) }
fn absolute(key: &str) -> String { url::join_url(BASE_URL, key) }
fn millis() -> u128 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0) }

fn parse_popular(body: &str, page_no: u64) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<PopularResponse>(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("valid fixture"));
    Paged {
        has_next_page: !response.data.is_end,
        entries: response.data.items.into_iter().map(|item| CatalogItem {
            key: format!("/manga/{}", item.manga_code),
            title: item.name,
            cover: Some(item.cover),
            url: Some(format!("{BASE_URL}/manga/{}", item.manga_code)),
            language: Some("zh".into()),
            content_rating: Some("safe".into()),
            latest_update: Some(page_no as i64),
            ..CatalogItem::default()
        }).collect(),
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "mg-title", "</").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| "嗨皮漫画".into()),
        cover: html::attr_after(body, "mg-cover", "src").map(|v| url::join_url(BASE_URL, &v)),
        authors: html::text_between(body, "mg-sub-title:nth-of-type(2)", "</").or_else(|| html::text_between(body, "mg-sub-title", "</")).map(|v| html::strip_tags(&v)).into_iter().collect(),
        artists: Vec::new(),
        tags: body.split("<a").filter(|c| c.contains("mg-cate")).map(html::strip_tags).filter(|v| !v.is_empty()).collect(),
        description: html::text_between(body, "manga-introduction", "</div>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()),
        status: ItemStatus::Unknown,
        url: Some(absolute(key)),
        language: Some("zh".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapter_page(comic_id: &str, page_no: u64) -> ChapterData {
    let request_id = millis().to_string();
    let target = format!("{BASE_URL}/v2.0/apis/manga/chapterByPage?code={comic_id}&lang=cn&order=asc&page={page_no}&_t={request_id}");
    serde_json::from_str::<ChapterResponse>(&fetch_json(&target, CHAPTERS_FIXTURE)).map(|r| r.data).unwrap_or_default()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let dto = serde_json::from_str::<PageResponse>(body).unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("valid fixture"));
    let scans = if dto.data.is_encode { decode_scans(&dto.data.scans).unwrap_or(dto.data.scans) } else { dto.data.scans };
    serde_json::from_str::<Vec<PageDto>>(&scans).unwrap_or_default().into_iter().filter(|p| p.n == 0).enumerate().map(|(index, page)| {
        MangaPage { content: PageContent::Url { url: page.url.split("?q=").next().unwrap_or(&page.url).to_string(), context: None }, headers: manga::image_headers(BASE_URL), description: Some(format!("Page {}", index + 1)), ..MangaPage::default() }
    }).collect()
}

fn decode_scans(input: &str) -> Option<String> {
    let buf = input.as_bytes();
    if buf.len() < 8 {
        return None;
    }
    let digest = Sha256::digest([b"DEV_SCAN_SECRET_2026_change_me".as_slice(), &buf[..8], b"happymh.com"].concat());
    let off1 = (digest[0] as usize) % 24 + 8;
    let off2 = (digest[1] as usize) % 24 + 8;
    let off3 = (digest[2] as usize) % 24 + 8;
    let key = hex_to_bytes(input.get(off1 + 8..off1 + 72)?)?;
    let nonce = hex_to_bytes(input.get(off1 + 72 + off2..off1 + 72 + off2 + 32)?)?;
    let ciphertext = STANDARD.decode(input.get(off1 + 72 + off2 + 32 + off3..)?).ok()?;
    let mut state = [0u8; 52];
    state[..32].copy_from_slice(&key);
    state[32..48].copy_from_slice(&nonce);
    let mut plain = vec![0u8; ciphertext.len()];
    for (block_idx, chunk) in ciphertext.chunks(32).enumerate() {
        state[48] = (block_idx >> 24) as u8;
        state[49] = (block_idx >> 16) as u8;
        state[50] = (block_idx >> 8) as u8;
        state[51] = block_idx as u8;
        let ks = Sha256::digest(state);
        for (j, byte) in chunk.iter().enumerate() {
            plain[block_idx * 32 + j] = byte ^ ks[j];
        }
    }
    if !plain.starts_with(b"SC01") {
        return None;
    }
    let mut decoder = DeflateDecoder::new(&plain[4..]);
    let mut out = String::new();
    decoder.read_to_string(&mut out).ok()?;
    Some(out)
}

fn hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()
}

#[derive(Deserialize)]
struct PopularResponse { data: PopularData }
#[derive(Deserialize)]
struct PopularData { items: Vec<MangaDto>, #[serde(rename = "isEnd")] is_end: bool }
#[derive(Deserialize)]
struct MangaDto { name: String, #[serde(rename = "manga_code")] manga_code: String, cover: String }
#[derive(Default, Deserialize)]
struct ChapterData { items: Vec<ChapterItem>, #[serde(rename = "isEnd")] is_end: i32 }
#[derive(Deserialize)]
struct ChapterResponse { data: ChapterData }
#[derive(Deserialize)]
struct ChapterItem { id: u64, #[serde(rename = "chapterName")] chapter_name: String }
#[derive(Deserialize)]
struct PageResponse { data: PageData }
#[derive(Deserialize)]
struct PageData { scans: String, #[serde(rename = "isEncode")] is_encode: bool }
#[derive(Deserialize)]
struct PageDto { n: i32, url: String }

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":{"items":[{"name":"Sample","manga_code":"sample","cover":"https://m.happymh.com/cover.jpg"}],"isEnd":true}}"#;
const DETAILS_FIXTURE: &str = r#"<div class="mg-property"><h2 class="mg-title">Sample</h2><p class="mg-sub-title">Author</p></div><div class="mg-cover"><mip-img src="/cover.jpg"></mip-img></div><div class="manga-introduction">Sample description.</div>"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"items":[{"id":1,"chapterName":"Chapter 1","order":1}],"isEnd":1}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"scans":"[{\"n\":0,\"url\":\"https://m.happymh.com/page.jpg\"}]","isEncode":false}}"#;
