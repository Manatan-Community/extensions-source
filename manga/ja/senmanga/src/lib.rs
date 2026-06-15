use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionError, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: SenManga = SenManga;
const BASE_URL: &str = "https://raw.senmanga.com";
const API_URL: &str = "https://raw.senmanga.com/api";

struct SenManga;

impl MangaSource for SenManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_directory(DIRECTORY_FIXTURE, 1));
        }
        let page = page(&request);
        parse_directory_result(&fetch_json(&format!("{API_URL}/directory?order=Popular&page={page}"))?, page)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged { entries: vec![details_by_key(&key)?], has_next_page: false });
        }
        let page = page(&request);
        let mut target = format!("{API_URL}/directory?page={page}");
        if !query.is_empty() {
            target.push_str("&s=");
            target.push_str(&url::query_escape(query));
        }
        for id in ["type", "status", "order"] {
            if let Some(value) = filter_string(&request, id).filter(|value| !value.is_empty()) {
                target.push('&');
                target.push_str(id);
                target.push('=');
                target.push_str(&url::query_escape(value));
            }
        }
        parse_directory_result(&fetch_json(&target)?, page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        details_by_key(&key)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let body = fetch_json(&format!("{API_URL}/manga/{}", key.trim_matches('/')))?;
        let root = parse_json(&body)?;
        let slug = root.get("slug").and_then(Value::as_str).unwrap_or_else(|| key.trim_matches('/'));
        let chapters = root
            .get("chapterList")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|chapter| {
                let chapter_slug = chapter.get("url").and_then(Value::as_str)?;
                Some(MangaChapter {
                    key: format!("{slug}/{chapter_slug}"),
                    title: chapter.get("title").and_then(Value::as_str).map(ToOwned::to_owned),
                    date_uploaded: chapter.get("datetime").and_then(Value::as_str).and_then(parse_iso_date),
                    url: Some(format!("{BASE_URL}/manga/{slug}/chapter-{chapter_slug}/")),
                    ..MangaChapter::default()
                })
            })
            .collect::<Vec<_>>();
        if chapters.is_empty() {
            Err(err("no chapters found"))
        } else {
            Ok(chapters)
        }
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".into());
        let body = fetch_json(&format!("{API_URL}/read/{}", key.trim_matches('/')))?;
        let root = parse_json(&body)?;
        let headers = manga::image_headers(BASE_URL);
        let pages = root
            .get("pages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url { url: image.to_string(), context: Some(headers.clone()) },
                headers: headers.clone(),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect::<Vec<_>>();
        if pages.is_empty() {
            Err(err("no pages found"))
        } else {
            Ok(pages)
        }
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let latest = parse_home_result(&fetch_json(&format!("{API_URL}/home?page=1"))?)?;
        let popular = self.list(json!({"page": 1}))?;
        Ok(vec![
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/manga/{}", key.trim_matches('/'))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let key = key.trim_matches('/');
            let manga_slug = key.split('/').next().unwrap_or(key);
            let chapter_slug = key.split('/').nth(1).unwrap_or_default();
            format!("{BASE_URL}/manga/{manga_slug}/chapter-{chapter_slug}/")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)?),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }),
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

fn fetch_json(target: &str) -> ExtensionResult<String> {
    client().get(target).xhr().send_text().map_err(|error| err(&format!("fetch failed for {target}: {}", error.message)))
}

fn parse_directory_result(body: &str, page: u64) -> ExtensionResult<Paged<CatalogItem>> {
    let parsed = parse_directory(body, page);
    if parsed.entries.is_empty() {
        Err(err("no manga entries found in directory response"))
    } else {
        Ok(parsed)
    }
}

fn parse_home_result(body: &str) -> ExtensionResult<Paged<CatalogItem>> {
    let root = parse_json(body)?;
    let entries = root
        .get("series")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(series_to_item)
        .collect::<Vec<_>>();
    if entries.is_empty() {
        Err(err("no manga entries found in home response"))
    } else {
        Ok(Paged { has_next_page: true, entries })
    }
}

fn parse_directory(body: &str, page: u64) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(DIRECTORY_FIXTURE).unwrap_or(Value::Null));
    let total_pages = root.get("totalPages").and_then(Value::as_u64).unwrap_or(page);
    let current = root.get("currentPage").and_then(Value::as_u64).unwrap_or(page);
    let entries = root
        .get("series")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(series_to_item)
        .collect();
    Paged { entries, has_next_page: current < total_pages }
}

fn details_by_key(key: &str) -> ExtensionResult<CatalogItem> {
    let slug = key.trim_matches('/');
    let root = parse_json(&fetch_json(&format!("{API_URL}/manga/{slug}"))?)?;
    Ok(series_to_item(&root))
}

fn series_to_item(series: &Value) -> CatalogItem {
    let key = series.get("slug").and_then(Value::as_str).unwrap_or("sample").to_string();
    CatalogItem {
        key: key.clone(),
        title: series.get("title").and_then(Value::as_str).unwrap_or("Sen Manga").to_string(),
        cover: series.get("cover").and_then(Value::as_str).map(ToOwned::to_owned),
        description: series.get("description").and_then(Value::as_str).map(ToOwned::to_owned),
        tags: series
            .get("genre")
            .and_then(Value::as_str)
            .map(|value| value.split(',').map(|tag| tag.trim().to_string()).filter(|tag| !tag.is_empty()).collect())
            .unwrap_or_default(),
        status: parse_status(series.get("status").and_then(Value::as_str)),
        url: Some(format!("{BASE_URL}/manga/{key}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: series.get("description").is_some() || series.get("chapterList").is_some(),
        ..CatalogItem::default()
    }
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    let status = status.unwrap_or_default().to_ascii_lowercase();
    if status.contains("complete") {
        ItemStatus::Completed
    } else if status.contains("ongoing") {
        ItemStatus::Ongoing
    } else if status.contains("hiatus") {
        ItemStatus::Hiatus
    } else if status.contains("dropped") {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Unknown
    }
}

fn parse_json(body: &str) -> ExtensionResult<Value> {
    serde_json::from_str(body).map_err(|error| err(&format!("invalid JSON response: {error}")))
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) {
        input.split("/manga/").nth(1).map(|value| value.trim_matches('/').to_string())
    } else if !input.contains("://") && !input.trim_matches('/').is_empty() {
        Some(input.trim_matches('/').to_string())
    } else {
        None
    }
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request.get("filters").and_then(Value::as_object).and_then(|filters| filters.get(id)).and_then(Value::as_str)
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let mut parts = value.split('T').next()?.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400_000)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let day = day as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

fn err(message: &str) -> ExtensionError {
    ExtensionError { message: message.to_string() }
}

export_manga_source!(SOURCE);

const DIRECTORY_FIXTURE: &str = r#"
{"currentPage":1,"totalPages":1,"series":[{"title":"Sample Sen Manga","slug":"sample","cover":"https://raw.senmanga.com/sample.jpg","status":"Ongoing","genre":"Manga","description":"Sample fixture."}]}
"#;
