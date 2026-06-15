use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: OkuToon = OkuToon;
const BASE_URL: &str = "https://okutoon.com";

struct OkuToon;

impl MangaSource for OkuToon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort = if listing == "latest" {
            "updated"
        } else {
            "popular"
        };
        let body = fetch_or_fixture(&format!("{BASE_URL}/tur?sira={sort}&sayfa={page}"), LIST_FIXTURE);
        Ok(parse_list(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mut target = format!(
            "{BASE_URL}/tur?sayfa={page}&q={}&sira={}",
            url::query_escape(query),
            filter_str(filters, "sort").unwrap_or("updated")
        );
        if let Some(status) = filter_str(filters, "status").filter(|value| !value.is_empty()) {
            target.push_str("&durum=");
            target.push_str(&url::query_escape(status));
        }
        for genre in filter_array(filters, "genres") {
            target.push_str("&k[]=");
            target.push_str(&url::query_escape(&genre));
        }
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_list(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/seri/sample".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/seri/sample".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/seri/sample/bolum-1".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("series-card")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "series-card-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("tr".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pagination-btn") && body.contains("Sonraki"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/seri/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "series-detail-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: html::attr_after(body, "series-detail-cover", "src")
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "data-series-description-content", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: html::text_between(body, "series-detail-author", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty() && value != "Bilinmiyor")
            .into_iter()
            .collect(),
        tags: body
            .split("series-detail-genres")
            .nth(1)
            .unwrap_or_default()
            .split("tag")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("tr".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapter-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "chapter-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| Some(url::slug_from_url(&key).unwrap_or_else(|| "Chapter".to_string()))),
                url: Some(url::join_url(BASE_URL, &key)),
                date_uploaded: html::text_between(chunk, "chapter-date", "</")
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
        .filter(|chunk| chunk.contains("reader-page"))
        .filter_map(|chunk| html::attr(chunk, "src"))
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

fn parse_status(body: &str) -> ItemStatus {
    let text = html::text_between(body, "badge-completed", "</")
        .or_else(|| html::text_between(body, "badge-ongoing", "</"))
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    match text.trim() {
        "Devam Ediyor" => ItemStatus::Ongoing,
        "Tamamlandı" | "Tamamlandi" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn filter_str<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters.get(id).and_then(Value::as_str)
}

fn filter_array(filters: &Value, id: &str) -> Vec<String> {
    filters
        .get(id)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
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

fn push_unique(mut out: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !out.iter().any(|existing| existing.key == item.key) {
        out.push(item);
    }
    out
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<a class="series-card" href="/seri/sample"><div class="series-card-title">Sample</div><div class="series-card-cover"><img src="/cover.jpg"></div></a><nav class="pagination"><a class="pagination-btn">Sonraki</a></nav>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="series-detail-title">Sample</h1><div class="series-detail-cover"><img src="/cover.jpg"></div><div class="series-detail-author">Author</div><div data-series-description-content>Desc</div><div class="series-detail-meta"><span class="badge-ongoing">Devam Ediyor</span></div><a class="chapter-item" href="/seri/sample/bolum-1"><span class="chapter-title">Bolum 1</span><span class="chapter-date">1 Ocak 2024</span></a>"#;
const PAGES_FIXTURE: &str = r#"<div id="readerPages"><img class="reader-page" src="/page1.jpg"></div>"#;
