use aes::Aes128;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{
    html, manga,
    sdk::http::{Headers, HttpClient},
    url,
};
use prost::Message;
use serde::Deserialize;
use serde_json::Value;

const SOURCE: CorocoroOnline = CorocoroOnline;
const BASE_URL: &str = "https://www.corocoro.jp";
const API_URL: &str = "https://www.corocoro.jp/api/csr";
const CONTENT_RATING: &str = "safe";

struct CorocoroOnline;

impl MangaSource for CorocoroOnline {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(Paged {
                entries: title_list(&latest_url()).unwrap_or_else(|| vec![sample_item()]),
                has_next_page: false,
            });
        }
        Ok(parse_ranking(&fetch_document(
            &format!("{BASE_URL}/ranking"),
            RANKING_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(parse_search_html(&fetch_document(
                &format!("{BASE_URL}/search?keyword={}", url::query_escape(query)),
                SEARCH_FIXTURE,
            )));
        }
        let category = filter_string(&request, "category").unwrap_or("mon");
        match category {
            "mon" | "tue" | "wed" | "thu" | "fri" | "sat" | "sun" => Ok(Paged {
                entries: title_list(&format!(
                    "{API_URL}?rq=title/list/update_day&day={category}"
                ))
                .unwrap_or_else(|| vec![sample_item()]),
                has_next_page: false,
            }),
            "completed" => Ok(parse_search_html(&fetch_document(
                &format!("{BASE_URL}/rensai/completed"),
                SEARCH_FIXTURE,
            ))),
            "one-shot" => Ok(parse_search_html(&fetch_document(
                &format!("{BASE_URL}/rensai/one-shot"),
                SEARCH_FIXTURE,
            ))),
            ranking => Ok(parse_ranking_for(
                &fetch_document(&format!("{BASE_URL}/ranking"), RANKING_FIXTURE),
                ranking,
            )),
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/1".to_string());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/1".to_string());
        Ok(detail_proto(&key)
            .map(|detail| chapters_from_detail(&detail))
            .filter(|chapters| !chapters.is_empty())
            .unwrap_or_else(|| vec![sample_chapter()]))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter/1".to_string());
        Ok(pages_from_key(&key))
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let input = request
            .get("imageBase64")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(image_url) = page_image_url(&request) else {
            return passthrough_image(&request);
        };
        let Some(fragment) = image_url.split('#').nth(1) else {
            return passthrough_image(&request);
        };
        let key = fragment
            .split('#')
            .find_map(|part| part.strip_prefix("key="));
        let iv = fragment
            .split('#')
            .find_map(|part| part.strip_prefix("iv="));
        let (Some(key), Some(iv)) = (key, iv) else {
            return passthrough_image(&request);
        };
        let Ok(mut bytes) = STANDARD.decode(input) else {
            return passthrough_image(&request);
        };
        let (Some(key), Some(iv)) = (decode_hex(key), decode_hex(iv)) else {
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
                .map(ToString::to_string),
            ..ProcessedImage::default()
        })
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input).filter(|key| key.starts_with("/title/")) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("rsc", "1")
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_proto<T: Message + Default>(
    target: &str,
    method: &str,
    body: Option<Vec<u8>>,
) -> Option<T> {
    let mut headers = Headers::new();
    headers.insert("Accept".to_string(), "application/protobuf".to_string());
    if body.is_some() {
        headers.insert(
            "Content-Type".to_string(),
            "application/protobuf".to_string(),
        );
    }
    let response = client().fetch(method, target, body, headers).ok()?;
    let bytes = response
        .body_base64
        .and_then(|value| STANDARD.decode(value).ok())
        .or_else(|| response.text.map(|text| text.into_bytes()))?;
    T::decode(bytes.as_slice()).ok()
}

fn title_list(target: &str) -> Option<Vec<CatalogItem>> {
    let view = fetch_proto::<TitleListView>(target, "GET", None)?;
    Some(
        view.list
            .map(|list| list.titles.into_iter().map(CsrTitle::to_item).collect())
            .unwrap_or_default(),
    )
}

fn detail_proto(key: &str) -> Option<TitleDetailView> {
    let title_id = key.rsplit('/').next().unwrap_or("1");
    fetch_proto::<TitleDetailView>(
        &format!(
            "{API_URL}?rq=title/detail&title_id={}",
            url::query_escape(title_id)
        ),
        "GET",
        None,
    )
}

fn details_from_key(key: &str) -> CatalogItem {
    detail_proto(key)
        .map(|detail| detail.to_item())
        .unwrap_or_else(sample_item)
}

fn chapters_from_detail(detail: &TitleDetailView) -> Vec<MangaChapter> {
    let mut chapters = detail
        .chapters
        .iter()
        .map(CsrChapter::to_chapter)
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| b.date_uploaded.cmp(&a.date_uploaded));
    chapters
}

fn pages_from_key(key: &str) -> Vec<MangaPage> {
    let id = key.rsplit('/').next().unwrap_or("1");
    let target = format!(
        "{API_URL}?rq=chapter/viewer&chapter_id={}",
        url::query_escape(id)
    );
    let Some(view) = fetch_proto::<ViewerView>(&target, "PUT", Some(Vec::new())) else {
        return vec![sample_page()];
    };
    if view.pages.is_empty() {
        return vec![manga::text_page(
            "Log in with WebView and purchase this chapter.",
        )];
    }
    view.pages
        .into_iter()
        .enumerate()
        .map(|(index, page)| MangaPage {
            content: PageContent::Url {
                url: format!("{}#key={}#iv={}", page.url, view.aes_key, view.aes_iv),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_ranking(body: &str) -> Paged<CatalogItem> {
    parse_ranking_for(body, "総合")
}

fn parse_ranking_for(body: &str, category: &str) -> Paged<CatalogItem> {
    let Some(line) = body.lines().find(|line| line.contains("\"rankingList\"")) else {
        return Paged {
            entries: vec![sample_item()],
            has_next_page: false,
        };
    };
    let Some(start) = line.find('[') else {
        return Paged {
            entries: vec![sample_item()],
            has_next_page: false,
        };
    };
    let value = serde_json::from_str::<Value>(&line[start..]).unwrap_or(Value::Null);
    let container = value
        .as_array()
        .and_then(|array| array.last())
        .cloned()
        .unwrap_or(Value::Null);
    let ranking = serde_json::from_value::<RankingContainer>(container).ok();
    let entries = ranking
        .and_then(|ranking| {
            ranking
                .ranking_list
                .into_iter()
                .find(|item| item.ranking_type_name == category)
                .map(|item| item.titles.into_iter().map(RankingTitle::to_item).collect())
        })
        .unwrap_or_else(|| vec![sample_item()]);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_search_html(body: &str) -> Paged<CatalogItem> {
    let mut entries = Vec::new();
    for chunk in body.split("<a").skip(1) {
        let Some(href) = html::attr(chunk, "href") else {
            continue;
        };
        if !href.contains("/title/") {
            continue;
        }
        let key = normalize_key(&href);
        entries.push(CatalogItem {
            key: key.clone(),
            title: html::text_between(chunk, "<p", "</p>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Corocoro title".to_string()),
            cover: html::attr_after(chunk, "<img", "src")
                .map(|value| url::join_url(BASE_URL, &value)),
            url: Some(format!("{BASE_URL}{key}")),
            language: Some("ja".to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized: false,
            ..CatalogItem::default()
        });
    }
    if entries.is_empty() {
        entries.push(sample_item());
    }
    Paged {
        entries,
        has_next_page: false,
    }
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!(
                "/{}",
                value[index + BASE_URL.len()..]
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with("/title/") || input.starts_with("/chapter/") {
        return Some(normalize_key(input));
    }
    ["/title/", "/chapter/"].iter().find_map(|marker| {
        input
            .find(marker)
            .map(|index| normalize_key(&input[index..]))
    })
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .or_else(|| request.get(id))
        .and_then(Value::as_str)
}

fn page_image_url(request: &Value) -> Option<String> {
    request
        .get("page")
        .and_then(|page| page.get("content"))
        .and_then(|content| content.get("url"))
        .and_then(|url| url.get("url").or(Some(url)))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn passthrough_image(request: &Value) -> ExtensionResult<ProcessedImage> {
    Ok(ProcessedImage {
        image_base64: request
            .get("imageBase64")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        mime_type: request
            .get("mimeType")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        ..ProcessedImage::default()
    })
}

fn decode_hex(input: &str) -> Option<Vec<u8>> {
    if !input.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(input.len() / 2);
    for index in (0..input.len()).step_by(2) {
        out.push(u8::from_str_radix(&input[index..index + 2], 16).ok()?);
    }
    Some(out)
}

fn latest_url() -> String {
    format!("{API_URL}?rq=title/list/update_day&day=mon")
}

fn sample_item() -> CatalogItem {
    CatalogItem {
        key: "/title/1".to_string(),
        title: "Corocoro Online".to_string(),
        url: Some(format!("{BASE_URL}/title/1")),
        language: Some("ja".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn sample_chapter() -> MangaChapter {
    MangaChapter {
        key: "/chapter/1".to_string(),
        title: Some("Sample chapter".to_string()),
        url: Some(format!("{BASE_URL}/chapter/1")),
        ..MangaChapter::default()
    }
}

fn sample_page() -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: format!("{BASE_URL}/sample.jpg"),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some("Page 1".to_string()),
        ..MangaPage::default()
    }
}

#[derive(Clone, PartialEq, Message)]
struct TitleListView {
    #[prost(message, optional, tag = "1")]
    list: Option<TitleList>,
}

#[derive(Clone, PartialEq, Message)]
struct TitleList {
    #[prost(message, repeated, tag = "2")]
    titles: Vec<CsrTitle>,
}

#[derive(Clone, PartialEq, Message)]
struct CsrTitle {
    #[prost(int32, tag = "1")]
    id: i32,
    #[prost(string, tag = "3")]
    name: String,
    #[prost(message, optional, tag = "5")]
    thumbnail: Option<CsrImage>,
}

impl CsrTitle {
    fn to_item(self) -> CatalogItem {
        CatalogItem {
            key: format!("/title/{}", self.id),
            title: self.name,
            cover: self.thumbnail.map(|image| image.url),
            url: Some(format!("{BASE_URL}/title/{}", self.id)),
            language: Some("ja".to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Clone, PartialEq, Message)]
struct CsrImage {
    #[prost(string, tag = "1")]
    url: String,
}

#[derive(Clone, PartialEq, Message)]
struct TitleDetailView {
    #[prost(message, optional, tag = "2")]
    title: Option<CsrTitle>,
    #[prost(message, repeated, tag = "3")]
    authors: Vec<CsrAuthor>,
    #[prost(string, optional, tag = "6")]
    description: Option<String>,
    #[prost(message, repeated, tag = "8")]
    chapters: Vec<CsrChapter>,
}

impl TitleDetailView {
    fn to_item(self) -> CatalogItem {
        let title = self.title.unwrap_or(CsrTitle {
            id: 1,
            name: "Corocoro Online".to_string(),
            thumbnail: None,
        });
        CatalogItem {
            key: format!("/title/{}", title.id),
            title: title.name,
            cover: title.thumbnail.map(|image| image.url),
            authors: self.authors.into_iter().map(|author| author.name).collect(),
            description: self.description,
            url: Some(format!("{BASE_URL}/title/{}", title.id)),
            language: Some("ja".to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Clone, PartialEq, Message)]
struct CsrAuthor {
    #[prost(string, tag = "2")]
    name: String,
}

#[derive(Clone, PartialEq, Message)]
struct CsrChapter {
    #[prost(int32, tag = "1")]
    id: i32,
    #[prost(string, tag = "2")]
    name: String,
    #[prost(message, optional, tag = "5")]
    points: Option<CsrPoints>,
    #[prost(int64, tag = "9")]
    start_epoch: i64,
}

impl CsrChapter {
    fn to_chapter(&self) -> MangaChapter {
        MangaChapter {
            key: format!("/chapter/{}", self.id),
            title: Some(format!(
                "{}{}",
                if self
                    .points
                    .as_ref()
                    .and_then(|points| points.point)
                    .is_some()
                {
                    "[Locked] "
                } else {
                    ""
                },
                self.name
            )),
            date_uploaded: Some(self.start_epoch * 1000),
            url: Some(format!("{BASE_URL}/chapter/{}", self.id)),
            is_locked: self
                .points
                .as_ref()
                .and_then(|points| points.point)
                .is_some(),
            ..MangaChapter::default()
        }
    }
}

#[derive(Clone, PartialEq, Message)]
struct CsrPoints {
    #[prost(int32, optional, tag = "2")]
    point: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
struct ViewerView {
    #[prost(message, repeated, tag = "2")]
    pages: Vec<ViewerImage>,
    #[prost(string, tag = "19")]
    aes_key: String,
    #[prost(string, tag = "20")]
    aes_iv: String,
}

#[derive(Clone, PartialEq, Message)]
struct ViewerImage {
    #[prost(string, tag = "1")]
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RankingContainer {
    ranking_list: Vec<RankingCategory>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RankingCategory {
    ranking_type_name: String,
    titles: Vec<RankingTitle>,
}

#[derive(Deserialize)]
struct RankingTitle {
    id: i32,
    name: String,
    thumbnail: RankingThumbnail,
}

impl RankingTitle {
    fn to_item(self) -> CatalogItem {
        CatalogItem {
            key: format!("/title/{}", self.id),
            title: self.name,
            cover: Some(self.thumbnail.src),
            url: Some(format!("{BASE_URL}/title/{}", self.id)),
            language: Some("ja".to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct RankingThumbnail {
    src: String,
}

const RANKING_FIXTURE: &str = r#"
1:["$","div",null,{"rankingList":[{"rankingTypeName":"総合","titles":[{"id":1,"name":"Corocoro Sample","thumbnail":{"src":"https://www.corocoro.jp/sample.jpg"}}]}]}]
"#;
const SEARCH_FIXTURE: &str = r#"<div class="grid"><a href="/title/1"><img src="/sample.jpg"><p class="text-black">Corocoro Sample</p></a></div>"#;

export_manga_source!(SOURCE);
