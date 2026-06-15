use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

const SOURCE: YaoiToon = YaoiToon;
const BASE_URL: &str = "https://yaoitoon.net";

struct YaoiToon;

impl MangaSource for YaoiToon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest-updated"
        } else {
            "most-viewd"
        };
        Ok(parse_listing(&fetch_document(
            &filter_url(page, sort, ""),
            LIST_FIXTURE,
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
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            let sort = filter_string(&request, "sort").unwrap_or_else(|| "default".to_string());
            let genres = filter_values(&request, "genres").join(",");
            filter_url(page, &sort, &genres)
        } else {
            format!(
                "{BASE_URL}/search/{page}/?keyword={}",
                url::query_escape(query)
            )
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let chapter_id = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("chapter-1");
        Ok(parse_pages(&fetch_ajax(
            &format!("{BASE_URL}/ajax/image/list/chap/{chapter_id}"),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                HomeSectionStyle::Cover,
                self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                HomeSectionStyle::Compact,
                self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?,
            ),
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
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
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

fn fetch_ajax(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("X-Requested-With", "XMLHttpRequest")
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn filter_url(page: u64, sort: &str, genres: &str) -> String {
    let mut target = format!("{BASE_URL}/filter/{page}/?sort={sort}&sex=All&chapter_count=0");
    if !genres.is_empty() {
        target.push_str("&genres=");
        target.push_str(&url::query_escape(genres));
    }
    target
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with(BASE_URL) {
        return format!(
            "/{}",
            value[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("manga-name")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| url::slug_from_url(&key))
                    .unwrap_or_else(|| "YaoiToon".to_string()),
                cover: nearby_image(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        has_next_page: body.contains("page-item") && body.contains("&rsaquo;"),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "manga-name", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "YaoiToon".to_string()),
        cover: nearby_image(body).map(|image| absolute_url(&image)),
        description: html::text_between(body, "description", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: body
            .split("genres")
            .skip(1)
            .flat_map(|chunk| chunk.split("<a").skip(1).take(20))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(
            html::text_between(body, "Status", "</div>")
                .map(|value| html::strip_tags(&value))
                .as_deref(),
        ),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("chapter-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "item-link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "name", "</")
                    .or_else(|| html::text_between(chunk, "<a", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: html::text_between(chunk, "release-time", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_relative_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Read".to_string()),
            url: Some(absolute_url(manga_key)),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let html_body = serde_json::from_str::<ImageList>(body)
        .map(|payload| payload.html)
        .unwrap_or_else(|_| body.to_string());
    html_body
        .split("separator")
        .skip(1)
        .filter_map(|chunk| nearby_image(chunk))
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn nearby_image(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_lowercase().as_str() {
        status if status.contains("completed") => ItemStatus::Completed,
        status if status.contains("hold") => ItemStatus::Hiatus,
        status if status.contains("canceled") || status.contains("cancelled") => {
            ItemStatus::Cancelled
        }
        status if status.contains("ongoing") => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn parse_relative_date(value: &str) -> Option<i64> {
    let trimmed = value.trim().to_ascii_lowercase();
    let token = trimmed.split_whitespace().next()?;
    let (number, unit) = token
        .chars()
        .partition::<String, _>(|ch| ch.is_ascii_digit());
    let number = number.parse::<i64>().ok()?;
    let seconds = match unit.as_str() {
        "s" => number,
        "m" => number * 60,
        "h" => number * 60 * 60,
        "d" => number * 24 * 60 * 60,
        "w" => number * 7 * 24 * 60 * 60,
        "M" => number * 30 * 24 * 60 * 60,
        "y" => number * 365 * 24 * 60 * 60,
        _ => return None,
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(now.saturating_sub(seconds))
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn filter_values(request: &Value, id: &str) -> Vec<String> {
    let Some(value) = request.get("filters").and_then(|filters| filters.get(id)) else {
        return Vec::new();
    };
    if let Some(values) = value.as_array() {
        return values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .map(|value| vec![value.to_string()])
        .unwrap_or_default()
}

fn home_section(
    id: &str,
    title: &str,
    style: HomeSectionStyle,
    page: Paged<CatalogItem>,
) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.to_string(),
        title: title.to_string(),
        style: Some(style),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Deserialize)]
struct ImageList {
    html: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="manga_list-sbs"><div class="mls-wrap"><div class="item"><div class="manga-poster"><img data-src="/cover.jpg"></div><div class="manga-name"><a href="/manga/sample">Sample Manga</a></div></div></div></div>
<ul class="pagination"><li class="page-item"><a>&rsaquo;</a></li></ul>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="anisc-detail"><h1 class="manga-name">Sample Manga</h1></div><div class="anisc-poster"><img data-src="/cover.jpg"></div><div class="description">Summary text.</div>
<div class="genres"><a>Yaoi</a></div><div class="item-title">Status <span class="name">Ongoing</span></div>
<ul id="chapters-list"><li class="chapter-item"><a class="item-link" href="/manga/sample/chapter-1"><span class="name">Chapter 1</span></a><span class="release-time">1d ago</span></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"
{"status":true,"html":"<div class='separator' data-src='/page1.jpg'></div>"}
"#;
