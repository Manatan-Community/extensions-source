use aes::Aes128;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    dates, html, manga,
    sdk::http::{Headers, HttpClient},
    url,
};
use prost::Message;
use serde_json::Value;

const SOURCE: MangaOne = MangaOne;
const BASE_URL: &str = "https://manga-one.com";
const API_URL: &str = "https://manga-one.com/api/client";

struct MangaOne;

impl MangaSource for MangaOne {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listingId")
            .or_else(|| request.get("listing"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            let latest = fetch_proto::<LatestResponse>(
                &format!("{API_URL}?rq=rensai"),
                "POST",
                LATEST_FIXTURE,
            )
            .unwrap_or_else(sample_latest);
            let entries = latest
                .list
                .into_iter()
                .flat_map(|list| list.response_list)
                .filter_map(|wrapper| {
                    wrapper
                        .titles
                        .and_then(|titles| titles.entry)
                        .map(Entry::to_item)
                })
                .collect();
            return Ok(Paged {
                entries,
                has_next_page: false,
            });
        }
        let ranking = fetch_proto::<RankingResponse>(
            &format!("{API_URL}?rq=ranking"),
            "GET",
            RANKING_FIXTURE,
        )
        .unwrap_or_else(sample_ranking);
        let entries = ranking
            .categories
            .into_iter()
            .flat_map(|category| category.ranking_lists)
            .find(|list| list.kind == "すべて")
            .map(|list| {
                list.titles
                    .into_iter()
                    .filter_map(|title| title.entry.map(RankingEntry::to_item))
                    .collect()
            })
            .unwrap_or_else(Vec::new);
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query).filter(|key| !key.contains('#')) {
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            let tag_id = filter_string(&request, "tag_id").unwrap_or_default();
            if tag_id.is_empty() {
                format!("{API_URL}?rq=title/search")
            } else {
                format!(
                    "{API_URL}?rq=title/search&tag_id={}",
                    url::query_escape(tag_id)
                )
            }
        } else {
            format!(
                "{API_URL}?rq=title/search&query={}",
                url::query_escape(query)
            )
        };
        let response = fetch_proto::<ResponseList>(&target, "POST", SEARCH_FIXTURE)
            .unwrap_or_else(sample_response_list);
        Ok(Paged {
            entries: response
                .response_list
                .into_iter()
                .filter_map(|wrapper| {
                    wrapper
                        .titles
                        .and_then(|titles| titles.entry)
                        .map(Entry::to_item)
                })
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let hide_locked = preference_bool(&request, "hide_locked");
        let target = format!(
            "{API_URL}?rq=viewer/chapter_list&title_id={key}&page=1&limit=9999&sort_type=desc&type=chapter"
        );
        let response = fetch_proto::<ChapterResponse>(&target, "GET", CHAPTERS_FIXTURE)
            .unwrap_or_else(sample_chapters);
        Ok(response
            .chapters
            .and_then(|chapters| Some(chapters.chapter_list))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|chapter| chapter.to_chapter(&key, hide_locked))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1#1".into());
        let mut parts = key.split('#');
        let chapter_id = parts.next().unwrap_or("1");
        let title_id = parts.next().unwrap_or("1");
        let target = format!("{API_URL}?rq=viewer_v2&title_id={title_id}&chapter_id={chapter_id}");
        let response = fetch_proto::<ViewerResponse>(&target, "POST", PAGES_FIXTURE)
            .unwrap_or_else(sample_viewer);
        if response.pages.is_empty() {
            return Ok(vec![manga::text_page(
                "Log in via WebView and rent or purchase this chapter to read.",
            )]);
        }
        Ok(response
            .pages
            .into_iter()
            .filter_map(|wrapper| {
                let page = wrapper.page?;
                Some(MangaPage {
                    content: PageContent::Url {
                        url: format!("{}#{}:{}", page.url, response.key, response.iv),
                        context: Some(manga::image_headers(BASE_URL)),
                    },
                    headers: manga::image_headers(BASE_URL),
                    ..MangaPage::default()
                })
            })
            .collect())
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let input = request
            .get("imageBase64")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(image_url) = page_image_url(&request) else {
            return passthrough_image(&request);
        };
        let Some(fragment) = image_url
            .split('#')
            .next_back()
            .filter(|part| part.contains(':'))
        else {
            return passthrough_image(&request);
        };
        let mut parts = fragment.split(':');
        let (Some(key_hex), Some(iv_hex)) = (parts.next(), parts.next()) else {
            return passthrough_image(&request);
        };
        let (Ok(mut bytes), Some(key), Some(iv)) = (
            STANDARD.decode(input),
            decode_hex(key_hex),
            decode_hex(iv_hex),
        ) else {
            return passthrough_image(&request);
        };
        if key.len() != 16 || iv.len() != 16 {
            return passthrough_image(&request);
        }
        let Ok(decrypted) =
            cbc::Decryptor::<Aes128>::new_from_slices(&key, &iv).and_then(|cipher| {
                cipher
                    .decrypt_padded_mut::<Pkcs7>(&mut bytes)
                    .map_err(|_| cbc::cipher::InvalidLength)
            })
        else {
            return passthrough_image(&request);
        };
        Ok(ProcessedImage {
            image_base64: STANDARD.encode(decrypted),
            mime_type: request
                .get("mimeType")
                .and_then(Value::as_str)
                .map(str::to_string),
            ..ProcessedImage::default()
        })
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input).filter(|key| !key.contains('#')) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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

fn fetch_proto<T: Message + Default>(target: &str, method: &str, fixture_hex: &str) -> Option<T> {
    let mut headers = Headers::new();
    headers.insert("Accept".into(), "application/protobuf".into());
    client()
        .fetch(method, target, None, headers)
        .ok()
        .and_then(|response| {
            response
                .body_base64
                .and_then(|body| STANDARD.decode(body).ok())
                .or_else(|| response.text.map(|text| text.into_bytes()))
        })
        .or_else(|| Some(hex_decode(fixture_hex)))
        .and_then(|bytes| T::decode(bytes.as_slice()).ok())
}

fn details_from_key(key: &str) -> CatalogItem {
    let target = format!("{API_URL}?rq=viewer_v2&title_id={key}");
    fetch_proto::<DetailResponse>(&target, "POST", DETAILS_FIXTURE)
        .and_then(|response| {
            response
                .detail_entry
                .and_then(|entry| entry.details)
                .map(|details| details.to_item(key))
        })
        .unwrap_or_else(|| sample_details().to_item(key))
}

fn key_from_url(input: &str) -> Option<String> {
    if input.is_empty() {
        return None;
    }
    if !input.starts_with("http") && !input.starts_with('/') {
        return Some(input.into());
    }
    input
        .find("/title/")
        .map(|index| {
            input[index + "/title/".len()..]
                .split(['?', '/', '#'])
                .next()
                .unwrap_or("1")
                .to_string()
        })
        .or_else(|| {
            let title = input.find("/manga/")?;
            let after = &input[title + "/manga/".len()..];
            let mut parts = after.split('/');
            let title_id = parts.next()?;
            let chapter_id = after
                .split("/chapter/")
                .nth(1)?
                .split(['?', '#', '/'])
                .next()?;
            Some(format!("{chapter_id}#{title_id}"))
        })
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .or_else(|| request.get(id))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(id))
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_str().map(|text| text == "true"))
        })
        .unwrap_or(false)
}

fn page_image_url(request: &Value) -> Option<String> {
    request
        .get("page")
        .and_then(|page| page.get("content"))
        .and_then(|content| content.get("url"))
        .and_then(|url| url.get("url").or(Some(url)))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn passthrough_image(request: &Value) -> ExtensionResult<ProcessedImage> {
    Ok(ProcessedImage {
        image_base64: request
            .get("imageBase64")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        mime_type: request
            .get("mimeType")
            .and_then(Value::as_str)
            .map(str::to_string),
        ..ProcessedImage::default()
    })
}

fn decode_hex(input: &str) -> Option<Vec<u8>> {
    if !input.len().is_multiple_of(2) {
        return None;
    }
    Some(hex_decode(input))
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

#[derive(Clone, PartialEq, Message)]
struct RankingResponse {
    #[prost(message, repeated, tag = "1")]
    categories: Vec<RankingCategory>,
}

#[derive(Clone, PartialEq, Message)]
struct RankingCategory {
    #[prost(message, repeated, tag = "3")]
    ranking_lists: Vec<RankingList>,
}

#[derive(Clone, PartialEq, Message)]
struct RankingList {
    #[prost(string, tag = "2")]
    kind: String,
    #[prost(message, repeated, tag = "3")]
    titles: Vec<RankingTitle>,
}

#[derive(Clone, PartialEq, Message)]
struct RankingTitle {
    #[prost(message, optional, tag = "1")]
    entry: Option<RankingEntry>,
}

#[derive(Clone, PartialEq, Message)]
struct RankingEntry {
    #[prost(int32, tag = "1")]
    id: i32,
    #[prost(string, tag = "2")]
    name: String,
    #[prost(string, optional, tag = "6")]
    cover: Option<String>,
}

impl RankingEntry {
    fn to_item(self) -> CatalogItem {
        CatalogItem {
            key: self.id.to_string(),
            title: self.name,
            cover: self.cover,
            url: Some(format!("{BASE_URL}/title/{}", self.id)),
            language: Some("ja".into()),
            content_rating: Some("safe".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Clone, PartialEq, Message)]
struct LatestResponse {
    #[prost(message, repeated, tag = "1")]
    list: Vec<ResponseList>,
}

#[derive(Clone, PartialEq, Message)]
struct ResponseList {
    #[prost(message, repeated, tag = "3")]
    response_list: Vec<TitleWrapper>,
}

#[derive(Clone, PartialEq, Message)]
struct TitleWrapper {
    #[prost(message, optional, tag = "1")]
    titles: Option<Titles>,
}

#[derive(Clone, PartialEq, Message)]
struct Titles {
    #[prost(message, optional, tag = "1")]
    entry: Option<Entry>,
}

#[derive(Clone, PartialEq, Message)]
struct Entry {
    #[prost(int32, tag = "1")]
    id: i32,
    #[prost(string, tag = "2")]
    name: String,
    #[prost(string, optional, tag = "6")]
    banner: Option<String>,
    #[prost(string, optional, tag = "16")]
    cover: Option<String>,
}

impl Entry {
    fn to_item(self) -> CatalogItem {
        CatalogItem {
            key: self.id.to_string(),
            title: self.name,
            cover: self.cover.or(self.banner),
            url: Some(format!("{BASE_URL}/title/{}", self.id)),
            language: Some("ja".into()),
            content_rating: Some("safe".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Clone, PartialEq, Message)]
struct DetailResponse {
    #[prost(message, optional, tag = "5")]
    detail_entry: Option<DetailEntry>,
}

#[derive(Clone, PartialEq, Message)]
struct DetailEntry {
    #[prost(message, optional, tag = "1")]
    details: Option<Details>,
}

#[derive(Clone, PartialEq, Message)]
struct Details {
    #[prost(string, tag = "2")]
    name: String,
    #[prost(string, optional, tag = "4")]
    info_text: Option<String>,
    #[prost(string, optional, tag = "5")]
    authors: Option<String>,
    #[prost(message, optional, tag = "22")]
    latest_thumbnail: Option<Thumbnail>,
}

impl Details {
    fn to_item(self, key: &str) -> CatalogItem {
        CatalogItem {
            key: key.into(),
            title: self.name,
            authors: self.authors.into_iter().collect(),
            description: self.info_text.map(|text| html::strip_tags(&text)),
            cover: self.latest_thumbnail.and_then(|thumb| thumb.thumbnail),
            url: Some(format!("{BASE_URL}/title/{key}")),
            language: Some("ja".into()),
            content_rating: Some("safe".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Clone, PartialEq, Message)]
struct Thumbnail {
    #[prost(string, optional, tag = "3")]
    thumbnail: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct ChapterResponse {
    #[prost(message, optional, tag = "1")]
    chapters: Option<Chapters>,
}

#[derive(Clone, PartialEq, Message)]
struct Chapters {
    #[prost(message, repeated, tag = "1")]
    chapter_list: Vec<ChapterItem>,
}

#[derive(Clone, PartialEq, Message)]
struct ChapterItem {
    #[prost(int32, tag = "1")]
    id: i32,
    #[prost(string, tag = "2")]
    title: String,
    #[prost(string, optional, tag = "3")]
    sub_name: Option<String>,
    #[prost(string, optional, tag = "5")]
    date: Option<String>,
    #[prost(message, optional, tag = "16")]
    points: Option<Points>,
}

impl ChapterItem {
    fn to_chapter(self, title_id: &str, hide_locked: bool) -> Option<MangaChapter> {
        let locked = self.points.as_ref().is_some_and(|points| {
            points.shortage.is_some() || points.life.is_some() || points.coin.is_some()
        });
        if hide_locked && locked {
            return None;
        }
        let sub_name = self
            .sub_name
            .filter(|value| !value.is_empty())
            .map(|value| format!(" - {value}"))
            .unwrap_or_default();
        Some(MangaChapter {
            key: format!("{}#{title_id}", self.id),
            title: Some(format!(
                "{}{}{}",
                if locked { "[Locked] " } else { "" },
                self.title,
                sub_name
            )),
            date_uploaded: self
                .date
                .as_deref()
                .and_then(dates::parse_ymd)
                .map(|seconds| seconds * 1000),
            url: Some(format!("{BASE_URL}/manga/{title_id}/chapter/{}", self.id)),
            language: Some("ja".into()),
            is_locked: locked,
            ..MangaChapter::default()
        })
    }
}

#[derive(Clone, PartialEq, Message)]
struct Points {
    #[prost(int32, optional, tag = "1")]
    shortage: Option<i32>,
    #[prost(int32, optional, tag = "2")]
    life: Option<i32>,
    #[prost(int32, optional, tag = "3")]
    coin: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
struct ViewerResponse {
    #[prost(message, repeated, tag = "1")]
    pages: Vec<ViewerPage>,
    #[prost(string, tag = "3")]
    key: String,
    #[prost(string, tag = "4")]
    iv: String,
}

#[derive(Clone, PartialEq, Message)]
struct ViewerPage {
    #[prost(message, optional, tag = "1")]
    page: Option<Image>,
}

#[derive(Clone, PartialEq, Message)]
struct Image {
    #[prost(string, tag = "1")]
    url: String,
}

fn sample_ranking() -> RankingResponse {
    RankingResponse {
        categories: vec![RankingCategory {
            ranking_lists: vec![RankingList {
                kind: "すべて".into(),
                titles: vec![RankingTitle {
                    entry: Some(RankingEntry {
                        id: 1,
                        name: "Sample Manga One".into(),
                        cover: Some("https://img.example.test/mangaone.jpg".into()),
                    }),
                }],
            }],
        }],
    }
}

fn sample_latest() -> LatestResponse {
    LatestResponse {
        list: vec![sample_response_list()],
    }
}

fn sample_response_list() -> ResponseList {
    ResponseList {
        response_list: vec![TitleWrapper {
            titles: Some(Titles {
                entry: Some(Entry {
                    id: 1,
                    name: "Sample Manga One".into(),
                    banner: None,
                    cover: Some("https://img.example.test/mangaone.jpg".into()),
                }),
            }),
        }],
    }
}

fn sample_details() -> Details {
    Details {
        name: "Sample Manga One".into(),
        info_text: Some("Sample description.".into()),
        authors: Some("Sample Author".into()),
        latest_thumbnail: Some(Thumbnail {
            thumbnail: Some("https://img.example.test/mangaone.jpg".into()),
        }),
    }
}

fn sample_chapters() -> ChapterResponse {
    ChapterResponse {
        chapters: Some(Chapters {
            chapter_list: vec![ChapterItem {
                id: 1,
                title: "Chapter 1".into(),
                sub_name: None,
                date: Some("2024/01/01".into()),
                points: None,
            }],
        }),
    }
}

fn sample_viewer() -> ViewerResponse {
    ViewerResponse {
        pages: vec![ViewerPage {
            page: Some(Image {
                url: "https://img.example.test/mangaone-page.jpg".into(),
            }),
        }],
        key: "00000000000000000000000000000000".into(),
        iv: "00000000000000000000000000000000".into(),
    }
}

export_manga_source!(SOURCE);

const RANKING_FIXTURE: &str = "";
const LATEST_FIXTURE: &str = "";
const SEARCH_FIXTURE: &str = "";
const DETAILS_FIXTURE: &str = "";
const CHAPTERS_FIXTURE: &str = "";
const PAGES_FIXTURE: &str = "";
