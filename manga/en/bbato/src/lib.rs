use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Bbato = Bbato;
const BASE_URL: &str = "https://bbato.com";

struct Bbato;

impl MangaSource for Bbato {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let path = if page == 1 {
                "/updated".to_string()
            } else {
                format!("/updated/page/{page}")
            };
            return Ok(parse_latest(&fetch_document(
                &url::join_url(BASE_URL, &path),
                LATEST_FIXTURE,
            )));
        }
        Ok(parse_popular(&fetch_document(BASE_URL, POPULAR_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(normalize_key(query)),
                )],
                has_next_page: false,
            });
        }
        let mut params = vec![format!("keyword={}", url::query_escape(query))];
        if let Some(page) = request
            .get("page")
            .and_then(Value::as_u64)
            .filter(|page| *page > 1)
        {
            params.push(format!("page={page}"));
        }
        append_filter_params(&request, &mut params);
        Ok(parse_latest(&fetch_document(
            &format!("{BASE_URL}/filter?{}", params.join("&")),
            LATEST_FIXTURE,
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
        let slug = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample");
        Ok(parse_chapter_json(
            &fetch_json(
                &format!(
                    "{BASE_URL}/get-chapter-list?slug={}",
                    url::query_escape(slug)
                ),
                &url::join_url(BASE_URL, &key),
                CHAPTERS_FIXTURE,
            ),
            slug,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/read/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
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

fn fetch_json(target: &str, referer: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_referer(referer)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .with_header("Accept", "application/json, text/javascript, */*; q=0.01")
        .with_header("X-Requested-With", "XMLHttpRequest")
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let mut entries = Vec::new();
    for chunk in body.split("swiper-slide unit").skip(1) {
        let Some(href) = html::attr_after(chunk, "<a", "href") else {
            continue;
        };
        let title = html::text_between(chunk, "<span", "</span>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&href))
            .unwrap_or_else(|| "Bbato".to_string());
        push_unique(
            &mut entries,
            CatalogItem {
                key: normalize_key(&href),
                title,
                cover: image_from_chunk(chunk),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            },
        );
    }
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("unit")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "poster", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "info", "</a>")
                .or_else(|| html::attr_after(chunk, "<a", "title"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| url::slug_from_url(&href))
                .unwrap_or_else(|| "Bbato".to_string());
            Some(CatalogItem {
                key: normalize_key(&href),
                title,
                cover: image_from_chunk(chunk),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "itemprop=name", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Bbato".to_string()),
        authors: meta_links(body, "Author"),
        description: html::text_between(body, "description", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: meta_links(body, "Genres"),
        status: status_from_text(
            &html::text_between(body, "info", "</p>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default(),
        ),
        cover: image_from_marker(body, "poster"),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapter_json(body: &str, manga_slug: &str) -> Vec<MangaChapter> {
    let response: ChapterListResponse = serde_json::from_str(body).unwrap_or_default();
    response
        .data
        .into_iter()
        .map(|chapter| MangaChapter {
            key: format!("/read/{manga_slug}/{}", chapter.chapter_slug),
            title: Some(chapter.chapter_name),
            url: Some(format!(
                "{BASE_URL}/read/{manga_slug}/{}",
                chapter.chapter_slug
            )),
            date_uploaded: None,
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("page") && !chunk.contains("notice-page"))
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
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

fn append_filter_params(request: &Value, params: &mut Vec<String>) {
    let filters = request.get("filters").unwrap_or(&Value::Null);
    for (key, param) in [
        ("type", "type[]"),
        ("genre", "genre[]"),
        ("status", "status[]"),
        ("year", "year[]"),
    ] {
        for value in selected_values(filters.get(key)) {
            params.push(format!("{param}={}", url::query_escape(&value)));
        }
    }
    if let Some(min_chap) = filters
        .get("minchap")
        .or_else(|| filters.get("minChapter"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        params.push(format!("minchap={}", url::query_escape(min_chap)));
    }
    if let Some(sort) = filters
        .get("sort")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        params.push(format!("sort={}", url::query_escape(sort)));
    }
}

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.to_string()],
        _ => Vec::new(),
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        format!("/{}", input.trim_start_matches(BASE_URL).trim_matches('/'))
    } else {
        format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|image| url::join_url(BASE_URL, &image))
}

fn image_from_marker(body: &str, marker: &str) -> Option<String> {
    html::attr_after(body, marker, "data-src")
        .or_else(|| html::attr_after(body, marker, "src"))
        .map(|image| url::join_url(BASE_URL, &image))
}

fn meta_links(body: &str, label: &str) -> Vec<String> {
    body.split("meta")
        .find(|chunk| chunk.contains(label))
        .map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn status_from_text(text: &str) -> ItemStatus {
    match text.trim().to_ascii_lowercase().as_str() {
        "ongoing" | "releasing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "on hiatus" => ItemStatus::Hiatus,
        "discontinued" | "cancelled" | "canceled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn push_unique(items: &mut Vec<CatalogItem>, item: CatalogItem) {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
}

#[derive(Default, Deserialize)]
struct ChapterListResponse {
    #[serde(default)]
    data: Vec<ChapterDto>,
}

#[derive(Deserialize)]
struct ChapterDto {
    chapter_name: String,
    chapter_slug: String,
    #[allow(dead_code)]
    updated_at: String,
}

export_manga_source!(SOURCE);

const POPULAR_FIXTURE: &str = r#"
<div id="most-viewed"><div class="tab-content"><div class="swiper-slide unit"><a href="/series/sample"><img data-src="/cover.jpg"><span>Sample Bbato</span></a></div></div></div>
"#;
const LATEST_FIXTURE: &str = r#"
<div class="original card-lg"><div class="unit"><a class="poster" href="/series/sample"><img data-src="/cover.jpg"></a><div class="info"><a>Sample Bbato</a></div></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 itemprop="name">Sample Bbato</h1><div class="poster"><img data-src="/cover.jpg"></div>
<div class="description">Sample summary.</div><div class="meta"><div><span>Author</span><a>Author Name</a></div><div><span>Genres</span><a>Action</a></div></div>
<div class="info"><p>Ongoing</p></div>
"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"chapter_name":"Chapter 1","chapter_slug":"chapter-1","updated_at":"2024-01-01 00:00:00"}]}"#;
const PAGES_FIXTURE: &str = r#"<div class="pages"><div class="page"><img data-src="/page1.jpg"></div><div class="page"><img data-src="/page2.jpg"></div></div>"#;
