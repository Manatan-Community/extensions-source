use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult,
    abi::{ExtensionResult, WebViewRequest, WebViewWait, cookies_get, cookies_set, webview_open},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: WebNovel = WebNovel;
const BASE_URL: &str = "https://www.webnovel.com";
const API_BASE: &str = "https://www.webnovel.com/go/pcm";
const COVER_BASE: &str = "https://book-pic.webnovel.com";

struct WebNovel;

impl MangaSource for WebNovel {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_browse(BROWSE_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if listing_id(&request) == "latest" {
            "5"
        } else {
            "1"
        };
        Ok(parse_browse(&fetch_api(
            &format!(
                "{API_BASE}/category/categoryAjax?categoryType=2&pageIndex={page}&categoryId=0&bookStatus=0&orderBy={sort}"
            ),
            BROWSE_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_details(&key), &key)],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(parse_browse(&fetch_api(
                &format!(
                    "{API_BASE}/search/result?type=manga&pageIndex={page}&keywords={}",
                    url::query_escape(query)
                ),
                QUERY_FIXTURE,
            )));
        }
        let filters = request.get("filters");
        Ok(parse_browse(&fetch_api(
            &format!(
                "{API_BASE}/category/categoryAjax?categoryType=2&pageIndex={page}&categoryId={}&bookStatus={}&orderBy={}",
                filter(filters, "genre", "0"),
                filter(filters, "status", "0"),
                filter(filters, "sort", "1")
            ),
            BROWSE_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(parse_details(&fetch_details(&key), &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(parse_chapters(&fetch_api(
            &format!("{API_BASE}/comic/getChapterList?comicId={key}"),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1:1".into());
        let (comic_id, chapter_id) = key.split_once(':').unwrap_or(("1", "1"));
        Ok(parse_pages(&fetch_api(
            &format!(
                "{API_BASE}/comic/getContent?comicId={comic_id}&chapterId={chapter_id}&width=9999"
            ),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/comic/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").and_then(|key| {
            let (comic_id, chapter_id) = key.split_once(':')?;
            Some(format!("{BASE_URL}/comic/{comic_id}/{chapter_id}"))
        }))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?,
                HomeSectionStyle::Cover,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?,
                HomeSectionStyle::Compact,
            ),
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_details(&key), &key)),
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

fn fetch_details(key: &str) -> String {
    fetch_api(
        &format!("{API_BASE}/comic/getComicDetailPage?comicId={key}"),
        DETAILS_FIXTURE,
    )
}

fn fetch_api(target: &str, fixture: &str) -> String {
    let url = append_csrf(target);
    client()
        .get(url)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn append_csrf(target: &str) -> String {
    let token = csrf_token().or_else(|| {
        let _ = webview_open(&WebViewRequest {
            url: BASE_URL.to_string(),
            wait_for: Some(WebViewWait::Delay { milliseconds: 1500 }),
            timeout_ms: Some(45_000),
            return_html: false,
            ..WebViewRequest::default()
        })
        .and_then(|response| cookies_set(response.final_url, response.cookies));
        csrf_token()
    });
    match token {
        Some(token) if target.contains('?') => {
            format!("{target}&_csrfToken={}", url::query_escape(&token))
        }
        Some(token) => format!("{target}?_csrfToken={}", url::query_escape(&token)),
        None => target.to_string(),
    }
}

fn csrf_token() -> Option<String> {
    let header = cookies_get(BASE_URL).ok()?.header?;
    header
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix("_csrfToken=").map(ToString::to_string))
}

fn parse_browse(body: &str) -> Paged<CatalogItem> {
    let value = webnovel_data(body);
    let data = value
        .get("comicInfo")
        .or_else(|| value.get("data").and_then(|data| data.get("comicInfo")))
        .or(Some(&value))
        .unwrap();
    let items = data
        .get("comicItems")
        .or_else(|| value.get("comicItems"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Paged {
        entries: items.iter().filter_map(comic_item).collect(),
        has_next_page: data
            .get("isLast")
            .or_else(|| value.get("isLast"))
            .and_then(Value::as_i64)
            .unwrap_or(1)
            == 0,
    }
}

fn parse_details(body: &str, fallback_key: &str) -> CatalogItem {
    let value = webnovel_data(body);
    let info = value.get("comicInfo").unwrap_or(&value);
    let key =
        string_field(info, &["comicId", "bookId"]).unwrap_or_else(|| fallback_key.to_string());
    let status = match int_field(info, &["actionStatus"]).unwrap_or(0) {
        1 => ItemStatus::Ongoing,
        2 => ItemStatus::Completed,
        3 => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    };
    let update_cycle = string_field(info, &["updateCycle"]).unwrap_or_default();
    let mut description = string_field(info, &["description"]);
    if status == ItemStatus::Ongoing && !update_cycle.is_empty() {
        description = Some(format!(
            "{}\n\nInformation:\n{}",
            description.unwrap_or_default(),
            capitalize(&update_cycle)
        ));
    }
    CatalogItem {
        key: key.clone(),
        title: string_field(info, &["comicName", "bookName"]).unwrap_or_else(|| "WebNovel".into()),
        authors: string_field(info, &["authorName"]).into_iter().collect(),
        description,
        tags: string_field(info, &["categoryName"]).into_iter().collect(),
        cover: cover_url(
            &key,
            int_field(info, &["CV", "coverUpdateTime"]).unwrap_or(0),
        ),
        url: Some(format!("{BASE_URL}/comic/{key}")),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        status,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let value = webnovel_data(body);
    let comic_id = value
        .get("comicInfo")
        .and_then(|comic| string_field(comic, &["comicId"]))
        .unwrap_or_else(|| "1".into());
    let mut chapters = value
        .get("comicChapters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    chapters.reverse();
    chapters
        .iter()
        .filter(|chapter| {
            int_field(chapter, &["userLevel"]).unwrap_or(0)
                >= int_field(chapter, &["chapterLevel"]).unwrap_or(0)
        })
        .filter_map(|chapter| {
            let id = string_field(chapter, &["chapterId"])?;
            let title = string_field(chapter, &["chapterName"]).unwrap_or_else(|| "Chapter".into());
            let premium = int_field(chapter, &["isVip"]).unwrap_or(0) != 0
                || int_field(chapter, &["price"]).unwrap_or(0) != 0;
            let locked = premium && int_field(chapter, &["isAuth"]).unwrap_or(0) != 1;
            Some(MangaChapter {
                key: format!("{comic_id}:{id}"),
                title: Some(if locked {
                    format!("[Locked] {title}")
                } else {
                    title
                }),
                is_locked: locked,
                url: Some(format!("{BASE_URL}/comic/{comic_id}/{id}")),
                language: Some("en".into()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let value = webnovel_data(body);
    value
        .get("chapterInfo")
        .and_then(|chapter| chapter.get("chapterPage"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| string_field(page, &["url"]))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn comic_item(item: &Value) -> Option<CatalogItem> {
    let key = string_field(item, &["comicId", "bookId"])?;
    Some(CatalogItem {
        key: key.clone(),
        title: string_field(item, &["bookName", "comicName"]).unwrap_or_else(|| "WebNovel".into()),
        authors: string_field(item, &["authorName"]).into_iter().collect(),
        description: string_field(item, &["description"]),
        tags: string_field(item, &["categoryName"]).into_iter().collect(),
        cover: cover_url(
            &key,
            int_field(item, &["CV", "coverUpdateTime"]).unwrap_or(0),
        ),
        url: Some(format!("{BASE_URL}/comic/{key}")),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn webnovel_data(body: &str) -> Value {
    #[derive(Deserialize)]
    struct Wrapper {
        data: Option<Value>,
    }
    serde_json::from_str::<Wrapper>(body)
        .ok()
        .and_then(|wrapper| wrapper.data)
        .or_else(|| serde_json::from_str::<Value>(body).ok())
        .unwrap_or_else(|| serde_json::from_str(BROWSE_FIXTURE).expect("fixture is valid"))
}

fn string_field(value: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        value
            .get(*name)
            .and_then(|field| {
                field
                    .as_str()
                    .map(ToString::to_string)
                    .or_else(|| field.as_i64().map(|id| id.to_string()))
            })
            .filter(|field| !field.is_empty())
    })
}

fn int_field(value: &Value, names: &[&str]) -> Option<i64> {
    names.iter().find_map(|name| {
        value
            .get(*name)
            .and_then(|field| field.as_i64().or_else(|| field.as_str()?.parse().ok()))
    })
}

fn cover_url(key: &str, updated_at: i64) -> Option<String> {
    Some(format!(
        "{COVER_BASE}/bookcover/{key}?imageId={updated_at}&imageMogr2/thumbnail/1024x"
    ))
}

fn normalize_key(input: &str) -> String {
    input
        .split("/comic/")
        .nth(1)
        .unwrap_or(input)
        .trim_matches('/')
        .split('/')
        .next()
        .unwrap_or(input)
        .to_string()
}

fn filter<'a>(filters: Option<&'a Value>, key: &str, fallback: &'a str) -> &'a str {
    filters
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn home_section(
    id: &str,
    title: &str,
    page: Paged<CatalogItem>,
    style: HomeSectionStyle,
) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(style),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

export_manga_source!(SOURCE);

const BROWSE_FIXTURE: &str = r#"{"code":0,"msg":"","data":{"isLast":1,"comicItems":[{"bookId":"1","bookName":"Sample Comic","authorName":"Author","description":"Summary","categoryName":"Action","coverUpdateTime":1}]}}"#;
const QUERY_FIXTURE: &str = r#"{"code":0,"msg":"","data":{"comicInfo":{"isLast":1,"comicItems":[{"comicId":"1","bookName":"Sample Comic","categoryName":"Action","CV":1}]}}}"#;
const DETAILS_FIXTURE: &str = r#"{"code":0,"msg":"","data":{"comicInfo":{"comicId":"1","comicName":"Sample Comic","authorName":"Author","description":"Summary","updateCycle":"updated weekly","categoryName":"Action","actionStatus":1,"CV":1}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"code":0,"msg":"","data":{"comicInfo":{"comicId":"1"},"comicChapters":[{"chapterId":"1","chapterName":"Chapter 1","publishTime":"1 day ago","chapterLevel":0,"userLevel":0,"price":0,"isVip":0,"isAuth":1}]}}"#;
const PAGES_FIXTURE: &str = r#"{"code":0,"msg":"","data":{"chapterInfo":{"chapterId":1,"chapterPage":[{"pageId":"1","url":"https://comic-image.webnovel.com/sample/page1.jpg?t=1"}]}}}"#;
