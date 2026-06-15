use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{
    manga,
    sdk::http::{Headers, HttpClient},
    url,
};
use prost::Message;
use serde_json::Value;

const SOURCE: ZerosumOnline = ZerosumOnline;
const BASE_URL: &str = "https://zerosumonline.com";
const API_URL: &str = "https://api.zerosumonline.com/api/v1";

struct ZerosumOnline;

impl MangaSource for ZerosumOnline {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: sample_title_list()
                    .titles
                    .into_iter()
                    .map(title_to_item)
                    .collect(),
                has_next_page: false,
            });
        }
        let response = fetch_proto::<TitleListView>(
            &format!("{API_URL}/list?category=series&sort=date"),
            "GET",
            None,
        )
        .unwrap_or_else(sample_title_list);
        Ok(Paged {
            entries: response.titles.into_iter().map(title_to_item).collect(),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            let sort = filter_string(&request, "sort").unwrap_or_else(|| "date".into());
            format!(
                "{API_URL}/list?category=series&sort={}",
                url::query_escape(&sort)
            )
        } else {
            format!("{API_URL}/search?keyword={}", url::query_escape(query))
        };
        let response =
            fetch_proto::<TitleListView>(&target, "GET", None).unwrap_or_else(sample_title_list);
        Ok(Paged {
            entries: response.titles.into_iter().map(title_to_item).collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let response = fetch_detail(&slug_from_key(&key));
        Ok(response
            .chapters
            .into_iter()
            .map(|chapter| {
                chapter_to_item(
                    chapter,
                    &response
                        .title
                        .as_ref()
                        .map(|title| title.slug.as_str())
                        .unwrap_or(&key),
                )
            })
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".into());
        let chapter_id = key.trim_matches('/').split('/').next_back().unwrap_or("1");
        let body = ViewerRequest {
            chapter_id: chapter_id.parse::<i32>().unwrap_or(1),
        }
        .encode_to_vec();
        let response = fetch_proto::<ViewerView>(
            &format!(
                "{API_URL}/viewer?chapter_id={}",
                url::query_escape(chapter_id)
            ),
            "POST",
            Some(body),
        )
        .unwrap_or_else(sample_viewer);
        Ok(response
            .pages
            .into_iter()
            .filter(|page| !page.url.is_empty())
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: image.url,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".into(),
            title: "Popular".into(),
            style: Some(HomeSectionStyle::Cover),
            has_more: popular.has_next_page,
            entries: popular.entries,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/detail/{}", slug_from_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let slug = key.trim_matches('/').split('/').next().unwrap_or("sample");
            format!("{BASE_URL}/detail/{slug}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
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
}

fn fetch_detail(slug: &str) -> TitleDetailView {
    fetch_proto::<TitleDetailView>(
        &format!("{API_URL}/title?tag={}", url::query_escape(slug)),
        "GET",
        None,
    )
    .unwrap_or_else(sample_detail)
}

fn details_by_key(key: &str) -> CatalogItem {
    let detail = fetch_detail(&slug_from_key(key));
    detail
        .title
        .map(title_to_initialized_item)
        .unwrap_or_else(|| title_to_initialized_item(sample_title()))
}

fn fetch_proto<T: Message + Default>(
    target: &str,
    method: &str,
    body: Option<Vec<u8>>,
) -> Option<T> {
    let mut headers = Headers::new();
    headers.insert("Accept".into(), "application/protobuf".into());
    if body.is_some() {
        headers.insert("Content-Type".into(), "application/protobuf".into());
    }
    client()
        .fetch(method, target, body, headers)
        .ok()
        .and_then(|response| {
            response
                .body_base64
                .and_then(|body| STANDARD.decode(body).ok())
                .or_else(|| response.text.map(|text| text.into_bytes()))
        })
        .and_then(|bytes| T::decode(bytes.as_slice()).ok())
}

fn title_to_item(title: ApiTitle) -> CatalogItem {
    let key = title.slug.clone();
    CatalogItem {
        key: key.clone(),
        title: title.name,
        cover: empty_to_none(title.thumbnail),
        authors: empty_to_none(title.authors).into_iter().collect(),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        url: Some(format!("{BASE_URL}/detail/{key}")),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn title_to_initialized_item(title: ApiTitle) -> CatalogItem {
    let mut item = title_to_item(title.clone());
    let mut description = title.description;
    if !title.alt_title.is_empty() {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str(&title.alt_title);
    }
    item.description = empty_to_none(description);
    item.initialized = true;
    item
}

fn chapter_to_item(chapter: ApiChapter, slug: &str) -> MangaChapter {
    let key = format!("{slug}/{}", chapter.id);
    MangaChapter {
        key: key.clone(),
        title: empty_to_none(chapter.name),
        date_uploaded: Some(normalize_timestamp(chapter.published_at)),
        url: Some(format!("{BASE_URL}/detail/{slug}")),
        ..MangaChapter::default()
    }
}

fn normalize_timestamp(value: i64) -> i64 {
    if value > 10_000_000_000 {
        value / 1000
    } else {
        value
    }
}

fn empty_to_none(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .and_then(|path| path.trim_start_matches('/').strip_prefix("detail/"))
        .map(|slug| slug.trim_matches('/').to_string())
}

fn slug_from_key(key: &str) -> String {
    key.trim_matches('/')
        .split('/')
        .next()
        .unwrap_or("sample")
        .to_string()
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

#[derive(Clone, PartialEq, Message)]
struct TitleListView {
    #[prost(message, repeated, tag = "3")]
    titles: Vec<ApiTitle>,
}

#[derive(Clone, PartialEq, Message)]
struct TitleDetailView {
    #[prost(message, optional, tag = "2")]
    title: Option<ApiTitle>,
    #[prost(message, repeated, tag = "3")]
    chapters: Vec<ApiChapter>,
}

#[derive(Clone, PartialEq, Message)]
struct ApiTitle {
    #[prost(string, tag = "2")]
    slug: String,
    #[prost(string, tag = "3")]
    name: String,
    #[prost(string, tag = "4")]
    alt_title: String,
    #[prost(string, tag = "5")]
    authors: String,
    #[prost(string, tag = "7")]
    description: String,
    #[prost(string, tag = "8")]
    thumbnail: String,
}

#[derive(Clone, PartialEq, Message)]
struct ApiChapter {
    #[prost(int32, tag = "1")]
    id: i32,
    #[prost(string, tag = "2")]
    name: String,
    #[prost(int64, tag = "4")]
    published_at: i64,
}

#[derive(Clone, PartialEq, Message)]
struct ViewerView {
    #[prost(message, repeated, tag = "5")]
    pages: Vec<ViewerImage>,
}

#[derive(Clone, PartialEq, Message)]
struct ViewerImage {
    #[prost(string, tag = "1")]
    url: String,
}

#[derive(Clone, PartialEq, Message)]
struct ViewerRequest {
    #[prost(int32, tag = "1")]
    chapter_id: i32,
}

fn sample_title() -> ApiTitle {
    ApiTitle {
        slug: "sample".into(),
        name: "Sample Zerosum".into(),
        alt_title: "Alt Sample".into(),
        authors: "Author".into(),
        description: "Summary".into(),
        thumbnail: "https://img.example/cover.jpg".into(),
    }
}

fn sample_title_list() -> TitleListView {
    TitleListView {
        titles: vec![sample_title()],
    }
}

fn sample_detail() -> TitleDetailView {
    TitleDetailView {
        title: Some(sample_title()),
        chapters: vec![ApiChapter {
            id: 1,
            name: "Chapter 1".into(),
            published_at: 1_704_067_200,
        }],
    }
}

fn sample_viewer() -> ViewerView {
    ViewerView {
        pages: vec![ViewerImage {
            url: "https://img.example/001.jpg".into(),
        }],
    }
}

export_manga_source!(SOURCE);
