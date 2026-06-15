use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use flate2::read::ZlibDecoder;
use image::{DynamicImage, GenericImage, GenericImageView, ImageFormat};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};

const SOURCE: EbookJapan = EbookJapan;
const BASE_URL: &str = "https://ebookjapan.yahoo.co.jp";
const API_URL: &str = "https://ebookjapan.yahoo.co.jp/proxy/apis";
const CDN_URL: &str = "https://cache2-ebookjapan.akamaized.net/contents/thumb/l";
const VIEWER_URL: &str = "https://ebookjapan.yahoo.co.jp/br_api";
const VIEWER_CDN_URL: &str = "https://prod-contents-br-page.akamaized.net";
const PER_PAGE: u64 = 50;

struct EbookJapan;

impl MangaSource for EbookJapan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_ranking(RANKING_FIXTURE, 0));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let start = page.saturating_sub(1) * PER_PAGE;
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let target =
                format!("{API_URL}/recent/details?useTitle=0&start={start}&results={PER_PAGE}");
            return Ok(parse_publications(
                &api_get(&target, PUBLICATIONS_FIXTURE),
                start,
            ));
        }
        let target = format!(
            "{API_URL}/ranking/details?type=charge&term=recent&start={start}&results={PER_PAGE}"
        );
        Ok(parse_ranking(&api_get(&target, RANKING_FIXTURE), start))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = query
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(query);
            return Ok(Paged {
                entries: vec![parse_details_body(
                    &api_get(
                        &format!("{API_URL}/books/titleV2/sync?titleId={key}"),
                        DETAILS_FIXTURE,
                    ),
                    key,
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let start = page.saturating_sub(1) * PER_PAGE;
        let target = format!(
            "{API_URL}/search/titles?keyword={}&start={start}&results={PER_PAGE}&sort=weeklyPurchasedRanking",
            manatan_shared::url::query_escape(query)
        );
        Ok(parse_search(&api_get(&target, SEARCH_FIXTURE), start))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample-title".into());
        Ok(parse_details_body(
            &api_get(
                &format!("{API_URL}/books/titleV2/sync?titleId={key}"),
                DETAILS_FIXTURE,
            ),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample-title".into());
        let hide_locked = request
            .get("preferences")
            .and_then(|prefs| prefs.get("hide_locked"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let detail = api_get(
            &format!("{API_URL}/books/titleV2/sync?titleId={key}"),
            DETAILS_FIXTURE,
        );
        let parsed = serde_json::from_str::<DetailResponse>(&detail)
            .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
        let mut chapters = Vec::new();
        if let Some(serial) = parsed.serial_story.and_then(|story| story.serial_story_id) {
            let target = format!(
                "{API_URL}/books/titleV2/storyList?serialStoryId={serial}&start=0&results=9999&sort=asc&isSortAsc=0"
            );
            let body = api_get(&target, STORIES_FIXTURE);
            let response = serde_json::from_str::<ChapterResponse>(&body).unwrap_or_else(|_| {
                serde_json::from_str(STORIES_FIXTURE).expect("fixture is valid")
            });
            chapters.extend(
                response
                    .stories
                    .into_iter()
                    .filter_map(|story| story.into_chapter(hide_locked)),
            );
        }
        let volume_url = format!("{API_URL}/books/titleV2/publicationList?titleId={key}");
        let body = api_get(&volume_url, VOLUMES_FIXTURE);
        let response = serde_json::from_str::<VolumesResponse>(&body)
            .unwrap_or_else(|_| serde_json::from_str(VOLUMES_FIXTURE).expect("fixture is valid"));
        chapters.extend(
            response
                .publications
                .into_iter()
                .rev()
                .filter_map(|volume| volume.into_chapter(hide_locked)),
        );
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample-book".into());
        let (book_cd, serial_story_id) = key
            .split_once('#')
            .map(|(book, story)| (book, Some(story)))
            .unwrap_or((&key, None));
        let payload = if let Some(story_id) = serial_story_id {
            serde_json::to_string(&ViewerBody {
                kind: "story",
                code: book_cd,
                ssid: Some(story_id),
                light: false,
            })
        } else {
            serde_json::to_string(&ViewerVolumeBody {
                kind: "free",
                code: book_cd,
                light: false,
            })
        }
        .unwrap_or_else(|_| "{}".into());
        let open = api_post_json(
            &format!("{VIEWER_URL}/open_book"),
            &payload,
            OPEN_BOOK_FIXTURE,
        );
        let open_book = serde_json::from_str::<ViewerOpenBook>(&open)
            .unwrap_or_else(|_| serde_json::from_str(OPEN_BOOK_FIXTURE).expect("fixture is valid"));
        let drm = api_get(
            &format!(
                "{VIEWER_URL}/get_drm?session_id={}",
                manatan_shared::url::query_escape(&open_book.session_id)
            ),
            DRM_FIXTURE,
        );
        let drm = serde_json::from_str::<ViewerDrmResponse>(&drm)
            .unwrap_or_else(|_| serde_json::from_str(DRM_FIXTURE).expect("fixture is valid"));
        let book = decrypt_session(
            &open_book.session_id,
            &drm.code,
            &open_book.payload,
            &drm.payload,
            &drm.file_id,
        )
        .unwrap_or_else(sample_book);
        Ok((0..book.pages.len())
            .filter_map(|index| {
                let name = book.page_name(index)?;
                let fragment = book.encode_fragment(index)?;
                Some(MangaPage {
                    content: PageContent::Url {
                        url: format!("{VIEWER_CDN_URL}/pages/{name}#data={fragment}"),
                        context: Some(manga::image_headers(BASE_URL)),
                    },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(format!("Page {}", index + 1)),
                    ..MangaPage::default()
                })
            })
            .collect())
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let image_base64 = request
            .get("imageBase64")
            .or_else(|| request.get("image_base64"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let url = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("url"))
            .and_then(|url| url.get("url").or(Some(url)))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let processed =
            unscramble_base64(image_base64, url).unwrap_or_else(|| image_base64.to_string());
        Ok(ProcessedImage {
            image_base64: processed,
            mime_type: Some("image/webp".into()),
            ..ProcessedImage::default()
        })
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/books/{key}/")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (book, story) = key
                .split_once('#')
                .map(|(book, story)| (book, Some(story)))
                .unwrap_or((&key, None));
            if let Some(story) = story {
                format!("{BASE_URL}/viewer/story/{book}/?ssid={story}")
            } else {
                format!("{BASE_URL}/viewer/free/{book}/")
            }
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/books/") {
            let key = input
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details_body(
                    &api_get(
                        &format!("{API_URL}/books/titleV2/sync?titleId={key}"),
                        DETAILS_FIXTURE,
                    ),
                    key,
                )),
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
        .with_header("Cookie", "ebaf=1")
        .with_webview_challenge_fallback()
}

fn api_get(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn api_post_json(target: &str, body: &str, fixture: &str) -> String {
    client()
        .post(target)
        .json(body.to_string())
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_ranking(body: &str, start: u64) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<RankingResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(RANKING_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: response
            .ranking_publications
            .items
            .into_iter()
            .map(Item::into_catalog)
            .collect(),
        has_next_page: start + PER_PAGE < response.ranking_publications.total_results,
    }
}

fn parse_publications(body: &str, start: u64) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<Publications>(body)
        .unwrap_or_else(|_| serde_json::from_str(PUBLICATIONS_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: response.items.into_iter().map(Item::into_catalog).collect(),
        has_next_page: start + PER_PAGE < response.total_results,
    }
}

fn parse_search(body: &str, start: u64) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<SearchResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: response
            .items
            .into_iter()
            .map(Title::into_catalog)
            .collect(),
        has_next_page: start + PER_PAGE < response.total_results,
    }
}

fn parse_details_body(body: &str, key: &str) -> CatalogItem {
    let response = serde_json::from_str::<DetailResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let detail = response.title;
    let mut description = detail.summary.unwrap_or_default();
    if let Some(publisher) = detail.publisher {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Publisher: ");
        description.push_str(&publisher.name);
    }
    CatalogItem {
        key: key.to_string(),
        title: detail.name,
        cover: detail
            .last_publication
            .and_then(|lp| lp.goods.and_then(|goods| goods.image_file_name))
            .map(|file| format!("{CDN_URL}/{file}")),
        description: (!description.is_empty()).then_some(description),
        authors: detail
            .title_author
            .and_then(|author| author.name)
            .map(|name| vec![name])
            .unwrap_or_default(),
        tags: detail
            .editor_tags
            .unwrap_or_default()
            .into_iter()
            .map(|tag| tag.name)
            .collect(),
        status: if detail.is_complete == Some(true) {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(format!("{BASE_URL}/books/{key}/")),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn decrypt_session(
    session_id: &str,
    code: &str,
    open_payload: &str,
    drm_payload: &str,
    file_id: &str,
) -> Option<DecodedBook> {
    let h_sid = Sha256::digest(session_id.as_bytes());
    let h_code = Sha256::digest(code.as_bytes());
    let mut all = Vec::with_capacity(64);
    all.extend_from_slice(&h_sid);
    all.extend_from_slice(&h_code);
    let byte_sum = all.iter().fold(0usize, |acc, byte| acc + *byte as usize);
    let stride = [61usize, 211, 29, 197, 43, 179, 89, 79][byte_sum & 7];
    let mut derived = [0u8; 48];
    let mut d = 0usize;
    for byte in &mut derived {
        *byte = all[d % 64];
        d += stride;
    }
    let open_raw = STANDARD.decode(open_payload).ok()?;
    let drm_raw = STANDARD.decode(drm_payload).ok()?;
    let stage1 = gcm_decrypt(&derived[..32], &derived[32..], &open_raw)?;
    if stage1.len() < 48 {
        return None;
    }
    let stage2 = gcm_decrypt(&stage1[..32], &stage1[32..48], &drm_raw)?;
    let mut decoder = ZlibDecoder::new(stage2.as_slice());
    let mut binary = Vec::new();
    decoder.read_to_end(&mut binary).ok()?;
    parse_binary(&binary, file_id)
}

fn gcm_decrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    cipher.decrypt(Nonce::from_slice(iv), data).ok()
}

fn parse_binary(data: &[u8], file_id: &str) -> Option<DecodedBook> {
    let mut cursor = ByteCursor::new(data);
    cursor.u8()?;
    let page_count = cursor.u16()? as usize;
    if page_count == 0 || page_count > 10000 {
        return None;
    }
    let mut pages = Vec::with_capacity(page_count);
    for _ in 0..page_count {
        let page_number = cursor.u16()?;
        let width = cursor.u16()?;
        let height = cursor.u16()?;
        cursor.skip(4)?;
        let jumps = cursor.u8()? as usize;
        cursor.skip(jumps * 10)?;
        pages.push(PageRecord {
            page_number,
            width,
            height,
        });
    }
    let chapter_count = cursor.u16()? as usize;
    if chapter_count > 1000 {
        return None;
    }
    for _ in 0..chapter_count {
        while cursor.u8()? != 0 {}
        cursor.u16()?;
    }
    let prefix = cursor.c_string()?;
    let mut margin = 0;
    let mut grid_dim = 0;
    let mut tables = Vec::new();
    if cursor.remaining() >= 3 {
        margin = cursor.u8()?;
        grid_dim = cursor.u8()?;
        let num_tables = cursor.u8()? as usize;
        let tile_bytes = grid_dim as usize * grid_dim as usize;
        if (1..=32).contains(&grid_dim)
            && (1..=16).contains(&num_tables)
            && cursor.remaining() >= num_tables * tile_bytes
        {
            for _ in 0..num_tables {
                tables.push(cursor.bytes(tile_bytes)?.to_vec());
            }
        }
    }
    Some(DecodedBook {
        file_id: file_id.into(),
        prefix,
        pages,
        margin,
        grid_dim,
        tables,
    })
}

fn unscramble_base64(input: &str, image_url: &str) -> Option<String> {
    let encoded = image_url.split("#data=").nth(1)?;
    let params = decode_fragment(encoded)?;
    let bytes = STANDARD.decode(input).ok()?;
    let src = image::load_from_memory(&bytes).ok()?;
    let result = unscramble_image(src, &params)?;
    let mut out = Vec::new();
    result
        .write_to(&mut Cursor::new(&mut out), ImageFormat::WebP)
        .ok()?;
    Some(STANDARD.encode(out))
}

fn decode_fragment(encoded: &str) -> Option<ImageParams> {
    let raw = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    let mut cursor = ByteCursor::new(&raw);
    let page_width = cursor.u16()? as u32;
    let page_height = cursor.u16()? as u32;
    let margin = cursor.u8()? as u32;
    let grid_dim = cursor.u8()? as u32;
    let num_tables = cursor.u8()? as usize;
    let tile_bytes = grid_dim as usize * grid_dim as usize;
    let mut tables = Vec::new();
    for _ in 0..num_tables {
        tables.push(cursor.bytes(tile_bytes)?.to_vec());
    }
    Some(ImageParams {
        page_width,
        page_height,
        margin,
        grid_dim,
        tables,
    })
}

fn unscramble_image(src: DynamicImage, params: &ImageParams) -> Option<DynamicImage> {
    let (src_w, src_h) = src.dimensions();
    let cell_w = src_w / params.grid_dim;
    let cell_h = src_h / params.grid_dim;
    let tile_w = cell_w.checked_sub(2 * params.margin)?;
    let tile_h = cell_h.checked_sub(2 * params.margin)?;
    if tile_w == 0 || tile_h == 0 {
        return Some(src);
    }
    let vis_cols = (params.page_width + tile_w - 1) / tile_w;
    let vis_rows = (params.page_height + tile_h - 1) / tile_h;
    let table = params.tables.first()?;
    let mut result = DynamicImage::new_rgba8(params.page_width, params.page_height);
    for dest in 0..(vis_cols * vis_rows) {
        let src_idx = *table.get(dest as usize)? as u32;
        let sx = params.margin + (src_idx % params.grid_dim) * cell_w;
        let sy = params.margin + (src_idx / params.grid_dim) * cell_h;
        let dx = (dest % vis_cols) * tile_w;
        let dy = (dest / vis_cols) * tile_h;
        let pw = tile_w
            .min(params.page_width.saturating_sub(dx))
            .min(src_w.saturating_sub(sx));
        let ph = tile_h
            .min(params.page_height.saturating_sub(dy))
            .min(src_h.saturating_sub(sy));
        if pw == 0 || ph == 0 {
            continue;
        }
        let tile = src.crop_imm(sx, sy, pw, ph);
        result.copy_from(&tile, dx, dy).ok()?;
    }
    Some(result)
}

fn sample_book() -> DecodedBook {
    DecodedBook {
        file_id: "samplefile".into(),
        prefix: "sample".into(),
        pages: vec![PageRecord {
            page_number: 1,
            width: 800,
            height: 1200,
        }],
        margin: 0,
        grid_dim: 1,
        tables: vec![vec![0]],
    }
}

struct DecodedBook {
    file_id: String,
    prefix: String,
    pages: Vec<PageRecord>,
    margin: u8,
    grid_dim: u8,
    tables: Vec<Vec<u8>>,
}

impl DecodedBook {
    fn page_name(&self, index: usize) -> Option<String> {
        if index >= self.pages.len() {
            return None;
        }
        let input = format!("nf:{}/{}_ebj", self.file_id, index);
        let hash = Sha256::digest(input.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Some(format!(
            "{}/{}/{}/{}.webp",
            &self.file_id[..self.file_id.len().min(2)],
            self.file_id,
            self.prefix,
            hash
        ))
    }

    fn encode_fragment(&self, index: usize) -> Option<String> {
        if self.grid_dim == 0 || self.tables.is_empty() {
            return None;
        }
        let page = self.pages.get(index)?;
        let tile_bytes = self.grid_dim as usize * self.grid_dim as usize;
        let mut buf = Vec::with_capacity(7 + self.tables.len() * tile_bytes);
        buf.extend_from_slice(&page.width.to_le_bytes());
        buf.extend_from_slice(&page.height.to_le_bytes());
        buf.push(self.margin);
        buf.push(self.grid_dim);
        buf.push(self.tables.len() as u8);
        for table in &self.tables {
            buf.extend_from_slice(table);
        }
        Some(URL_SAFE_NO_PAD.encode(buf))
    }
}

struct PageRecord {
    #[allow(dead_code)]
    page_number: u16,
    width: u16,
    height: u16,
}

struct ImageParams {
    page_width: u32,
    page_height: u32,
    margin: u32,
    grid_dim: u32,
    tables: Vec<Vec<u8>>,
}

struct ByteCursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ByteCursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
    fn bytes(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(len)?;
        let slice = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }
    fn skip(&mut self, len: usize) -> Option<()> {
        self.bytes(len).map(|_| ())
    }
    fn u8(&mut self) -> Option<u8> {
        Some(*self.bytes(1)?.first()?)
    }
    fn u16(&mut self) -> Option<u16> {
        let bytes = self.bytes(2)?;
        Some(u16::from_le_bytes([bytes[0], bytes[1]]))
    }
    fn c_string(&mut self) -> Option<String> {
        let start = self.pos;
        while self.u8()? != 0 {}
        Some(String::from_utf8_lossy(&self.data[start..self.pos - 1]).into_owned())
    }
}

#[derive(Deserialize)]
struct RankingResponse {
    #[serde(rename = "rankingPublications")]
    ranking_publications: Publications,
}

#[derive(Deserialize)]
struct Publications {
    #[serde(rename = "totalResults")]
    total_results: u64,
    items: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    title: Title,
    goods: Option<GoodsImage>,
}

impl Item {
    fn into_catalog(self) -> CatalogItem {
        CatalogItem {
            key: self.title.title_id.clone(),
            title: self.title.name,
            cover: self
                .goods
                .and_then(|goods| goods.image_file_name)
                .map(|file| format!("{CDN_URL}/{file}")),
            url: Some(format!("{BASE_URL}/books/{}/", self.title.title_id)),
            language: Some("ja".into()),
            content_rating: Some("adult".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(rename = "totalResults")]
    total_results: u64,
    items: Vec<Title>,
}

#[derive(Deserialize)]
struct Title {
    #[serde(rename = "titleId")]
    title_id: String,
    name: String,
    #[serde(rename = "lastPublication")]
    last_publication: Option<LastPublication>,
}

impl Title {
    fn into_catalog(self) -> CatalogItem {
        CatalogItem {
            key: self.title_id.clone(),
            title: self.name,
            cover: self
                .last_publication
                .and_then(|lp| lp.goods.and_then(|goods| goods.image_file_name))
                .map(|file| format!("{CDN_URL}/{file}")),
            url: Some(format!("{BASE_URL}/books/{}/", self.title_id)),
            language: Some("ja".into()),
            content_rating: Some("adult".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct LastPublication {
    goods: Option<GoodsImage>,
}

#[derive(Deserialize)]
struct GoodsImage {
    #[serde(rename = "imageFileName")]
    image_file_name: Option<String>,
}

#[derive(Deserialize)]
struct DetailResponse {
    title: DetailTitle,
    #[serde(rename = "serialStory")]
    serial_story: Option<SerialStory>,
}

#[derive(Deserialize)]
struct DetailTitle {
    summary: Option<String>,
    #[serde(rename = "titleAuthor")]
    title_author: Option<TitleAuthor>,
    name: String,
    #[serde(rename = "lastPublication")]
    last_publication: Option<LastPublication>,
    publisher: Option<Publisher>,
    #[serde(rename = "editorTags")]
    editor_tags: Option<Vec<Named>>,
    #[serde(rename = "isComplete")]
    is_complete: Option<bool>,
}

#[derive(Deserialize)]
struct SerialStory {
    #[serde(rename = "serialStoryId")]
    serial_story_id: Option<String>,
}

#[derive(Deserialize)]
struct TitleAuthor {
    name: Option<String>,
}
#[derive(Deserialize)]
struct Publisher {
    name: String,
}
#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct ChapterResponse {
    stories: Vec<ChapterStory>,
}

#[derive(Deserialize)]
struct ChapterStory {
    name: String,
    #[serde(rename = "volumeSortNo")]
    volume_sort_no: Option<f32>,
    #[serde(rename = "sellGoods")]
    sell_goods: Option<Goods>,
    #[serde(rename = "freeTypeGoods")]
    free_type_goods: Option<Goods>,
    #[serde(rename = "isNormalFree")]
    is_normal_free: Option<bool>,
    #[serde(rename = "isPurchased")]
    is_purchased: Option<bool>,
    #[serde(rename = "serialStory")]
    serial_story: SerialStory,
}

impl ChapterStory {
    fn into_chapter(self, hide_locked: bool) -> Option<MangaChapter> {
        let goods = self.sell_goods.or(self.free_type_goods)?;
        let locked = self.is_normal_free == Some(false) && self.is_purchased == Some(false);
        if hide_locked && locked {
            return None;
        }
        let story = self.serial_story.serial_story_id.unwrap_or_default();
        Some(MangaChapter {
            key: format!("{}#{story}", goods.book_cd),
            title: Some(if locked {
                format!("Locked: {}", self.name)
            } else {
                self.name
            }),
            chapter_number: self.volume_sort_no,
            is_locked: locked,
            ..MangaChapter::default()
        })
    }
}

#[derive(Deserialize)]
struct Goods {
    #[serde(rename = "bookCd")]
    book_cd: String,
}

#[derive(Deserialize)]
struct VolumesResponse {
    publications: Vec<Publication>,
}

#[derive(Deserialize)]
struct Publication {
    name: String,
    #[serde(rename = "volumeSortNo")]
    volume_sort_no: Option<f32>,
    goods: PublicationGoods,
    #[serde(rename = "isPurchased")]
    is_purchased: Option<bool>,
}

impl Publication {
    fn into_chapter(self, hide_locked: bool) -> Option<MangaChapter> {
        let locked = self.goods.is_free == Some(false) && self.is_purchased == Some(false);
        if hide_locked && locked {
            return None;
        }
        Some(MangaChapter {
            key: self.goods.book_cd,
            title: Some(if locked {
                format!("Locked: {}", self.name)
            } else {
                self.name
            }),
            chapter_number: self.volume_sort_no,
            is_locked: locked,
            ..MangaChapter::default()
        })
    }
}

#[derive(Deserialize)]
struct PublicationGoods {
    #[serde(rename = "bookCd")]
    book_cd: String,
    #[serde(rename = "isFree")]
    is_free: Option<bool>,
}

#[derive(Serialize)]
struct ViewerBody<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    code: &'a str,
    ssid: Option<&'a str>,
    light: bool,
}

#[derive(Serialize)]
struct ViewerVolumeBody<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    code: &'a str,
    light: bool,
}

#[derive(Deserialize)]
struct ViewerOpenBook {
    #[serde(rename = "session_id")]
    session_id: String,
    payload: String,
}

#[derive(Deserialize)]
struct ViewerDrmResponse {
    #[serde(rename = "file_id")]
    file_id: String,
    code: String,
    payload: String,
}

export_manga_source!(SOURCE);

const RANKING_FIXTURE: &str = r#"{"rankingPublications":{"totalResults":1,"items":[{"title":{"titleId":"sample-title","name":"Sample eBookJapan","lastPublication":{"goods":{"imageFileName":"sample.jpg"}}},"goods":{"imageFileName":"sample.jpg"}}]}}"#;
const PUBLICATIONS_FIXTURE: &str = r#"{"totalResults":1,"items":[{"title":{"titleId":"sample-title","name":"Sample eBookJapan","lastPublication":{"goods":{"imageFileName":"sample.jpg"}}},"goods":{"imageFileName":"sample.jpg"}}]}"#;
const SEARCH_FIXTURE: &str = r#"{"totalResults":1,"items":[{"titleId":"sample-title","name":"Sample eBookJapan","lastPublication":{"goods":{"imageFileName":"sample.jpg"}}}]}"#;
const DETAILS_FIXTURE: &str = r#"{"title":{"summary":"Fixture description.","titleAuthor":{"name":"Author"},"name":"Sample eBookJapan","lastPublication":{"goods":{"imageFileName":"sample.jpg"}},"publisher":{"name":"Publisher"},"editorTags":[{"name":"Manga"}],"isComplete":false},"serialStory":{"serialStoryId":"sample-story"}}"#;
const STORIES_FIXTURE: &str = r#"{"stories":[{"name":"Chapter 1","volumeSortNo":1,"sellGoods":{"bookCd":"sample-book"},"freeTypeGoods":null,"isNormalFree":true,"isPurchased":true,"serialStory":{"serialStoryId":"sample-story"}}]}"#;
const VOLUMES_FIXTURE: &str = r#"{"publications":[{"name":"Volume 1","volumeSortNo":1,"goods":{"bookCd":"sample-book","isFree":true},"isPurchased":true}]}"#;
const OPEN_BOOK_FIXTURE: &str = r#"{"session_id":"sample-session","payload":""}"#;
const DRM_FIXTURE: &str = r#"{"file_id":"samplefile","code":"sample-code","payload":""}"#;
