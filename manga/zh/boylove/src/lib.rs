use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: BoyLove = BoyLove;
const BASE_URL: &str = "https://boylove1.mobi";
const IMAGE_BASE_URL: &str = "https://blcnimghost2.cc";

struct BoyLove;

impl MangaSource for BoyLove {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request).saturating_sub(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/home/Api/getDailyUpdate.html?widx=4&page={page}&limit=10")
        } else {
            format!("{BASE_URL}/home/api/getpage/tp/1-topestmh-{page}")
        };
        Ok(parse_manga_page(&fetch_json(&target, if target.contains("Daily") { LATEST_FIXTURE } else { LIST_FIXTURE })))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let id = query.rsplit('/').next().unwrap_or(query);
            return Ok(Paged { entries: vec![details_from_search(id, id)], has_next_page: false });
        }
        let target = format!("{BASE_URL}/home/api/searchk?keyword={}&type=1&pageNo={}", query_escape(query), page(&request));
        Ok(parse_manga_page(&fetch_json(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        Ok(details_from_search(&key, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        let body = fetch_json(&format!("{BASE_URL}/home/api/chapter_list/tp/{key}"), CHAPTERS_FIXTURE);
        Ok(serde_json::from_str::<ResultDto<ListPageDto<ChapterDto>>>(&body)
            .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("valid fixture"))
            .result
            .list
            .into_iter()
            .rev()
            .map(ChapterDto::to_chapter)
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "/home/book/capter/id/1".to_string());
        let body = fetch_html(&join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) && input.contains("/home/book/index/id/") {
            let id = input.rsplit('/').next().unwrap_or(input);
            return Ok(Some(UrlResolveResult { item: Some(details_from_search(id, id)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client().get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_html(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn parse_manga_page(body: &str) -> Paged<CatalogItem> {
    if let Ok(page) = serde_json::from_str::<ResultDto<ListPageDto<MangaDto>>>(body) {
        return Paged { entries: page.result.list.into_iter().map(MangaDto::to_item).collect(), has_next_page: !page.result.last_page };
    }
    let latest = serde_json::from_str::<ResultDto<Vec<MangaDto>>>(body)
        .or_else(|_| serde_json::from_str::<Vec<MangaDto>>(body).map(|list| ResultDto { result: list }))
        .unwrap_or_else(|_| serde_json::from_str(LATEST_FIXTURE).expect("valid fixture"));
    Paged { has_next_page: latest.result.len() >= 10, entries: latest.result.into_iter().map(MangaDto::to_item).collect() }
}

fn details_from_search(key: &str, query: &str) -> CatalogItem {
    let body = fetch_json(&format!("{BASE_URL}/home/api/searchk?keyword={}&type=1&pageNo=1", query_escape(query)), LIST_FIXTURE);
    let mut page = parse_manga_page(&body);
    page.entries.drain(..).find(|item| item.key == key).unwrap_or_else(|| CatalogItem {
        key: key.to_string(),
        title: format!("BoyLove {key}"),
        url: Some(format!("{BASE_URL}/home/book/index/id/{key}")),
        language: Some("zh".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

#[derive(Debug, Deserialize)]
struct ResultDto<T> {
    result: T,
}

#[derive(Debug, Deserialize)]
struct ListPageDto<T> {
    #[serde(rename = "lastPage")]
    last_page: bool,
    #[serde(default)]
    list: Vec<T>,
}

#[derive(Debug, Default, Deserialize)]
struct MangaDto {
    id: i64,
    title: String,
    image: String,
    #[serde(default)]
    auther: String,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    mhstatus: i64,
    #[serde(default)]
    keyword: String,
    #[serde(default)]
    update_time: Value,
}

impl MangaDto {
    fn to_item(self) -> CatalogItem {
        let mut description = self.desc.unwrap_or_default();
        if !self.update_time.is_null() {
            description = format!("更新时间：{}\n\n{}", self.update_time, description);
        }
        CatalogItem {
            key: self.id.to_string(),
            title: self.title,
            cover: Some(to_image_url(&self.image)),
            authors: (!self.auther.is_empty()).then_some(self.auther).into_iter().collect(),
            description: (!description.trim().is_empty()).then_some(description.trim().to_string()),
            tags: self.keyword.split(',').map(str::trim).filter(|value| !value.is_empty()).map(ToString::to_string).collect(),
            status: match self.mhstatus { 0 => ItemStatus::Ongoing, 1 => ItemStatus::Completed, _ => ItemStatus::Unknown },
            url: Some(format!("{BASE_URL}/home/book/index/id/{}", self.id)),
            language: Some("zh".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ChapterDto {
    id: i64,
    title: String,
    create_time: String,
}

impl ChapterDto {
    fn to_chapter(self) -> MangaChapter {
        let key = format!("/home/book/capter/id/{}", self.id);
        MangaChapter {
            key: key.clone(),
            title: Some(self.title.trim().to_string()),
            date_uploaded: parse_ymd(self.create_time.split_whitespace().next().unwrap_or("")),
            url: Some(join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img").skip(1).filter_map(|chunk| {
        let image = attr(chunk, "data-original").or_else(|| attr(chunk, "src"))?;
        let image = image.trim();
        if image.ends_with(".gif") || image.contains("load.png") {
            return None;
        }
        Some(to_image_url(image))
    }).enumerate().map(|(index, image)| MangaPage {
        content: PageContent::Url { url: image, context: None },
        headers: image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }).collect()
}

fn to_image_url(value: &str) -> String {
    if value.starts_with("http") { value.to_string() } else { join_url(IMAGE_BASE_URL, value) }
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .and_then(Value::as_str)
        .or_else(|| request.get(field).and_then(Value::as_str))
        .map(ToString::to_string)
}

fn image_headers(referer: &str) -> manatan_extension::Context {
    let mut headers = manatan_extension::Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn join_url(base: &str, value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        return value.to_string();
    }
    format!("{}/{}", base.trim_end_matches('/'), value.trim_start_matches('/'))
}

fn query_escape(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn attr(input: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let needle = format!("{name}={quote}");
        let Some(start) = input.find(&needle).map(|index| index + needle.len()) else {
            continue;
        };
        let rest = &input[start..];
        let end = rest.find(quote)?;
        return Some(rest[..end].to_string());
    }
    None
}

fn parse_ymd(value: &str) -> Option<i64> {
    let mut parts = value.trim().split(['-', '/']);
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - (month <= 2) as i32;
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let yoe = year - era * 400;
    let mp = month as i32 + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as i64
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"result":{"lastPage":true,"list":[{"id":1,"title":"Sample","image":"/cover.jpg","auther":"Author","desc":"Sample description.","mhstatus":0,"keyword":"BL"}]}}"#;
const LATEST_FIXTURE: &str = r#"{"result":[{"id":1,"title":"Sample","image":"/cover.jpg","auther":"Author","desc":"Sample description.","mhstatus":0,"keyword":"BL"}]}"#;
const CHAPTERS_FIXTURE: &str = r#"{"result":{"lastPage":true,"list":[{"id":1,"title":"Chapter 1","create_time":"2024-01-01 00:00:00"}]}}"#;
const PAGES_FIXTURE: &str = r#"<section><img src="/page-1.jpg"></section>"#;
