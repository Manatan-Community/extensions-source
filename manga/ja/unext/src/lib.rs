use aes::Aes128;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::{
    Decryptor,
    cipher::{BlockDecryptMut, KeyIvInit, block_padding::NoPadding},
};
use flate2::read::DeflateDecoder;
use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ImageRequest, ItemStatus, MangaChapter,
    MangaPage, PageContent, Paged, ProcessedImage, SearchRequest, UrlResolveResult,
    abi::{ExtensionError, ExtensionResult},
    export_manga_source,
    source::MangaSource,
    webview,
};
use manatan_shared::{
    manga,
    sdk::http::{Headers, HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Read};

type Aes128CbcDec = Decryptor<Aes128>;

const SOURCE: UNext = UNext;
const BASE_URL: &str = "https://video.unext.jp";
const API_URL: &str = "https://cc.unext.jp";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36";
const POPULAR_QUERY_HASH: &str = "1e1e84fd9b5718c37ef030ea8230bbf9ddd1e5b86f5b8ce2c224b3704f0468ec";
const LATEST_QUERY_HASH: &str = "0570a586caa9869bd5eb0b05a59bdfec853f92dc6cf280ebd583df2cc93e1c21";
const SEARCH_QUERY_HASH: &str = "2ec7804350bf993678c92a5d79f20812b3b0d5b38aaba2603c9dd291c6df927e";
const DETAILS_QUERY_HASH: &str = "99f21ebea20b64b11ef5d3b811c2b3fa5b4dbd8c5d2933baadf9c26fc60b35d1";
const CHAPTER_LIST_QUERY_HASH: &str =
    "66f0c600259b82a4826fba7be2ace33726f3ec09735e65a421f8f602b481487d";
const PLAYLIST_QUERY_HASH: &str =
    "f8a851c14ec61eb42dff966570b2ad49f86eeec7f39d2d32ab0ec58cad268fc1";
const POPULAR_FIXTURE: &str = r#"{
  "bookRanking": {
    "books": [
      {
        "bookSakuhin": {
          "sakuhinCode": "BSD0000820098",
          "name": "U-NEXT Sample",
          "book": {
            "thumbnail": {
              "standard": "img.unext.jp/book_thumb/sample.jpg"
            },
            "credits": [
              {
                "penName": "Sample Author"
              }
            ]
          },
          "detail": {
            "introduction": "Offline smoke-test fixture for the U-NEXT source."
          },
          "isCompleted": false,
          "subgenreTagList": [
            {
              "name": "Manga"
            }
          ]
        }
      }
    ],
    "pageInfo": {
      "page": 1,
      "pageSize": 20,
      "results": 1
    }
  }
}"#;

struct UNext;

impl MangaSource for UNext {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            let result: PopularResponse = parse_json_str(POPULAR_FIXTURE, "popular fixture")?;
            return Ok(Paged {
                entries: result
                    .book_ranking
                    .books
                    .into_iter()
                    .filter_map(|entry| entry.book_sakuhin.to_item())
                    .collect(),
                has_next_page: result.book_ranking.page_info.has_next_page(),
            });
        }
        let page = page(&request);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let data = graph_data(
                "cosmo_getNewBooks",
                json!({"tagCode":"TAG0000014500","page":page,"pageSize":20}),
                LATEST_QUERY_HASH,
            )?;
            let result: LatestResponse = parse_json_value(data, "latest response")?;
            return Ok(page_from_books(
                result.new_books.books,
                result.new_books.page_info,
            ));
        }
        let data = graph_data(
            "cosmo_getBookRanking",
            json!({"targetCode":"D_C_COMIC","page":page,"pageSize":20}),
            POPULAR_QUERY_HASH,
        )?;
        let result: PopularResponse = parse_json_value(data, "popular response")?;
        Ok(Paged {
            entries: result
                .book_ranking
                .books
                .into_iter()
                .filter_map(|entry| entry.book_sakuhin.to_item())
                .collect(),
            has_next_page: result.book_ranking.page_info.has_next_page(),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query).filter(|key| key.starts_with("/book/title/")) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)?],
                has_next_page: false,
            });
        }
        let data = graph_data(
            "cosmo_bookFreewordSearch",
            json!({"query":query,"page":page(&request),"pageSize":20,"filterSaleType":null,"sortOrder":"RECOMMEND"}),
            SEARCH_QUERY_HASH,
        )?;
        let result: SearchResponse = parse_json_value(data, "search response")?;
        Ok(page_from_books(
            result.search.books,
            result.search.page_info,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/book/title/BSD0000820098".into());
        details_by_key(&key)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/book/title/BSD0000820098".into());
        let code = key.rsplit('/').next().unwrap_or(&key);
        let data = graph_data(
            "cosmo_bookTitleBooks",
            json!({"bookSakuhinCode":code,"booksPage":1,"booksPageSize":9999}),
            CHAPTER_LIST_QUERY_HASH,
        )?;
        let result: ChapterListResponse = parse_json_value(data, "chapter list response")?;
        let hide_paid = preference_bool(&request, "hide_paid", true);
        let mut chapters = result
            .book_title_books
            .books
            .into_iter()
            .filter(|book| !hide_paid || book.is_readable())
            .filter_map(|book| book.to_chapter(code))
            .collect::<Vec<_>>();
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/book/view/BSD0000820098/BID0001508570#BFC0002699405".into());
        let Some(book_file_code) = key.split('#').nth(1).filter(|value| !value.is_empty()) else {
            return Ok(vec![manga::text_page(
                "This chapter does not expose a readable book file code.",
            )]);
        };
        let data = graph_data(
            "cosmo_getBookPlaylistUrl",
            json!({"bookFileCode":book_file_code}),
            PLAYLIST_QUERY_HASH,
        )
        .map_err(|_| err("Log in via WebView and rent or purchase this chapter to read."))?;
        let result: PlaylistResponse = parse_json_value(data, "playlist response")?;
        let Some(ubook) = result.playlist_url.playlist_url.ubooks.first() else {
            return Ok(vec![manga::text_page(
                "No UBook playlist was returned for this chapter.",
            )]);
        };
        let zip_url = url::join_url(&result.playlist_url.playlist_base_url, &ubook.content);
        let keys = fetch_keys(&absolute_url(&key))?;
        let zip = ZipIndex::load(&zip_url)?;
        let index_json: UBookIndex = parse_json_slice(&zip.fetch("index.json")?, "UBook index")?;
        let drm_json: UBookDrm = parse_json_slice(&zip.fetch("drm.json")?, "UBook DRM")?;
        let mut pages = Vec::new();
        for (index, spine) in index_json.spine.iter().enumerate() {
            let Some(page) = index_json.pages.get(&spine.page_id) else {
                continue;
            };
            let entry_name = &page.image.src;
            let Some(drm) = drm_json.encrypted_file_list.get(entry_name) else {
                continue;
            };
            let Some(key) = keys.get(&drm.key_id) else {
                return Ok(vec![manga::text_page(&format!(
                    "Decryption key was not loaded for {}.",
                    drm.key_id
                ))]);
            };
            let Some(entry) = zip.entry(entry_name) else {
                continue;
            };
            let range_start = zip.zip_start_offset + entry.local_file_header_relative_offset as u64;
            let range_end = range_start + entry.compressed_size as u64 + 512;
            let mut headers = Context::new();
            headers.insert("Range".into(), format!("bytes={range_start}-{range_end}"));
            let extra = json!({
                "compressedSize": entry.compressed_size,
                "key": key,
                "iv": drm.iv,
                "originalFileSize": drm.original_file_size
            });
            pages.push(MangaPage {
                content: PageContent::Request {
                    request: ImageRequest {
                        url: zip_url.clone(),
                        method: Some("GET".into()),
                        headers: headers.clone(),
                        referrer: Some(BASE_URL.into()),
                        extra: object_from_value(extra.clone()),
                        ..ImageRequest::default()
                    },
                },
                headers,
                extra: BTreeMap::from([("unext".into(), extra)]),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            });
        }
        if pages.is_empty() {
            Ok(vec![manga::text_page(
                "No readable image pages were found in the UBook manifest.",
            )])
        } else {
            Ok(pages)
        }
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
        process_unext_image(request)
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
            if key.starts_with("/book/title/") {
                return Ok(Some(UrlResolveResult {
                    item: Some(details_by_key(&key)?),
                    url: Some(input.into()),
                    ..UrlResolveResult::default()
                }));
            }
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
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn graph_data(operation_name: &str, variables: Value, hash: &str) -> ExtensionResult<Value> {
    let extensions = json!({"persistedQuery":{"version":1,"sha256Hash":hash}});
    let target = format!(
        "{API_URL}/?operationName={}&variables={}&extensions={}",
        url::query_escape(operation_name),
        url::query_escape(&variables.to_string()),
        url::query_escape(&extensions.to_string())
    );
    let body = client()
        .get(&target)
        .header("Content-Type", "application/json")
        .header("Apollographql-Client-Name", "cosmo")
        .header("Accept", "application/json")
        .send_text()?;
    let root: Value = parse_json_str(&body, "GraphQL response")?;
    if let Some(errors) = root.get("errors") {
        return Err(err(&format!("GraphQL error: {errors}")));
    }
    root.get("data")
        .cloned()
        .ok_or_else(|| err("GraphQL response did not contain data"))
}

fn details_by_key(key: &str) -> ExtensionResult<CatalogItem> {
    let code = key.rsplit('/').next().unwrap_or(key);
    let data = graph_data(
        "cosmo_bookTitleDetail",
        json!({"bookSakuhinCode":code,"viewBookCode":"TOTAL","bookListPageSize":2,"bookListChapterPageSize":5}),
        DETAILS_QUERY_HASH,
    )?;
    let result: DetailsResponse = parse_json_value(data, "details response")?;
    result
        .book_title
        .to_item()
        .ok_or_else(|| err("U-NEXT title details were missing"))
}

fn page_from_books(books: Vec<BookSakuhin>, page_info: PageInfo) -> Paged<CatalogItem> {
    Paged {
        entries: books.into_iter().filter_map(BookSakuhin::to_item).collect(),
        has_next_page: page_info.has_next_page(),
    }
}

fn fetch_keys(viewer_url: &str) -> ExtensionResult<BTreeMap<String, String>> {
    let script = r#"
new Promise((resolve) => {
  const finish = (value) => resolve(JSON.stringify(value || {}));
  const start = Date.now();
  const attempt = async () => {
    try {
      const el = document.querySelector(".swiper");
      const getFiber = n => {
        const k = Object.keys(n || {}).find(x => x.startsWith("__reactFiber$") || x.startsWith("__reactInternalInstance$"));
        return n && n[k];
      };
      let curr = getFiber(el);
      let mgr = null;
      while (curr) {
        if (mgr = curr.memoizedProps?.manager) break;
        curr = curr.return;
      }
      const fileList = mgr?.parser?.drmParser?.drmHeader?.encryptedFileList;
      const contextKeys = mgr?.parser?.drmContext?.keys || {};
      if (fileList) {
        const loaded = new Set(Object.keys(contextKeys));
        for (const [path, info] of Object.entries(fileList)) {
          if (info.keyId && !loaded.has(info.keyId)) mgr.parser.getBinaryObject(path).catch(() => {});
        }
      }
      const out = {};
      for (const [id, key] of Object.entries(contextKeys)) {
        try {
          const raw = await crypto.subtle.exportKey("raw", key);
          out[id] = btoa(String.fromCharCode(...new Uint8Array(raw)));
        } catch (_) {}
      }
      if (Object.keys(out).length > 0) return finish(out);
    } catch (_) {}
    if (Date.now() - start > 45000) return finish({});
    setTimeout(attempt, 750);
  };
  attempt();
})
"#;
    let text = webview::extract_text(
        webview::ExtractRequest::new(viewer_url, script)
            .user_agent(UA)
            .wait_for_selector(".swiper")
            .timeout_ms(60_000)
            .cookies(true)
            .headless(true),
    )?;
    serde_json::from_str(&text)
        .map_err(|error| err(&format!("U-NEXT key extraction JSON error: {error}")))
}

fn process_unext_image(request: Value) -> ExtensionResult<ProcessedImage> {
    let image_base64 = request
        .get("imageBase64")
        .or_else(|| request.get("image_base64"))
        .and_then(Value::as_str)
        .ok_or_else(|| err("U-NEXT image processing did not receive image bytes"))?;
    let bytes = STANDARD
        .decode(image_base64)
        .map_err(|error| err(&format!("U-NEXT image base64 decode failed: {error}")))?;
    let meta = request
        .get("page")
        .and_then(|page| page.get("extra"))
        .and_then(|extra| extra.get("unext"))
        .ok_or_else(|| err("U-NEXT page metadata was missing"))?;
    let compressed_size = meta
        .get("compressedSize")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let key = meta
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| err("U-NEXT DRM key missing"))?;
    let iv = meta
        .get("iv")
        .and_then(Value::as_str)
        .ok_or_else(|| err("U-NEXT DRM IV missing"))?;
    let original_size = meta
        .get("originalFileSize")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let (method, compressed) = parse_local_file(&bytes, compressed_size)?;
    let mut payload = if method == 8 {
        let mut decoder = DeflateDecoder::new(compressed.as_slice());
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|error| err(&format!("U-NEXT ZIP inflate failed: {error}")))?;
        out
    } else {
        compressed
    };
    let key_bytes = STANDARD
        .decode(key)
        .map_err(|error| err(&format!("U-NEXT key decode failed: {error}")))?;
    let iv_bytes = STANDARD
        .decode(iv)
        .map_err(|error| err(&format!("U-NEXT IV decode failed: {error}")))?;
    let decrypted = Aes128CbcDec::new_from_slices(&key_bytes, &iv_bytes)
        .map_err(|_| err("U-NEXT AES key/IV length is invalid"))?
        .decrypt_padded_mut::<NoPadding>(&mut payload)
        .map_err(|_| err("U-NEXT AES decrypt failed"))?;
    let final_bytes = if original_size > 0 && original_size <= decrypted.len() {
        &decrypted[..original_size]
    } else {
        decrypted
    };
    Ok(ProcessedImage {
        image_base64: STANDARD.encode(final_bytes),
        mime_type: Some("image/webp".into()),
        ..ProcessedImage::default()
    })
}

#[derive(Debug, Clone)]
struct ZipIndex {
    url: String,
    zip_start_offset: u64,
    entries: Vec<CentralDirectoryRecord>,
}

impl ZipIndex {
    fn load(zip_url: &str) -> ExtensionResult<Self> {
        let content_length = content_length(zip_url)?;
        let eocd_start = content_length.saturating_sub(64 * 1024);
        let eocd_buffer = fetch_range(zip_url, eocd_start, content_length.saturating_sub(1))?;
        let eocd =
            parse_eocd(&eocd_buffer, eocd_start).ok_or_else(|| err("U-NEXT ZIP EOCD not found"))?;
        let absolute_cd_start = eocd
            .location_in_file
            .saturating_sub(eocd.central_directory_size);
        let zip_start_offset = absolute_cd_start.saturating_sub(eocd.central_directory_offset);
        let cd_start = zip_start_offset + eocd.central_directory_offset;
        let cd_end = cd_start + eocd.central_directory_size;
        let cd = fetch_range(zip_url, cd_start, cd_end)?;
        Ok(Self {
            url: zip_url.into(),
            zip_start_offset,
            entries: parse_central_directory(&cd),
        })
    }

    fn entry(&self, path: &str) -> Option<&CentralDirectoryRecord> {
        self.entries.iter().find(|entry| entry.filename == path)
    }

    fn fetch(&self, path: &str) -> ExtensionResult<Vec<u8>> {
        let entry = self
            .entry(path)
            .ok_or_else(|| err(&format!("U-NEXT ZIP entry not found: {path}")))?;
        let start = self.zip_start_offset + entry.local_file_header_relative_offset as u64;
        let end = start + entry.compressed_size as u64 + 512;
        let range = fetch_range(&self.url, start, end)?;
        let (method, compressed) = parse_local_file(&range, entry.compressed_size as usize)?;
        if method == 8 {
            let mut decoder = DeflateDecoder::new(compressed.as_slice());
            let mut out = Vec::new();
            decoder
                .read_to_end(&mut out)
                .map_err(|error| err(&format!("U-NEXT ZIP inflate failed: {error}")))?;
            Ok(out)
        } else {
            Ok(compressed)
        }
    }
}

#[derive(Debug, Clone)]
struct EndOfCentralDirectory {
    central_directory_size: u64,
    central_directory_offset: u64,
    location_in_file: u64,
}

#[derive(Debug, Clone)]
struct CentralDirectoryRecord {
    compressed_size: u32,
    local_file_header_relative_offset: u32,
    filename: String,
}

fn content_length(url: &str) -> ExtensionResult<u64> {
    let response = client().fetch("HEAD", url, None, Headers::new())?;
    response
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<u64>().ok())
        .ok_or_else(|| err("U-NEXT ZIP Content-Length was missing"))
}

fn fetch_range(url: &str, start: u64, end: u64) -> ExtensionResult<Vec<u8>> {
    let mut headers = Headers::new();
    headers.insert("Range".into(), format!("bytes={start}-{end}"));
    let response = client().fetch("GET", url, None, headers)?;
    let body = response
        .body_base64
        .ok_or_else(|| err("U-NEXT range response had no bytes"))?;
    STANDARD
        .decode(body)
        .map_err(|error| err(&format!("U-NEXT range base64 decode failed: {error}")))
}

fn parse_eocd(buffer: &[u8], buffer_start_offset: u64) -> Option<EndOfCentralDirectory> {
    if buffer.len() < 22 {
        return None;
    }
    for index in (0..=buffer.len() - 22).rev() {
        if le_u32(buffer, index)? == 0x0605_4b50 {
            return Some(EndOfCentralDirectory {
                central_directory_size: le_u32(buffer, index + 12)? as u64,
                central_directory_offset: le_u32(buffer, index + 16)? as u64,
                location_in_file: buffer_start_offset + index as u64,
            });
        }
    }
    None
}

fn parse_central_directory(buffer: &[u8]) -> Vec<CentralDirectoryRecord> {
    let mut out = Vec::new();
    let mut index = 0usize;
    while index + 46 <= buffer.len() {
        if le_u32(buffer, index) == Some(0x0201_4b50) {
            let name_len = le_u16(buffer, index + 28).unwrap_or(0) as usize;
            let extra_len = le_u16(buffer, index + 30).unwrap_or(0) as usize;
            let comment_len = le_u16(buffer, index + 32).unwrap_or(0) as usize;
            let name_start = index + 46;
            let name_end = name_start + name_len;
            if name_end <= buffer.len() {
                out.push(CentralDirectoryRecord {
                    compressed_size: le_u32(buffer, index + 20).unwrap_or(0),
                    local_file_header_relative_offset: le_u32(buffer, index + 42).unwrap_or(0),
                    filename: String::from_utf8_lossy(&buffer[name_start..name_end]).to_string(),
                });
            }
            index = name_start + name_len + extra_len + comment_len;
        } else {
            index += 1;
        }
    }
    out
}

fn parse_local_file(buffer: &[u8], compressed_size: usize) -> ExtensionResult<(u16, Vec<u8>)> {
    if buffer.len() < 30 || le_u32(buffer, 0) != Some(0x0403_4b50) {
        return Err(err("U-NEXT ZIP local file header was invalid"));
    }
    let method = le_u16(buffer, 8).unwrap_or(0);
    let name_len = le_u16(buffer, 26).unwrap_or(0) as usize;
    let extra_len = le_u16(buffer, 28).unwrap_or(0) as usize;
    let data_start = 30 + name_len + extra_len;
    let data_end = data_start + compressed_size;
    if data_start > buffer.len() {
        return Err(err("U-NEXT ZIP local data was truncated"));
    }
    Ok((
        method,
        buffer[data_start..data_end.min(buffer.len())].to_vec(),
    ))
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

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
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

fn key_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .map(|path| format!("/{}", path.trim_start_matches('/').trim_end_matches('/')))
}

fn absolute_url(path: &str) -> String {
    url::join_url(BASE_URL, path)
}

fn object_from_value(value: Value) -> BTreeMap<String, Value> {
    value
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn parse_json_value<T: serde::de::DeserializeOwned>(
    value: Value,
    label: &str,
) -> ExtensionResult<T> {
    serde_json::from_value(value)
        .map_err(|error| err(&format!("U-NEXT {label} JSON decode failed: {error}")))
}

fn parse_json_slice<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    label: &str,
) -> ExtensionResult<T> {
    serde_json::from_slice(bytes)
        .map_err(|error| err(&format!("U-NEXT {label} JSON decode failed: {error}")))
}

fn parse_json_str<T: serde::de::DeserializeOwned>(text: &str, label: &str) -> ExtensionResult<T> {
    serde_json::from_str(text)
        .map_err(|error| err(&format!("U-NEXT {label} JSON decode failed: {error}")))
}

fn err(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.to_string(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    page: u64,
    page_size: u64,
    results: u64,
}

impl PageInfo {
    fn has_next_page(&self) -> bool {
        self.page * self.page_size < self.results
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PopularResponse {
    book_ranking: BookRanking,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookRanking {
    books: Vec<BookRankingEntry>,
    page_info: PageInfo,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookRankingEntry {
    book_sakuhin: BookSakuhin,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LatestResponse {
    #[serde(rename = "webfront_newBooks")]
    new_books: BookPage,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    #[serde(rename = "webfront_bookFreewordSearch")]
    search: BookPage,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookPage {
    books: Vec<BookSakuhin>,
    page_info: PageInfo,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailsResponse {
    book_title: BookSakuhin,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterListResponse {
    #[serde(rename = "bookTitle_books")]
    book_title_books: BookTitleBooks,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookTitleBooks {
    books: Vec<Book>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaylistResponse {
    #[serde(rename = "webfront_bookPlaylistUrl")]
    playlist_url: PlaylistUrl,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaylistUrl {
    playlist_base_url: String,
    playlist_url: UBookContainer,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UBookContainer {
    ubooks: Vec<UBook>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UBook {
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookSakuhin {
    sakuhin_code: String,
    name: String,
    book: Book,
    detail: Option<SakuhinDetail>,
    is_completed: Option<bool>,
    subgenre_tag_list: Option<Vec<SubgenreTag>>,
}

impl BookSakuhin {
    fn to_item(self) -> Option<CatalogItem> {
        let key = format!("/book/title/{}", self.sakuhin_code);
        Some(CatalogItem {
            key: key.clone(),
            title: self.name,
            cover: self
                .book
                .thumbnail
                .and_then(|thumb| thumb.standard)
                .map(|value| format!("https://{value}")),
            description: self.detail.and_then(|detail| detail.introduction),
            status: if self.is_completed == Some(true) {
                ItemStatus::Completed
            } else {
                ItemStatus::Ongoing
            },
            tags: self
                .subgenre_tag_list
                .unwrap_or_default()
                .into_iter()
                .map(|tag| tag.name)
                .collect(),
            authors: self
                .book
                .credits
                .unwrap_or_default()
                .into_iter()
                .filter_map(|credit| credit.pen_name)
                .collect(),
            url: Some(absolute_url(&key)),
            language: Some("ja".into()),
            content_rating: Some("safe".into()),
            initialized: true,
            ..CatalogItem::default()
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SakuhinDetail {
    introduction: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubgenreTag {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Book {
    code: Option<String>,
    name: Option<String>,
    thumbnail: Option<Thumbnail>,
    public_start_date_time: Option<String>,
    is_free: Option<bool>,
    is_purchased: Option<bool>,
    rights_expiration_datetime: Option<String>,
    credits: Option<Vec<Credit>>,
    book_content: Option<BookContent>,
}

impl Book {
    fn is_readable(&self) -> bool {
        self.is_free == Some(true)
            || self.is_purchased == Some(true)
            || self.rights_expiration_datetime.is_some()
    }

    fn to_chapter(self, sakuhin_code: &str) -> Option<MangaChapter> {
        let readable = self.is_readable();
        let code = self.code?;
        let title = self.name.unwrap_or_else(|| code.clone());
        let file_code = self
            .book_content
            .and_then(|content| content.main_book_file)
            .map(|file| file.code);
        let lock = if readable { "" } else { "Locked " };
        let key = format!(
            "/book/view/{sakuhin_code}/{code}{}",
            file_code
                .map(|value| format!("#{value}"))
                .unwrap_or_default()
        );
        Some(MangaChapter {
            key: key.clone(),
            title: Some(format!("{lock}{title}")),
            url: Some(absolute_url(&key)),
            date_uploaded: self
                .public_start_date_time
                .as_deref()
                .and_then(parse_unext_date),
            is_locked: !readable,
            ..MangaChapter::default()
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Thumbnail {
    standard: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Credit {
    pen_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookContent {
    main_book_file: Option<BookFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookFile {
    code: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UBookIndex {
    pages: BTreeMap<String, UBookPage>,
    spine: Vec<UBookSpine>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UBookPage {
    image: UBookImage,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UBookImage {
    src: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UBookSpine {
    page_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UBookDrm {
    encrypted_file_list: BTreeMap<String, DrmFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DrmFile {
    iv: String,
    key_id: String,
    original_file_size: u64,
}

fn parse_unext_date(value: &str) -> Option<i64> {
    let date = value.split('T').next()?;
    manatan_shared::dates::parse_ymd(date)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_zip_eocd() {
        let mut bytes = vec![0; 32];
        bytes[10..14].copy_from_slice(&0x0605_4b50u32.to_le_bytes());
        bytes[22..26].copy_from_slice(&100u32.to_le_bytes());
        bytes[26..30].copy_from_slice(&20u32.to_le_bytes());
        let eocd = parse_eocd(&bytes, 5).unwrap();
        assert_eq!(eocd.central_directory_size, 100);
        assert_eq!(eocd.central_directory_offset, 20);
        assert_eq!(eocd.location_in_file, 15);
    }
}

export_manga_source!(SOURCE);
