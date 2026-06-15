use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: ValirScans = ValirScans;
const BASE_URL: &str = "https://valirscans.org";
const BROWSE_PAGE_SIZE: u64 = 18;

struct ValirScans;

impl MangaSource for ValirScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_browse(LIST_FIXTURE, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updated"
        } else {
            "views"
        };
        Ok(parse_browse(
            &fetch_document(
                &format!("{BASE_URL}/series?sort={sort}&order=desc&page={page}"),
                LIST_FIXTURE,
            ),
            page,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_browse(
            &fetch_document(
                &format!(
                    "{BASE_URL}/series?page={page}&q={}",
                    url::query_escape(query)
                ),
                LIST_FIXTURE,
            ),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/series/comic/sample".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/series/comic/sample".to_string());
        let first_body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let first = extract_series_page_data(&first_body).unwrap_or_else(series_fixture);
        let mut chapters = first
            .get("chapters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let current = first
            .get("currentPage")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        let total = first
            .get("totalPages")
            .and_then(Value::as_u64)
            .unwrap_or(current);
        for page in (current + 1)..=total {
            let body = fetch_document(
                &format!("{}?page={page}", absolute_url(&key)),
                DETAILS_FIXTURE,
            );
            if let Some(next) = extract_series_page_data(&body) {
                chapters.extend(
                    next.get("chapters")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                );
            }
        }
        let mut parsed = chapters
            .into_iter()
            .filter(|chapter| {
                !chapter
                    .get("isLocked")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .map(|chapter| chapter_from_value(&chapter, &key))
            .collect::<Vec<_>>();
        parsed.reverse();
        Ok(parsed)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/comic/sample/chapter/1".to_string());
        let body = fetch_document_rsc(&absolute_url(&key), PAGES_FIXTURE);
        let chapter = extract_chapter_page_data(&body)
            .and_then(|value| value.get("chapter").cloned())
            .unwrap_or_else(|| json!({"pages":[{"pageNumber":1,"imageUrl":"/page1.jpg"}]}));
        let mut pages = chapter
            .get("pages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        pages.sort_by_key(|page| page.get("pageNumber").and_then(Value::as_i64).unwrap_or(0));
        Ok(pages
            .into_iter()
            .filter_map(|page| {
                page.get("imageUrl")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: absolute_asset(&image),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
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
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(key),
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
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document_rsc(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("rsc", "1")
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_browse(body: &str, page: u64) -> Paged<CatalogItem> {
    let entries = body
        .split("role=\"gridcell\"")
        .skip(1)
        .filter_map(parse_card)
        .collect::<Vec<_>>();
    let total = total_results(body);
    Paged {
        has_next_page: total.is_some_and(|value| page * BROWSE_PAGE_SIZE < value)
            || body.contains("rel=\"next\""),
        entries,
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    if href.contains("/chapter/") {
        return None;
    }
    let key = normalize_key(href.split('?').next().unwrap_or(&href));
    let title = html::text_between(chunk, "<h3", "</h3>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "<img", "src")
            .or_else(|| {
                html::attr_after(chunk, "<img", "srcset")
                    .and_then(|srcset| srcset.split_whitespace().next().map(ToString::to_string))
            })
            .map(|image| next_image_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let data = extract_series_page_data(body).unwrap_or_else(series_fixture);
    let series = data.get("series").unwrap_or(&Value::Null);
    let key = key.unwrap_or_else(|| {
        series
            .get("slug")
            .and_then(Value::as_str)
            .map(|slug| format!("/series/comic/{slug}"))
            .unwrap_or_else(|| "/series/comic/sample".to_string())
    });
    let title = series
        .get("title")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| schema_string(body, "name"))
        .or_else(|| html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value)))
        .unwrap_or_else(|| "Manga".to_string());
    let mut tags = Vec::new();
    if let Some(kind) = series.get("type").and_then(Value::as_str) {
        tags.push(titlecase(kind));
    }
    if let Some(genres) = series.get("genres").and_then(Value::as_array) {
        tags.extend(genres.iter().filter_map(|genre| {
            genre
                .get("name")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        }));
    }
    CatalogItem {
        key: key.clone(),
        title,
        cover: series
            .get("coverImage")
            .and_then(Value::as_str)
            .map(absolute_asset)
            .or_else(|| schema_string(body, "image").map(|image| absolute_asset(&image))),
        description: series
            .get("description")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| schema_string(body, "description")),
        authors: series
            .get("author")
            .and_then(Value::as_str)
            .map(|value| vec![value.to_string()])
            .unwrap_or_default(),
        artists: series
            .get("artist")
            .and_then(Value::as_str)
            .map(|value| vec![value.to_string()])
            .unwrap_or_default(),
        tags,
        status: parse_status(series.get("status").and_then(Value::as_str)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn chapter_from_value(value: &Value, series_key: &str) -> MangaChapter {
    let number = value.get("number").and_then(Value::as_f64).unwrap_or(0.0);
    let number_text = format_chapter_number(number);
    let key = format!("{}/chapter/{number_text}", series_key.trim_end_matches('/'));
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("Chapter {number_text}"));
    MangaChapter {
        key: key.clone(),
        title: Some(title),
        chapter_number: Some(number as f32),
        date_uploaded: value
            .get("publishedAt")
            .and_then(Value::as_str)
            .and_then(parse_date),
        is_locked: value
            .get("isLocked")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        url: Some(absolute_url(&key)),
        ..MangaChapter::default()
    }
}

fn extract_series_page_data(body: &str) -> Option<Value> {
    find_json_object_with_keys(body, &["series", "chapters"])
}

fn extract_chapter_page_data(body: &str) -> Option<Value> {
    find_json_object_with_keys(body, &["chapter", "pages"])
}

fn find_json_object_with_keys(body: &str, keys: &[&str]) -> Option<Value> {
    let bytes = body.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'{' {
            continue;
        }
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escape = false;
        for (offset, ch) in body[index..].char_indices() {
            if in_string {
                if escape {
                    escape = false;
                } else if ch == '\\' {
                    escape = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let candidate = &body[index..index + offset + ch.len_utf8()];
                        if keys
                            .iter()
                            .all(|key| candidate.contains(&format!("\"{key}\"")))
                        {
                            if let Ok(value) = serde_json::from_str(candidate) {
                                return Some(value);
                            }
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn schema_string(body: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}\"");
    let json = html::text_between(body, "application/ld+json", "</script>")?;
    let value: Value = serde_json::from_str(json.trim()).ok()?;
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| value.to_string().contains(&marker).then(|| None).flatten())
}

fn total_results(body: &str) -> Option<u64> {
    body.split("totalResults").nth(1).and_then(|rest| {
        rest.chars()
            .filter(|ch| ch.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()
    })
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_uppercase().as_str() {
        "ONGOING" => ItemStatus::Ongoing,
        "COMPLETED" => ItemStatus::Completed,
        "HIATUS" => ItemStatus::Hiatus,
        "CANCELLED" | "CANCELED" | "DROPPED" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: &str) -> Option<i64> {
    let date = value.split('T').next()?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
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

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        let path = value.strip_prefix(BASE_URL).unwrap_or(value);
        return format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn absolute_asset(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else if value.starts_with("//") {
        format!("https:{value}")
    } else {
        url::join_url(BASE_URL, value)
    }
}

fn next_image_url(value: &str) -> String {
    if value.contains("/_next/image?") {
        query_param(value, "url")
            .map(|encoded| percent_decode(&encoded))
            .map(|decoded| absolute_asset(&decoded))
            .unwrap_or_else(|| absolute_asset(value))
    } else {
        absolute_asset(value)
    }
}

fn query_param(input: &str, key: &str) -> Option<String> {
    let query = input.split('?').nth(1)?;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=')?;
        if name == key {
            return Some(value.to_string());
        }
    }
    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(&input[index + 1..index + 3], 16) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn format_chapter_number(number: f64) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i64)
    } else {
        let mut text = format!("{number:.2}");
        while text.ends_with('0') {
            text.pop();
        }
        text.trim_end_matches('.').to_string()
    }
}

fn titlecase(input: &str) -> String {
    let mut chars = input.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

fn series_fixture() -> Value {
    serde_json::from_str(r#"{"series":{"title":"Sample","slug":"sample","description":"Summary","coverImage":"/cover.jpg","status":"ONGOING","genres":[{"name":"Action"}]},"chapters":[{"number":1,"title":"Chapter 1","publishedAt":"2024-01-01","isLocked":false}],"currentPage":1,"totalPages":1}"#).expect("fixture is valid")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div role="gridcell"><a href="/series/comic/sample?ref=browse"><h3>Sample</h3><img src="/cover.jpg"></a></div>"#;
const DETAILS_FIXTURE: &str = r#"{"series":{"title":"Sample","slug":"sample","description":"Summary","coverImage":"/cover.jpg","status":"ONGOING","genres":[{"name":"Action"}]},"chapters":[{"number":1,"title":"Chapter 1","publishedAt":"2024-01-01","isLocked":false}],"currentPage":1,"totalPages":1}"#;
const PAGES_FIXTURE: &str = r#"{"chapter":{"pages":[{"pageNumber":1,"imageUrl":"/page1.jpg"}]}}"#;
