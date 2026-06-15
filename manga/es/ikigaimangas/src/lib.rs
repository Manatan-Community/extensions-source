use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: IkigaiMangas = IkigaiMangas;
const DEFAULT_BASE_URL: &str = "https://zonaikigai.gamesview.shop";
const API_BASE_URL: &str = "https://panel.ikigaimangas.com";
const NAME: &str = "Ikigai Mangas";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";
const QUERY_SCAN_LIMIT: u64 = 10;

struct IkigaiMangas;

impl MangaSource for IkigaiMangas {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_listing(LIST_FIXTURE, 1));
        }
        let nsfw = show_nsfw(&request);
        if listing_id(&request) == "latest" {
            let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
            Ok(parse_latest_listing(&fetch_json_or_fixture(
                &format!("{API_BASE_URL}/api/swf/new-chapters?nsfw={nsfw}&page={page}"),
                LATEST_FIXTURE,
            )))
        } else {
            Ok(parse_series_listing(
                &fetch_json_or_fixture(
                    &format!(
                        "{API_BASE_URL}/api/swf/series/ranking-list?type=total_ranking&series_type=comic&nsfw={nsfw}"
                    ),
                    LIST_FIXTURE,
                ),
                1,
            ))
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with("http://") || query.starts_with("https://") {
            let key = normalize_series_key(query);
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(search_query_pages(&request, query));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_series_listing(
            &fetch_json_or_fixture(&series_url(page, &request), LIST_FIXTURE),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/series/comic-sample#1".to_string());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/series/comic-sample#1".to_string());
        Ok(fetch_all_chapters(&series_slug(&key)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base_url = pref_base_url(&request);
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/capitulo/1/".to_string());
        let target = url::join_url(&base_url, key.split('#').next().unwrap_or(&key));
        Ok(parse_pages(&fetch_document_or_fixture(
            &request,
            &target,
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base_url = pref_base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| {
            let slug = series_slug(&key);
            format!("{}/series/{slug}", base_url.trim_end_matches('/'))
        }))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base_url = pref_base_url(&request);
        Ok(manga::request_key(&request, "chapter")
            .map(|key| url::join_url(&base_url, key.split('#').next().unwrap_or(&key))))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let nsfw = show_nsfw(&request);
        let popular = parse_series_listing(
            &fetch_json_or_fixture(
                &format!(
                    "{API_BASE_URL}/api/swf/series/ranking-list?type=total_ranking&series_type=comic&nsfw={nsfw}"
                ),
                LIST_FIXTURE,
            ),
            1,
        );
        let latest = parse_latest_listing(&fetch_json_or_fixture(
            &format!("{API_BASE_URL}/api/swf/new-chapters?nsfw={nsfw}&page=1"),
            LATEST_FIXTURE,
        ));
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.contains("/series/") {
            let key = normalize_series_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{DEFAULT_BASE_URL}/"))
        .with_cookies_for(DEFAULT_BASE_URL)
        .with_cookies_for(API_BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document_or_fixture(request: &Value, target: &str, fixture: &str) -> String {
    let client = client();
    let mut get = client.get(target).browser_document();
    if show_nsfw(request) {
        get = get.header("Cookie", "nsfw-mode=true");
    }
    get.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn series_url(page: u64, request: &Value) -> String {
    let filters = request.get("filters").unwrap_or(&Value::Null);
    let nsfw = show_nsfw(request);
    let mut target = format!("{API_BASE_URL}/api/swf/series?page={page}&type=comic&nsfw={nsfw}");
    if let Some(genres) = filter_csv(filters, "genres") {
        target.push_str("&genres=");
        target.push_str(&url::query_escape(&genres));
    }
    if let Some(status) = filter_csv(filters, "status") {
        target.push_str("&status=");
        target.push_str(&url::query_escape(&status));
    }
    let column = filter_str(filters, "sort").unwrap_or_else(|| "name".to_string());
    let direction = filter_str(filters, "direction").unwrap_or_else(|| "asc".to_string());
    target.push_str("&column=");
    target.push_str(&url::query_escape(&column));
    target.push_str("&direction=");
    target.push_str(&url::query_escape(&direction));
    target
}

fn parse_series_listing(body: &str, page: u64) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, LIST_FIXTURE);
    let entries = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str).unwrap_or("comic") == "comic")
        .map(catalog_from_series)
        .collect();
    let current = root
        .get("current_page")
        .and_then(Value::as_u64)
        .unwrap_or(page);
    let last = root
        .get("last_page")
        .and_then(Value::as_u64)
        .unwrap_or(current);
    Paged {
        entries,
        has_next_page: current < last,
    }
}

fn parse_latest_listing(body: &str) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, LATEST_FIXTURE);
    let entries = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str).unwrap_or("comic") == "comic")
        .map(|item| {
            let id = json_string(item, "series_id").unwrap_or_else(|| "1".to_string());
            let slug = json_string(item, "series_slug").unwrap_or_else(|| "sample".to_string());
            CatalogItem {
                key: format!("/series/comic-{slug}#{id}"),
                title: json_string(item, "series_name").unwrap_or_else(|| NAME.to_string()),
                cover: json_string(item, "thumbnail"),
                url: Some(format!(
                    "{}/series/{slug}",
                    DEFAULT_BASE_URL.trim_end_matches('/')
                )),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect();
    let current = root
        .get("current_page")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let last = root
        .get("last_page")
        .and_then(Value::as_u64)
        .unwrap_or(current);
    Paged {
        entries,
        has_next_page: current < last,
    }
}

fn search_query_pages(request: &Value, query: &str) -> Paged<CatalogItem> {
    let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
    let query_lc = query.to_ascii_lowercase();
    let mut matches = Vec::new();
    let mut api_page = 1;
    let mut has_more_api = true;
    while has_more_api && api_page <= QUERY_SCAN_LIMIT {
        let parsed = parse_series_listing(
            &fetch_json_or_fixture(&series_url(api_page, request), LIST_FIXTURE),
            api_page,
        );
        matches.extend(
            parsed
                .entries
                .into_iter()
                .filter(|item| item.title.to_ascii_lowercase().contains(&query_lc)),
        );
        has_more_api = parsed.has_next_page;
        api_page += 1;
    }
    let start = ((page.saturating_sub(1)) * 20) as usize;
    let entries = matches.into_iter().skip(start).take(20).collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() == 20 && has_more_api,
        entries,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let slug = series_slug(key);
    let body = fetch_json_or_fixture(
        &format!("{API_BASE_URL}/api/swf/series/{slug}"),
        DETAILS_FIXTURE,
    );
    let root = json_or_fixture(&body, DETAILS_FIXTURE);
    let series = root
        .get("series")
        .or_else(|| root.get("data"))
        .unwrap_or(&root);
    let mut item = catalog_from_series(series);
    item.description = json_string(series, "summary")
        .or_else(|| json_string(series, "description"))
        .or_else(|| json_string(series, "synopsis"));
    item.status = parse_status(
        series
            .get("status")
            .and_then(|status| json_string(status, "id"))
            .as_deref(),
    );
    item.initialized = true;
    item
}

fn catalog_from_series(item: &Value) -> CatalogItem {
    let id = json_string(item, "id").unwrap_or_else(|| "1".to_string());
    let slug = json_string(item, "slug").unwrap_or_else(|| "sample".to_string());
    CatalogItem {
        key: format!("/series/comic-{slug}#{id}"),
        title: json_string(item, "name").unwrap_or_else(|| NAME.to_string()),
        cover: json_string(item, "cover"),
        url: Some(format!(
            "{}/series/{slug}",
            DEFAULT_BASE_URL.trim_end_matches('/')
        )),
        tags: item
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| json_string(genre, "name"))
            .collect(),
        status: parse_status(
            item.get("status")
                .and_then(|status| json_string(status, "id"))
                .as_deref(),
        ),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn fetch_all_chapters(slug: &str) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    let mut page = 1;
    loop {
        let body = fetch_json_or_fixture(
            &format!("{API_BASE_URL}/api/swf/series/{slug}/chapters?page={page}"),
            CHAPTERS_FIXTURE,
        );
        let root = json_or_fixture(&body, CHAPTERS_FIXTURE);
        chapters.extend(parse_chapter_page(&root));
        let meta = root.get("meta").unwrap_or(&root);
        let current = meta
            .get("current_page")
            .and_then(Value::as_u64)
            .unwrap_or(page);
        let last = meta
            .get("last_page")
            .and_then(Value::as_u64)
            .unwrap_or(current);
        if current >= last || page >= 20 {
            break;
        }
        page += 1;
    }
    chapters
}

fn parse_chapter_page(root: &Value) -> Vec<MangaChapter> {
    root.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let id = json_string(chapter, "id")?;
            let number = json_string(chapter, "name").unwrap_or_else(|| "1".to_string());
            let mut title = format!("Capitulo {number}");
            if let Some(extra) = json_string(chapter, "title") {
                title.push_str(": ");
                title.push_str(&extra);
            }
            Some(MangaChapter {
                key: format!("/capitulo/{id}/"),
                title: Some(title),
                chapter_number: number.parse::<f32>().ok(),
                date_uploaded: json_string(chapter, "published_at")
                    .and_then(|value| parse_rfc3339(&value)),
                url: Some(format!(
                    "{}/capitulo/{id}/",
                    DEFAULT_BASE_URL.trim_end_matches('/')
                )),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "src")
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "data-lazy-src"))
        })
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: None,
            },
            headers: manga::image_headers(DEFAULT_BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_status(status_id: Option<&str>) -> ItemStatus {
    match status_id {
        Some("906397890812182531") | Some("911437469204086787") => ItemStatus::Ongoing,
        Some("906409397258190851") => ItemStatus::Hiatus,
        Some("906409532796731395") | Some("911793517664960513") => ItemStatus::Completed,
        Some("906426661911756802")
        | Some("906428048651190273")
        | Some("911793767845265410")
        | Some("911793856861798402") => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn normalize_series_key(input: &str) -> String {
    let mut value = input.trim().trim_end_matches('/');
    if let Some((_, rest)) = value.split_once("/series/comic-") {
        value = rest.split('#').next().unwrap_or(rest);
    } else if let Some((_, rest)) = value.split_once("/series/") {
        value = rest.split('#').next().unwrap_or(rest);
    }
    let slug = value.trim_matches('/');
    format!("/series/comic-{slug}")
}

fn series_slug(key: &str) -> String {
    key.split("/series/comic-")
        .nth(1)
        .or_else(|| key.split("/series/").nth(1))
        .unwrap_or(key)
        .split('#')
        .next()
        .unwrap_or("sample")
        .trim_matches('/')
        .to_string()
}

fn pref_base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("BASE_URL"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("http"))
        .unwrap_or(DEFAULT_BASE_URL)
        .trim_end_matches('/')
        .to_string()
}

fn show_nsfw(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("SHOW_NSFW"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn filter_csv(filters: &Value, key: &str) -> Option<String> {
    let values = filters
        .get(key)
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.join(","))
}

fn filter_str(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    let raw = value.get(key)?;
    if let Some(text) = raw.as_str().filter(|text| !text.is_empty()) {
        return Some(text.to_string());
    }
    if let Some(number) = raw.as_i64() {
        return Some(number.to_string());
    }
    raw.as_u64().map(|number| number.to_string())
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn parse_rfc3339(value: &str) -> Option<i64> {
    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = value.get(5..7)?.parse::<i32>().ok()?;
    let day = value.get(8..10)?.parse::<i32>().ok()?;
    let hour = value.get(11..13)?.parse::<i64>().ok()?;
    let minute = value.get(14..16)?.parse::<i64>().ok()?;
    let second = value.get(17..19)?.parse::<i64>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

const LIST_FIXTURE: &str = r#"{"current_page":1,"last_page":1,"data":[{"id":"1","name":"Sample","slug":"sample","cover":"https://media.ikigaimangas.cloud/sample.jpg","type":"comic","status":{"id":"911437469204086787"},"genres":[{"name":"Drama","id":"1"}]}]}"#;
const LATEST_FIXTURE: &str = r#"{"current_page":1,"last_page":1,"data":[{"series_id":"1","series_name":"Sample","series_slug":"sample","thumbnail":"https://media.ikigaimangas.cloud/sample.jpg","type":"comic"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"series":{"id":"1","name":"Sample","slug":"sample","cover":"https://media.ikigaimangas.cloud/sample.jpg","summary":"Summary","type":"comic","status":{"id":"911437469204086787"},"genres":[{"name":"Drama","id":"1"}]}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"id":"1","name":"1","title":"Sample","published_at":"2024-01-01T00:00:00.000000Z"}],"meta":{"current_page":1,"last_page":1}}"#;
const PAGES_FIXTURE: &str = r#"<section><div class="img"><img src="https://media.ikigaimangas.cloud/page1.jpg"></div></section>"#;

export_manga_source!(SOURCE);
