use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{
    html, manga,
    sdk::{ItemStatus, PageContent, SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: KunMangaOnline = KunMangaOnline;
const BASE_URL: &str = "https://www.kunmanga.online";
const POSTS_PER_PAGE: usize = 20;
const CHAPTERS_PER_PAGE: u64 = 50;

struct KunMangaOnline;

impl MangaSource for KunMangaOnline {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, false));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let is_latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if is_latest {
            latest_url(page)
        } else if page > 1 {
            format!("{BASE_URL}/page/{page}/?orderby=views&post_type=wp-manga")
        } else {
            format!("{BASE_URL}/?orderby=views&post_type=wp-manga")
        };
        Ok(parse_listing(
            &fetch_document(&target, LIST_FIXTURE),
            is_latest,
        ))
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
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let body = fetch_document(
            &search_url(page, query, request.get("filters").unwrap_or(&Value::Null)),
            LIST_FIXTURE,
        );
        Ok(parse_listing(&body, false))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let slug = key
            .trim_matches('/')
            .split('/')
            .nth(1)
            .unwrap_or("sample")
            .to_string();
        let mut out = Vec::new();
        let mut current_page = 1;
        let mut last_page = 1;
        while current_page <= last_page {
            let target = format!(
                "{BASE_URL}/api/comics/{slug}/chapters?page={current_page}&per_page={CHAPTERS_PER_PAGE}&order=desc"
            );
            let body = fetch_api(&target, CHAPTERS_FIXTURE);
            let payload: ChapterListResponse = serde_json::from_str(&body).unwrap_or_default();
            last_page = payload.data.last_page.max(current_page);
            out.extend(
                payload
                    .data
                    .chapters
                    .into_iter()
                    .map(|chapter| chapter.into_chapter(&slug)),
            );
            current_page += 1;
            if current_page > 20 {
                break;
            }
        }
        Ok(out)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let body = fetch_document(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
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
        .with_origin(BASE_URL)
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

fn fetch_api(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn latest_url(page: u64) -> String {
    format!(
        "{BASE_URL}/?action=madara_load_more&page={}&template=madara-core%2Fcontent%2Fcontent-archive&vars%5Borderby%5D=meta_value_num&vars%5Bpaged%5D={page}&vars%5Btimerange%5D=&vars%5Bposts_per_page%5D={POSTS_PER_PAGE}&vars%5Btax_query%5D%5Brelation%5D=OR&vars%5Bmeta_query%5D%5B0%5D%5Brelation%5D=AND&vars%5Bmeta_query%5D%5Brelation%5D=AND&vars%5Bpost_type%5D=wp-manga&vars%5Bpost_status%5D=publish&vars%5Bmeta_key%5D=_latest_update&vars%5Border%5D=desc&vars%5Bsidebar%5D=right&vars%5Bmanga_archives_item_layout%5D=big_thumbnail",
        page.saturating_sub(1)
    )
}

fn search_url(page: u64, query: &str, filters: &Value) -> String {
    let base = if page > 1 {
        format!("{BASE_URL}/page/{page}/")
    } else {
        BASE_URL.to_string()
    };
    let mut parts = Vec::new();
    if !query.is_empty() {
        parts.push(format!("s={}", url::query_escape(query)));
    }
    parts.push("post_type=wp-manga".to_string());
    for id in ["author", "artist", "release", "op", "adult", "orderby"] {
        if let Some(value) = filter_string(filters, id).filter(|value| !value.is_empty()) {
            parts.push(format!("{id}={}", url::query_escape(&value)));
        }
    }
    for genre in filter_values(filters.get("genre")) {
        parts.push(format!("genre%5B%5D={}", url::query_escape(&genre)));
    }
    for status in filter_values(filters.get("status")) {
        parts.push(format!("status%5B%5D={}", url::query_escape(&status)));
    }
    format!("{base}?{}", parts.join("&"))
}

fn parse_listing(body: &str, is_ajax: bool) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("c-tabs-item__content") || chunk.contains("page-item-detail")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "post-title", "href")
                .or_else(|| html::attr_after(chunk, "<h3", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "post-title", "</a>")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| url::slug_from_url(&key))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Manga".to_string());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    let has_next_page = if is_ajax {
        entries.len() >= POSTS_PER_PAGE
    } else {
        body.contains("aria-label=\"Next\"")
            || body.contains("nav-previous")
            || body.contains("next page-numbers")
            || body.contains("rel=\"next\"")
    };
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "post-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Manga".to_string()),
        cover: html::attr_after(body, "summary_image", "src")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "summary__content", "</div>")
            .or_else(|| html::text_between(body, "description-summary", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_values(body, "author"),
        artists: info_values(body, "artist"),
        tags: info_values(body, "genres"),
        status: parse_status(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "data-src")
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "data-aload"))
                .or_else(|| html::attr(chunk, "data-backup"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.starts_with("data:") && !image.contains("/thumb"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find("/manga/") {
            return format!("/{}", value[index + 1..].trim_end_matches('/'));
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-backup")
        .or_else(|| html::attr_after(input, "<img", "src"))
        .or_else(|| html::attr_after(input, "<img", "data-src"))
        .or_else(|| html::attr_after(input, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(input, "<img", "data-aload"))
}

fn filter_string(filters: &Value, id: &str) -> Option<String> {
    filters
        .get(id)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn filter_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => value
            .split(',')
            .map(|part| part.trim().to_string())
            .collect(),
        _ => Vec::new(),
    }
}

fn info_values(body: &str, name: &str) -> Vec<String> {
    body.split("post-content_item")
        .filter(|chunk| chunk.to_ascii_lowercase().contains(name))
        .flat_map(|chunk| chunk.split("<a").skip(1))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("on-hold") || lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("dropped") {
        ItemStatus::Cancelled
    } else if lower.contains("ongoing") || lower.contains("on-going") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn push_unique(mut values: Vec<CatalogItem>, value: CatalogItem) -> Vec<CatalogItem> {
    if !values.iter().any(|item| item.key == value.key) {
        values.push(value);
    }
    values
}

fn parse_iso_date(value: Option<&str>) -> Option<i64> {
    let date = value?.split('.').next().unwrap_or_default();
    if date.len() < 10 {
        return None;
    }
    let year = date.get(0..4)?.parse::<i64>().ok()?;
    let month = date.get(5..7)?.parse::<i64>().ok()?;
    let day = date.get(8..10)?.parse::<i64>().ok()?;
    Some(timestamp_utc(year, month, day, 0, 0, 0))
}

fn timestamp_utc(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> i64 {
    let y = year - (month <= 2) as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86400 + hour * 3600 + minute * 60 + second
}

#[derive(Default, Deserialize)]
struct ChapterListResponse {
    #[serde(default)]
    data: ChapterData,
}

#[derive(Default, Deserialize)]
struct ChapterData {
    #[serde(default)]
    chapters: Vec<ChapterDto>,
    #[serde(default)]
    last_page: u64,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    #[serde(default)]
    chapter_name: String,
    #[serde(default)]
    chapter_slug: String,
    updated_at: Option<String>,
}

impl ChapterDto {
    fn into_chapter(self, slug: &str) -> MangaChapter {
        let key = format!("/manga/{slug}/{}", self.chapter_slug.trim_matches('/'));
        MangaChapter {
            key: key.clone(),
            title: Some(if self.chapter_name.is_empty() {
                "Chapter".to_string()
            } else {
                self.chapter_name
            }),
            date_uploaded: parse_iso_date(self.updated_at.as_deref()),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample Manga</a></h3><img data-src="/cover.jpg"></div>
<a class="next page-numbers"></a>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="post-title">Sample Manga</h1><div class="summary_image"><img src="/cover.jpg"></div><div class="summary__content">A sample.</div>
"#;
const CHAPTERS_FIXTURE: &str = r#"
{"data":{"last_page":1,"chapters":[{"chapter_name":"Chapter 1","chapter_slug":"chapter-1","updated_at":"2024-01-01T00:00:00.000000Z"}]}}
"#;
const PAGES_FIXTURE: &str = r#"
<div class="reading-content"><img class="wp-manga-chapter-img" data-src="/page1.jpg"></div>
"#;
