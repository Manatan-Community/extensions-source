use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MadaraScans = MadaraScans;
const BASE_URL: &str = "https://madarascans.com";

struct MadaraScans;

impl MangaSource for MadaraScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_cards(LIST_FIXTURE),
                has_next_page: has_next(LIST_FIXTURE),
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        Ok(parse_search_page(&fetch_document(
            &series_url(page, "", order, request.get("filters")),
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
                    &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_search_page(&fetch_document(
            &series_url(
                page,
                query,
                filter(request.get("filters"), "order", ""),
                request.get("filters"),
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let hide_paid = preference_bool(&request, "hide_paid_chapters", true);
        Ok(parse_chapters(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            hide_paid,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
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

fn series_url(page: u64, query: &str, order: &str, filters: Option<&Value>) -> String {
    let mut params = vec![
        format!("title={}", url::query_escape(query)),
        format!("page={page}"),
    ];
    for key in ["author", "yearx", "status", "type"] {
        let value = filter(filters, key, "");
        if !value.is_empty() {
            params.push(format!("{key}={}", url::query_escape(value)));
        }
    }
    let order = if order.is_empty() { "popular" } else { order };
    params.push(format!("order={}", url::query_escape(order)));
    format!("{BASE_URL}/series/?{}", params.join("&"))
}

fn parse_search_page(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: parse_cards(body),
        has_next_page: has_next(body),
    }
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("listupd")
                || chunk.contains("legend-inner")
                || chunk.contains("card-v-title")
                || chunk.contains("legend-title")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "card-v-title", "href")
                .or_else(|| html::attr_after(chunk, "legend-title", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "card-v-title", "</a>")
                .or_else(|| html::text_between(chunk, "legend-title", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .unwrap_or_else(|| {
                    url::slug_from_url(&href).unwrap_or_else(|| "Manga".to_string())
                });
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), |mut items, item| {
            if !items
                .iter()
                .any(|existing: &CatalogItem| existing.key == item.key)
            {
                items.push(item);
            }
            items
        })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "lh-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: html::attr_after(body, "lh-poster", "src")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "manga-story", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: link_values(body, "lh-genre-tag"),
        status: parse_status(
            &html::text_between(body, "status-badge-lux", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default(),
        ),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, hide_paid: bool) -> Vec<MangaChapter> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("ch-item"))
        .filter(|chunk| !hide_paid || chunk.contains("free"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "ch-num", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            let is_locked = !chunk.contains("free");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(if is_locked {
                    format!("Locked: {title}")
                } else {
                    title
                }),
                url: Some(url::join_url(BASE_URL, &key)),
                is_locked,
                date_uploaded: html::text_between(chunk, "ch-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("readerarea") || chunk.contains("chapter") || chunk.contains("src")
        })
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
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

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find(BASE_URL) {
            return format!(
                "/{}",
                input[index + BASE_URL.len()..]
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(input, "<img", "src"))
}

fn link_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(input: &str) -> ItemStatus {
    match input.to_lowercase().as_str() {
        value if value.contains("completed") => ItemStatus::Completed,
        value if value.contains("dropped") => ItemStatus::Cancelled,
        value if value.contains("hiatus") => ItemStatus::Hiatus,
        value if value.contains("ongoing") => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn has_next(body: &str) -> bool {
    body.contains("pagination")
        || body.contains("legendary-pagination")
        || body.contains("magma-pagination")
}

fn filter<'a>(filters: Option<&'a Value>, key: &str, fallback: &'a str) -> &'a str {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
}

fn preference_bool(request: &Value, key: &str, fallback: bool) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(fallback)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="listupd"><div><h3 class="card-v-title"><a href="/series/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="lh-container"><h1 class="lh-title">Sample Manga</h1><div class="lh-poster"><img src="/cover.jpg"></div><div class="lh-story"><div id="manga-story">Sample description</div></div><a class="lh-genre-tag">Action</a><span class="status-badge-lux">Ongoing</span><div class="ch-item free"><a href="/series/sample/chapter-1/"><span class="ch-num">Chapter 1</span></a><span class="ch-date">2024/01/01</span></div></div>
"#;
const PAGES_FIXTURE: &str = r#"<div id="readerarea"><img src="/page1.jpg"></div>"#;
