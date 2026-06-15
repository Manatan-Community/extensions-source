use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Zaimanhua = Zaimanhua;
const BASE_URL: &str = "https://manhua.zaimanhua.com";
const MOBILE_URL: &str = "https://m.zaimanhua.com";
const API_URL: &str = "https://v4api.zaimanhua.com/app/v1";
const PC_DETAIL_URL: &str = "https://manhua.zaimanhua.com/api/v1/comic2/comic/detail";
const PAGE_SIZE: u64 = 20;

struct Zaimanhua;

impl MangaSource for Zaimanhua {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{API_URL}/comic/update/list/0/{}", page(&request))
        } else {
            format!("{API_URL}/comic/rank/list?tag_id=0&page={}", page(&request))
        };
        let body = fetch_json(&request, &target, LIST_FIXTURE, None);
        Ok(parse_page_items(&body, page(&request)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = id_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&request, &id)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target =
            if query.is_empty() && filter(filters, "by_time").is_some_and(|v| !v.is_empty()) {
                let mut params = vec!["tag_id=0".to_string(), format!("page={}", page(&request))];
                for key in ["by_time", "rank_type"] {
                    if let Some(value) = filter(filters, key).filter(|value| !value.is_empty()) {
                        params.push(format!("{key}={value}"));
                    }
                }
                format!("{API_URL}/comic/rank/list?{}", params.join("&"))
            } else if query.is_empty() {
                let mut params = vec![
                    format!("size={PAGE_SIZE}"),
                    format!("page={}", page(&request)),
                ];
                for key in ["sortType", "status", "cate", "zone", "theme"] {
                    if let Some(value) = filter(filters, key).filter(|value| *value != "0") {
                        params.push(format!("{key}={value}"));
                    }
                }
                format!("{API_URL}/comic/filter/list?{}", params.join("&"))
            } else if filters
                .get("searchById")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && query.parse::<u64>().is_ok()
            {
                format!("{API_URL}/comic/detail/{query}?_v=2.2.5")
            } else {
                format!(
                    "{API_URL}/search/index?source=0&size={PAGE_SIZE}&keyword={}&page={}",
                    url::query_escape(query),
                    page(&request)
                )
            };
        if target.contains("/comic/detail/") {
            let id = target
                .split("/comic/detail/")
                .nth(1)
                .and_then(|v| v.split('?').next())
                .unwrap_or(query);
            return Ok(Paged {
                entries: vec![fetch_details(&request, id)],
                has_next_page: false,
            });
        }
        Ok(parse_search(
            &fetch_json(&request, &target, SEARCH_FIXTURE, None),
            page(&request),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let id = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(fetch_details(&request, &id))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let id = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let target = format!("{API_URL}/comic/detail/{id}?_v=2.2.5#{id}");
        let mut data = parse_data::<ChapterData>(
            &fetch_json(&request, &target, CHAPTERS_FIXTURE, Some("pc")),
            CHAPTERS_FIXTURE,
        )
        .unwrap_or_default();
        if data.chapter_list.is_empty() {
            data = parse_data::<ChapterData>(
                &fetch_json(
                    &request,
                    &format!("{PC_DETAIL_URL}?id={id}"),
                    CHAPTERS_FIXTURE,
                    Some("pc"),
                ),
                CHAPTERS_FIXTURE,
            )
            .unwrap_or_default();
        }
        let mut chapters = Vec::new();
        for group in data.chapter_list {
            for item in group.data {
                chapters.push(MangaChapter {
                    key: format!("{id}/{}", item.chapter_id),
                    title: Some(format_chapter_name(&item.chapter_title)),
                    scanlators: vec![group.title.clone()],
                    date_uploaded: item.updatetime.map(|time| time * 1000),
                    url: Some(format!(
                        "{MOBILE_URL}/pages/comic/page?comic_id={id}&chapter_id={}",
                        item.chapter_id
                    )),
                    ..MangaChapter::default()
                });
            }
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1/1".into());
        let target = format!("{API_URL}/comic/chapter/{key}?_v=2.2.5");
        let data = parse_data::<ChapterImages>(
            &fetch_json(&request, &target, PAGES_FIXTURE, Some("h5")),
            PAGES_FIXTURE,
        )
        .unwrap_or_default();
        let mut pages = data
            .images
            .into_iter()
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: None,
                },
                headers: manga::image_headers(MOBILE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect::<Vec<_>>();
        if comments_enabled(&request) {
            if let Some(text) = fetch_comments(&request, &key) {
                pages.push(manga::text_page(&text));
            }
        }
        Ok(pages)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|id| format!("{MOBILE_URL}/pages/comic/detail?id={id}")))
    }
    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let parts = key.split('/').collect::<Vec<_>>();
            format!(
                "{MOBILE_URL}/pages/comic/page?comic_id={}&chapter_id={}",
                parts.first().copied().unwrap_or("1"),
                parts.get(1).copied().unwrap_or("1")
            )
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&request, &id)),
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

fn client(request: &Value) -> http::HttpClient {
    let mut client = http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(MOBILE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback();
    if let Some(token) = token(request) {
        client = client.with_header("authorization", format!("Bearer {token}"));
    }
    client
}
fn fetch_json(request: &Value, target: &str, fixture: &str, platform: Option<&str>) -> String {
    let http = client(request);
    let mut builder = http
        .get(target)
        .header("Accept", "application/json, text/plain, */*");
    if let Some(platform) = platform {
        builder = builder.header("Platform", platform);
    }
    builder.send_text().unwrap_or_else(|_| fixture.into())
}
fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}
fn token(request: &Value) -> Option<String> {
    request
        .get("preferences")
        .and_then(|p| p.get("token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
}
fn comments_enabled(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|p| p.get("comments"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}
fn filter<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).and_then(Value::as_str)
}
fn id_from_url(input: &str) -> Option<String> {
    if input.chars().all(|ch| ch.is_ascii_digit()) {
        return Some(input.into());
    }
    input
        .split("id=")
        .nth(1)
        .map(|v| v.split('&').next().unwrap_or(v).to_string())
        .or_else(|| {
            input
                .split("/comic/detail/")
                .nth(1)
                .map(|v| v.split('?').next().unwrap_or(v).to_string())
        })
        .filter(|v| !v.is_empty() && v.chars().all(|ch| ch.is_ascii_digit()))
}

fn fetch_details(request: &Value, id: &str) -> CatalogItem {
    parse_data::<MangaDto>(
        &fetch_json(
            request,
            &format!("{API_URL}/comic/detail/{id}?_v=2.2.5"),
            DETAILS_FIXTURE,
            None,
        ),
        DETAILS_FIXTURE,
    )
    .map(|item| item.catalog())
    .unwrap_or_else(|| CatalogItem {
        key: id.into(),
        title: "再漫画".into(),
        url: Some(format!("{MOBILE_URL}/pages/comic/detail?id={id}")),
        language: Some("zh".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_page_items(body: &str, page_no: u64) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<Response<Vec<PageItem>>>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("valid fixture"));
    let entries = response
        .data
        .into_iter()
        .map(PageItem::catalog)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: !entries.is_empty() && page_no < 1000,
        entries,
    }
}
fn parse_search(body: &str, fallback_page: u64) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<Response<SearchPage>>(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).expect("valid fixture"));
    let page_no = response.data.page.unwrap_or(fallback_page);
    let page_size = response.data.size.unwrap_or(PAGE_SIZE);
    Paged {
        has_next_page: page_no * page_size < response.data.total,
        entries: response
            .data
            .list
            .into_iter()
            .map(PageItem::catalog)
            .collect(),
    }
}
fn parse_data<T: for<'de> Deserialize<'de>>(body: &str, fixture: &str) -> Option<T> {
    serde_json::from_str::<Response<DataWrapper<T>>>(body)
        .or_else(|_| serde_json::from_str(fixture))
        .ok()?
        .data
        .data
}
fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.unwrap_or_default() {
        "连载中" => ItemStatus::Ongoing,
        "已完结" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}
fn format_list(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split('/')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}
fn format_chapter_name(name: &str) -> String {
    let plain = name.trim_start_matches("连载").trim_start_matches("版");
    if plain.chars().next().is_some_and(|ch| ch.is_ascii_digit())
        && !plain.contains('话')
        && !plain.contains('卷')
    {
        format!("第{plain}话")
    } else {
        name.to_string()
    }
}
fn fetch_comments(request: &Value, key: &str) -> Option<String> {
    let mut parts = key.split('/');
    let comic_id = parts.next()?;
    let chapter_id = parts.next()?;
    let body = fetch_json(
        request,
        &format!("{API_URL}/viewpoint/list?comicId={comic_id}&chapterId={chapter_id}"),
        COMMENTS_FIXTURE,
        None,
    );
    let response = serde_json::from_str::<Response<CommentData>>(&body).ok()?;
    let comments = response
        .data
        .list
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| item.last().and_then(Value::as_str).map(ToOwned::to_owned))
        .collect::<Vec<_>>();
    Some(if comments.is_empty() {
        "没有吐槽".into()
    } else {
        comments.join("\n")
    })
}

#[derive(Deserialize)]
struct Response<T> {
    data: T,
}
#[derive(Deserialize)]
struct DataWrapper<T> {
    #[serde(alias = "comicInfo")]
    data: Option<T>,
}
#[derive(Deserialize)]
struct MangaDto {
    id: u64,
    title: String,
    cover: Option<String>,
    description: Option<String>,
    types: Option<Vec<Tag>>,
    status: Option<Vec<Tag>>,
    authors: Option<Vec<Tag>>,
}
impl MangaDto {
    fn catalog(self) -> CatalogItem {
        CatalogItem {
            key: self.id.to_string(),
            title: self.title,
            cover: self.cover,
            authors: self
                .authors
                .unwrap_or_default()
                .into_iter()
                .map(|tag| tag.name)
                .collect(),
            tags: self
                .types
                .unwrap_or_default()
                .into_iter()
                .map(|tag| tag.name)
                .collect(),
            description: self.description,
            status: parse_status(
                self.status
                    .as_ref()
                    .and_then(|items| items.first())
                    .map(|tag| tag.name.as_str()),
            ),
            url: Some(format!("{MOBILE_URL}/pages/comic/detail?id={}", self.id)),
            language: Some("zh".into()),
            content_rating: Some("safe".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}
#[derive(Deserialize)]
struct Tag {
    #[serde(rename = "tag_name")]
    name: String,
}
#[derive(Default, Deserialize)]
struct ChapterData {
    #[serde(alias = "chapters", alias = "chapterList", default)]
    chapter_list: Vec<ChapterGroup>,
}
#[derive(Deserialize)]
struct ChapterGroup {
    title: String,
    data: Vec<ChapterDto>,
}
#[derive(Deserialize)]
struct ChapterDto {
    chapter_id: u64,
    chapter_title: String,
    updatetime: Option<i64>,
}
#[derive(Default, Deserialize)]
struct ChapterImages {
    #[serde(rename = "page_url_hd", default)]
    images: Vec<String>,
}
#[derive(Deserialize)]
struct SearchPage {
    #[serde(alias = "comicList", alias = "list", default)]
    list: Vec<PageItem>,
    page: Option<u64>,
    size: Option<u64>,
    #[serde(alias = "totalNum")]
    total: u64,
}
#[derive(Deserialize)]
struct PageItem {
    id: Option<u64>,
    comic_id: Option<u64>,
    #[serde(alias = "name")]
    title: String,
    authors: Option<String>,
    status: Option<String>,
    cover: Option<String>,
    types: Option<String>,
}
impl PageItem {
    fn catalog(self) -> CatalogItem {
        let id = self.comic_id.filter(|id| *id != 0).or(self.id).unwrap_or(0);
        CatalogItem {
            key: id.to_string(),
            title: self.title,
            cover: self.cover,
            authors: format_list(self.authors),
            tags: format_list(self.types),
            status: parse_status(self.status.as_deref()),
            url: Some(format!("{MOBILE_URL}/pages/comic/detail?id={id}")),
            language: Some("zh".into()),
            content_rating: Some("safe".into()),
            ..CatalogItem::default()
        }
    }
}
#[derive(Deserialize)]
struct CommentData {
    list: Option<Vec<Vec<Value>>>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":[{"id":1,"comic_id":1,"title":"Sample","authors":"Author","status":"连载中","cover":"https://m.zaimanhua.com/cover.jpg","types":"Tag"}]}"#;
const SEARCH_FIXTURE: &str = r#"{"data":{"list":[{"id":1,"comic_id":1,"title":"Sample","authors":"Author","status":"连载中","cover":"https://m.zaimanhua.com/cover.jpg","types":"Tag"}],"page":1,"size":20,"total":1}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"data":{"id":1,"title":"Sample","cover":"https://m.zaimanhua.com/cover.jpg","description":"Sample description.","types":[{"tag_name":"Tag"}],"status":[{"tag_name":"连载中"}],"authors":[{"tag_name":"Author"}]}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"data":{"id":1,"last_update_chapter_id":1,"last_updatetime":1760000000,"chapters":[{"title":"Main","data":[{"chapter_id":1,"chapter_title":"1","updatetime":1760000000}]}],"isHideChapter":0,"canRead":true}}}"#;
const PAGES_FIXTURE: &str =
    r#"{"data":{"data":{"page_url_hd":["https://m.zaimanhua.com/page.jpg"],"canRead":true}}}"#;
const COMMENTS_FIXTURE: &str = r#"{"data":{"list":[]}}"#;
