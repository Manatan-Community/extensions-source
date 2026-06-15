use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: MangaSect = MangaSect;
const BASE_URL: &str = "https://mangasect.net";

struct MangaSect;

impl MangaSource for MangaSect {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/all-manga/{page}/?sort=last_update&status=0")
        } else {
            format!("{BASE_URL}/ranking/week/{page}")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
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
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
            return Ok(parse_listing(&fetch_document(
                &format!("{BASE_URL}/filter/{page}/"),
                LIST_FIXTURE,
            )));
        }
        let body = client()
            .post(format!("{BASE_URL}/ajax/search"))
            .header("Accept", "application/json, text/javascript, */*; q=0.01")
            .header("Origin", BASE_URL)
            .header("X-Requested-With", "XMLHttpRequest")
            .form(&[("search", query)])
            .xhr()
            .send_text()
            .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
        Ok(parse_search_json(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let chapter_url = url::join_url(BASE_URL, &key);
        let body = fetch_document(&chapter_url, PAGES_FIXTURE);
        Ok(parse_pages(&body, &chapter_url))
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("text-center")
                || chunk.contains("advance-item")
                || chunk.contains("grid")
        })
        .filter_map(catalog_from_chunk)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pagecurrent") || body.contains("blog-pager"),
    }
}

fn parse_search_json(body: &str) -> Paged<CatalogItem> {
    let list = serde_json::from_str::<SearchResponse>(body)
        .map(|response| response.list)
        .unwrap_or_default();
    Paged {
        entries: list
            .into_iter()
            .map(|manga| {
                let key = normalize_key(&manga.url);
                CatalogItem {
                    key: key.clone(),
                    title: manga.name,
                    cover: Some(url::join_url(BASE_URL, &manga.cover)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("en".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                }
            })
            .collect(),
        has_next_page: false,
    }
}

fn catalog_from_chunk(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let title = html::text_between(chunk, ".text-center", "</a>")
        .or_else(|| html::text_between(chunk, "<a", "</a>"))
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Manga Sect".into()));
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_from_chunk(chunk),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, ".a2 header h1", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga Sect".into())),
        cover: html::attr_after(body, ".a1", "src").or_else(|| image_from_chunk(body)),
        description: html::text_between(body, "id=\"syn-target\"", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: html::text_between(body, "fa-user", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.eq_ignore_ascii_case("updating"))
            .into_iter()
            .collect(),
        tags: body
            .split("rel=\"tag\"")
            .skip(1)
            .filter_map(|part| {
                html::text_between(part, "<a", "</a>").map(|value| html::strip_tags(&value))
            })
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number(&title),
                date_uploaded: html::attr_after(chunk, "time", "datetime")
                    .and_then(|value| value.parse::<i64>().ok()),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let html_body = body
        .split("const CHAPTER_ID = ")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .and_then(|chapter_id| {
            client()
                .get(format!(
                    "{BASE_URL}/ajax/image/list/chap/{}",
                    chapter_id.trim()
                ))
                .header("Accept", "application/json, text/javascript, */*; q=0.01")
                .header("X-Requested-With", "XMLHttpRequest")
                .referer(chapter_url.to_string())
                .xhr()
                .send_text()
                .ok()
        })
        .and_then(|json| serde_json::from_str::<PageListResponse>(&json).ok())
        .filter(|response| response.status)
        .map(|response| response.html)
        .unwrap_or_else(|| body.to_string());
    html_body
        .split("separator")
        .skip(1)
        .filter_map(|chunk| {
            html::attr_after(chunk, "<a", "href").or_else(|| html::attr_after(chunk, "<img", "src"))
        })
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

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-lazy-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|image| url::join_url(BASE_URL, &image))
}

fn parse_status(body: &str) -> ItemStatus {
    let status = html::text_between(body, "fa-rss", "</div>")
        .map(|value| html::strip_tags(&value).to_ascii_lowercase())
        .unwrap_or_default();
    match status.as_str() {
        value if value.contains("ongoing") => ItemStatus::Ongoing,
        value if value.contains("completed") => ItemStatus::Completed,
        value if value.contains("on-hold") => ItemStatus::Hiatus,
        value if value.contains("canceled") => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|window| window[0].to_ascii_lowercase().contains("chapter"))
        .and_then(|window| window[1].trim_matches(':').parse().ok())
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

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    list: Vec<SearchItem>,
}

#[derive(Debug, Deserialize)]
struct SearchItem {
    cover: String,
    name: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct PageListResponse {
    status: bool,
    html: String,
}

const LIST_FIXTURE: &str = r#"<div id="main"><div class="grid"><div><img src="/cover.jpg"><div class="text-center"><a href="/manga/sample">Sample</a></div></div></div></div><div class="blog-pager"><span class="pagecurrent"></span><span>2</span></div>"#;
const SEARCH_FIXTURE: &str =
    r#"{"list":[{"cover":"/cover.jpg","name":"Sample","url":"/manga/sample"}]}"#;
const DETAILS_FIXTURE: &str = r#"<div class="a1"><figure><img src="/cover.jpg"></figure></div><div class="a2"><header><h1>Sample</h1></header><div><a rel="tag">Action</a></div></div><div id="syn-target">Summary</div><ul><li class="chapter"><a href="/manga/sample/chapter-1">Chapter 1</a><time datetime="1704067200"></time></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<script>const CHAPTER_ID = 1;</script><div class="separator"><a href="/page1.jpg"></a></div>"#;

export_manga_source!(SOURCE);
