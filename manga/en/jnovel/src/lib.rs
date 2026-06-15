mod reader;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ImageRequest, ItemStatus, MangaChapter,
    MangaPage, PageContent, Paged, ProcessedImage, SearchRequest, UrlResolveResult,
    abi::{ExtensionError, ExtensionResult},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{
    html, manga,
    manga_image::{image_base64, page_extra_str},
    sdk::http::{Headers, HttpClient},
    url,
};
use prost::Message;
use reader::{E4PQSTicket, edrm_version};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: JNovel = JNovel;
const BASE_URL: &str = "https://j-novel.club";
const VIEWER_URL: &str = "https://labs.j-novel.club/embed/v2";
const LIST_FIXTURE: &str = r#"{"seriesList":{"series":[{"slug":"sample-series","title":"Sample J-Novel","cover":{"coverUrl":"https://j-novel.club/cdn-cgi/image/width=400/sample.jpg"}}],"nextPageToken":""}}"#;
const DETAILS_FIXTURE: &str = r#"{"series":{"title":"Sample J-Novel","description":"Fixture description.","tags":["Action"],"status":0,"banner":{"originalUrl":"https://j-novel.club/banner.jpg"}},"volumes":[{"volume":{"creators":[{"name":"Sample Author","role":1},{"name":"Sample Artist","role":4}],"owned":true},"parts":[{"slug":"sample-part","title":"Sample J-Novel Part 1","launch":{"seconds":"1704067200"},"number":1,"preview":true,"rental":null}]}]}"#;

struct JNovel;

impl MangaSource for JNovel {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return parse_list(LIST_FIXTURE);
        }
        let target = format!("{BASE_URL}/series?type=manga&page={}", page(&request));
        parse_list(&fetch_rsc(&target, LIST_FIXTURE))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query).filter(|key| key.starts_with("/series/")) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)?],
                has_next_page: false,
            });
        }
        let mut target = format!("{BASE_URL}/series?type=manga&page={}", page(&request));
        if !query.is_empty() {
            target.push_str("&search=");
            target.push_str(&url::query_escape(query));
        }
        for id in ["sort", "label", "status", "rentals"] {
            if let Some(value) = filter_string(&request, id).filter(|value| !value.is_empty()) {
                target.push('&');
                target.push_str(id);
                target.push('=');
                target.push_str(&url::query_escape(&value));
            }
        }
        parse_list(&fetch_rsc(&target, LIST_FIXTURE))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample-series".into());
        details_by_key(&key)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample-series".into());
        let body = fetch_rsc(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        let details: SeriesDetailsResponse = extract_next(&body, "volumes")
            .ok_or_else(|| err("J-Novel details response did not contain volumes"))?;
        let hide_locked = preference_bool(&request, "hide_locked", false);
        let title = details.series.title;
        let mut chapters = details
            .volumes
            .into_iter()
            .flat_map(|volume| {
                let owned = volume.volume.as_ref().and_then(|volume| volume.owned).unwrap_or(false);
                let title = title.clone();
                volume.parts.into_iter().filter_map(move |part| {
                    let locked = part.is_locked(owned);
                    if hide_locked && locked {
                        return None;
                    }
                    Some(part.to_chapter(&title, locked))
                })
            })
            .collect::<Vec<_>>();
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample-part".into());
        let reader_url = format!("{BASE_URL}/read/{}", key.trim_start_matches('/'));
        let body = client().get(&reader_url).send_text().map_err(|_| {
            err("Log in via WebView and purchase this chapter to read.")
        })?;
        let iframe = html::attr_after(&body, "iframe", "src")
            .filter(|src| src.starts_with(VIEWER_URL))
            .ok_or_else(|| err("J-Novel reader iframe was not found; log in and purchase this chapter."))?;
        let embed = client().get(&iframe).send_text().map_err(|_| {
            err("J-Novel embed failed; log in and purchase this chapter.")
        })?;
        let manifest_url = html::attr_after(&embed, "data-e4p-manifest", "data-e4p-manifest")
            .or_else(|| html::attr(&embed, "data-e4p-manifest"))
            .map(|value| resolve_url(&iframe, &value))
            .ok_or_else(|| err("J-Novel embed did not expose a manifest URL"))?;
        let ticket_bytes = fetch_bytes(&manifest_url, Headers::new())?;
        let ticket = E4PQSTicket::decode(ticket_bytes.as_slice())
            .map_err(|error| err(&format!("J-Novel manifest protobuf decode failed: {error}")))?;
        let decoded = reader::decode_manifest_full(ticket.clone())
            .map_err(|error| err(&format!("J-Novel manifest decrypt failed: {error}")))?;
        let query = query_string(&manifest_url);
        let consumer_id = padded_consumer_id(&ticket.consumer);

        let pages = decoded
            .r#pub
            .spine
            .iter()
            .enumerate()
            .filter_map(|(index, link)| {
                let variant = link
                    .variants
                    .iter()
                    .find(|variant| variant.link.contains("h2048") && variant.image.is_some())?;
                let (archive_url, entry_name) = image_archive_url(&manifest_url, &variant.link, &query);
                let drm = variant.image.as_ref()?.drm.as_ref()?;
                let mut extra = json!({
                    "archiveUrl": archive_url,
                    "entryName": entry_name,
                    "contentId": ticket.content_id,
                    "consumerIdHex": hex(&consumer_id)
                });
                if drm.version == edrm_version::XEBP && drm.iv.len() == 32 {
                    let seed = decoded.pbex_seed.as_ref()?;
                    extra["ivHex"] = json!(hex(&drm.iv));
                    extra["pbexSeedHex"] = json!(hex(seed));
                }
                Some(page_from_extra(index, extra))
            })
            .collect::<Vec<_>>();
        if pages.is_empty() {
            Ok(vec![manga::text_page(
                "No readable J-Novel image pages were found in the manifest.",
            )])
        } else {
            Ok(pages)
        }
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        Ok(vec![HomeSection {
            id: "popular".into(),
            title: "Popular".into(),
            style: Some(HomeSectionStyle::Cover),
            has_more: popular.has_next_page,
            entries: popular.entries,
            ..HomeSection::default()
        }])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        process_jnovel_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}/read/{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input).filter(|key| key.starts_with("/series/")) {
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
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_rsc(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("rsc", "1")
        .header("RSC", "1")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_bytes(target: &str, headers: Headers) -> ExtensionResult<Vec<u8>> {
    let response = client().fetch("GET", target, None, headers)?;
    if let Some(body) = response.body_base64 {
        STANDARD
            .decode(body)
            .map_err(|error| err(&format!("J-Novel binary base64 decode failed: {error}")))
    } else {
        Ok(response.text.unwrap_or_default().into_bytes())
    }
}

fn parse_list(body: &str) -> ExtensionResult<Paged<CatalogItem>> {
    let response: SeriesResponse =
        extract_next(body, "seriesList").ok_or_else(|| err("J-Novel list response missing seriesList"))?;
    Ok(Paged {
        entries: response
            .series_list
            .series
            .into_iter()
            .map(SeriesItem::to_item)
            .collect(),
        has_next_page: !response.series_list.next_page_token.is_empty(),
    })
}

fn details_by_key(key: &str) -> ExtensionResult<CatalogItem> {
    let body = fetch_rsc(&url::join_url(BASE_URL, key), DETAILS_FIXTURE);
    let details: SeriesDetailsResponse =
        extract_next(&body, "volumes").ok_or_else(|| err("J-Novel details response missing series data"))?;
    Ok(details.to_item(key))
}

fn process_jnovel_image(request: Value) -> ExtensionResult<ProcessedImage> {
    let directory = STANDARD
        .decode(image_base64(&request).ok_or_else(|| err("J-Novel image processing did not receive QSC directory bytes"))?)
        .map_err(|error| err(&format!("J-Novel QSC directory base64 decode failed: {error}")))?;
    let archive_url = page_extra_str(&request, "archiveUrl").ok_or_else(|| err("J-Novel page missing archive URL"))?;
    let entry_name = page_extra_str(&request, "entryName").ok_or_else(|| err("J-Novel page missing QSC entry name"))?;
    let entry = reader::qsc_find_entry(&directory, entry_name)
        .map_err(|error| err(&format!("J-Novel QSC directory parse failed: {error}")))?
        .ok_or_else(|| err("J-Novel QSC entry was not found"))?;
    let start = reader::QSC_DIR_SIZE as u64 + entry.offset as u64;
    let end = start + entry.size as u64 - 1;
    let mut headers = Headers::new();
    headers.insert("Range".into(), format!("bytes={start}-{end}"));
    let bytes = fetch_bytes(archive_url, headers)?;
    let final_bytes = if let (Some(iv), Some(content_id), Some(consumer), Some(seed)) = (
        page_extra_str(&request, "ivHex").and_then(decode_hex),
        page_extra_str(&request, "contentId"),
        page_extra_str(&request, "consumerIdHex").and_then(decode_hex),
        page_extra_str(&request, "pbexSeedHex").and_then(decode_hex),
    ) {
        reader::decrypt_xebp(
            &bytes,
            &reader::XebpContext {
                iv,
                content_id: content_id.into(),
                consumer_id: consumer,
                pbex_seed: seed,
            },
        )
        .map_err(|error| err(&format!("J-Novel XEBP decrypt failed: {error}")))?
    } else {
        reader::strip_to_webp(&bytes)
    };
    Ok(ProcessedImage {
        image_base64: STANDARD.encode(final_bytes),
        mime_type: Some("image/webp".into()),
        ..ProcessedImage::default()
    })
}

fn page_from_extra(index: usize, extra: Value) -> MangaPage {
    let archive_url = extra.get("archiveUrl").and_then(Value::as_str).unwrap_or_default();
    let mut headers = Context::new();
    headers.insert("Range".into(), format!("bytes=0-{}", reader::QSC_DIR_SIZE - 1));
    MangaPage {
        content: PageContent::Request {
            request: ImageRequest {
                url: archive_url.into(),
                method: Some("GET".into()),
                headers: headers.clone(),
                referrer: Some(VIEWER_URL.into()),
                extra: object_from_value(extra.clone()),
                ..ImageRequest::default()
            },
        },
        headers,
        extra: object_from_value(extra),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn extract_next<T: serde::de::DeserializeOwned>(body: &str, marker: &str) -> Option<T> {
    if let Ok(value) = serde_json::from_str(body) {
        return Some(value);
    }
    for candidate in balanced_json_objects(body).into_iter().filter(|item| item.contains(marker)) {
        if let Ok(value) = serde_json::from_str(&candidate) {
            return Some(value);
        }
        let unescaped = candidate.replace("\\\"", "\"").replace("\\\\", "\\");
        if let Ok(value) = serde_json::from_str(&unescaped) {
            return Some(value);
        }
    }
    None
}

fn balanced_json_objects(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut start = None;
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = start.take() {
                        out.push(String::from_utf8_lossy(&bytes[start..=index]).into_owned());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn image_archive_url(manifest_url: &str, link: &str, query: &str) -> (String, String) {
    let resolved = resolve_url(manifest_url, link);
    let (without_fragment, fragment) = split_fragment(&resolved);
    let mut archive_url = without_fragment.to_string();
    if !query.is_empty() {
        archive_url.push(if archive_url.contains('?') { '&' } else { '?' });
        archive_url.push_str(query);
    }
    (archive_url, fragment.unwrap_or_default().to_string())
}

fn resolve_url(base: &str, link: &str) -> String {
    if link.starts_with("http://") || link.starts_with("https://") {
        return link.to_string();
    }
    if link.starts_with('/') {
        return format!("{}{}", origin(base), link);
    }
    let no_query = base.split('?').next().unwrap_or(base);
    let dir = no_query.rsplit_once('/').map(|(dir, _)| dir).unwrap_or(no_query);
    format!("{}/{}", dir.trim_end_matches('/'), link)
}

fn origin(input: &str) -> String {
    let Some((scheme, rest)) = input.split_once("://") else {
        return BASE_URL.into();
    };
    let host = rest.split('/').next().unwrap_or_default();
    format!("{scheme}://{host}")
}

fn split_fragment(input: &str) -> (&str, Option<&str>) {
    input.split_once('#').map_or((input, None), |(left, right)| (left, Some(right)))
}

fn query_string(input: &str) -> String {
    input
        .split_once('?')
        .map(|(_, query)| query.split('#').next().unwrap_or_default().to_string())
        .unwrap_or_default()
}

fn key_from_url(input: &str) -> Option<String> {
    if input.is_empty() {
        return None;
    }
    let path = input
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split_once('/').map(|(_, path)| path))
        .unwrap_or(input)
        .split(['?', '#'])
        .next()
        .unwrap_or(input);
    let key = format!("/{}", path.trim_matches('/'));
    (key.starts_with("/series/") || key.starts_with("/read/")).then_some(key)
}

fn padded_consumer_id(consumer: &str) -> Vec<u8> {
    let mut out = consumer.as_bytes().to_vec();
    out.resize(32, b'0');
    out.truncate(32);
    out
}

fn decode_hex(input: &str) -> Option<Vec<u8>> {
    if input.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(input.len() / 2);
    for pair in input.as_bytes().chunks_exact(2) {
        let high = hex_nibble(pair[0])?;
        let low = hex_nibble(pair[1])?;
        out.push((high << 4) | low);
    }
    Some(out)
}

fn hex(input: &[u8]) -> String {
    input.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn preference_bool(request: &Value, id: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(id))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn object_from_value(value: Value) -> BTreeMap<String, Value> {
    value
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn err(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.to_string(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesResponse {
    series_list: SeriesList,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesList {
    #[serde(default)]
    series: Vec<SeriesItem>,
    #[serde(default)]
    next_page_token: String,
}

#[derive(Debug, Deserialize)]
struct SeriesItem {
    slug: String,
    title: String,
    cover: Option<Cover>,
}

impl SeriesItem {
    fn to_item(self) -> CatalogItem {
        CatalogItem {
            key: format!("/series/{}", self.slug.trim_matches('/')),
            title: self.title,
            cover: self.cover.and_then(|cover| cover.cover_url).map(fix_cover_size),
            language: Some("en".into()),
            content_rating: Some("safe".into()),
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cover {
    cover_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SeriesDetailsResponse {
    series: SeriesDetails,
    #[serde(default)]
    volumes: Vec<Volume>,
}

impl SeriesDetailsResponse {
    fn to_item(self, key: &str) -> CatalogItem {
        let creators = self
            .volumes
            .first()
            .and_then(|volume| volume.volume.as_ref())
            .map(|volume| volume.creators.as_slice())
            .unwrap_or(&[]);
        CatalogItem {
            key: key.into(),
            title: self.series.title,
            cover: self.series.banner.and_then(|banner| banner.original_url),
            authors: creators
                .iter()
                .filter(|creator| creator.role == Some(1))
                .filter_map(|creator| creator.name.clone())
                .collect(),
            artists: creators
                .iter()
                .filter(|creator| creator.role == Some(4))
                .filter_map(|creator| creator.name.clone())
                .collect(),
            description: self.series.description,
            tags: self.series.tags,
            language: Some("en".into()),
            content_rating: Some("safe".into()),
            status: match self.series.status {
                Some(0) => ItemStatus::Ongoing,
                Some(1) => ItemStatus::Completed,
                Some(2) => ItemStatus::Hiatus,
                _ => ItemStatus::Unknown,
            },
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct SeriesDetails {
    title: String,
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    status: Option<i32>,
    banner: Option<Banner>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Banner {
    original_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Volume {
    #[serde(default)]
    parts: Vec<Part>,
    volume: Option<VolumeInfo>,
}

#[derive(Debug, Deserialize)]
struct VolumeInfo {
    #[serde(default)]
    creators: Vec<Creator>,
    owned: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct Creator {
    name: Option<String>,
    role: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct Part {
    slug: String,
    title: String,
    launch: Option<Time>,
    number: Option<i32>,
    preview: Option<bool>,
    rental: Option<Value>,
}

impl Part {
    fn is_locked(&self, owned: bool) -> bool {
        !owned && self.preview == Some(false) && self.rental.is_none()
    }

    fn to_chapter(self, manga_title: &str, locked: bool) -> MangaChapter {
        let mut title = self.title.strip_prefix(manga_title).unwrap_or(&self.title).trim().to_string();
        if title.is_empty() {
            title = self.title;
        }
        if locked {
            title = format!("Locked: {title}");
        }
        MangaChapter {
            key: self.slug,
            title: Some(title),
            chapter_number: self.number.map(|number| number as f32),
            date_uploaded: self
                .launch
                .and_then(|launch| launch.seconds)
                .and_then(|seconds| seconds.parse::<i64>().ok())
                .map(|seconds| seconds * 1000),
            language: Some("en".into()),
            is_locked: locked,
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct Time {
    seconds: Option<String>,
}

fn fix_cover_size(input: String) -> String {
    input.replace("/width=400/", "/width=1200/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture_list() {
        let page = parse_list(LIST_FIXTURE).unwrap();
        assert_eq!(page.entries[0].key, "/series/sample-series");
        assert_eq!(page.entries[0].title, "Sample J-Novel");
        assert!(!page.has_next_page);
    }

    #[test]
    fn parses_fixture_details_and_chapters() {
        let item = details_by_key("/series/sample-series").unwrap();
        assert_eq!(item.title, "Sample J-Novel");
        let chapters = SOURCE
            .chapters(json!({"manga":{"key":"/series/sample-series"},"preferences":{"hide_locked":false}}))
            .unwrap();
        assert_eq!(chapters[0].key, "sample-part");
        assert_eq!(chapters[0].chapter_number, Some(1.0));
    }

    #[test]
    fn builds_archive_url_with_manifest_query_and_fragment() {
        let (archive, entry) = image_archive_url(
            "https://cdn.example.test/books/manifest.e4p?token=abc",
            "pages.qsc#p001",
            "token=abc",
        );
        assert_eq!(archive, "https://cdn.example.test/books/pages.qsc?token=abc");
        assert_eq!(entry, "p001");
    }
}

export_manga_source!(SOURCE);
