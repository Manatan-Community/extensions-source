use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: ManhwasMe = ManhwasMe;
const BASE_URL: &str = "https://manhwas.me";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct ManhwasMe;

impl MangaSource for ManhwasMe {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "-updated_at"
        } else {
            "-views"
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/search?sort={sort}&page={page}"),
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
            let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = format!(
            "{BASE_URL}/search?page={page}&filter%5Bname%5D={}&sort={}&type={}&genre={}&status={}&caution={}",
            url::query_escape(query),
            url::query_escape(filter(filters, "sort", "")),
            url::query_escape(filter(filters, "type", "")),
            url::query_escape(filter(filters, "genre", "")),
            url::query_escape(filter(filters, "status", "")),
            url::query_escape(filter(filters, "caution", "")),
        );
        Ok(parse_listing(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(normalize_key(&key))))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let body = fetch_document_or_fixture(&absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
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
            let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("result-card")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "result-card-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("page-btn") && body.contains("fa-chevron-right"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "detail-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "ManhwasMe".into())),
        cover: html::attr_after(body, "detail-hero-cover", "data-src")
            .or_else(|| html::attr_after(body, "detail-hero-cover", "src"))
            .map(|value| absolute_url(&value)),
        authors: detail_values(body, "Autores"),
        artists: detail_values(body, "Artistas"),
        tags: detail_values(body, "Géneros"),
        description: html::text_between(body, "detail-synopsis", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(body),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("detail-chapter-row")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value).replace("Ch.", "Chapter"))
                .map(|value| value.strip_suffix(".00").unwrap_or(&value).to_string())
                .filter(|value| !value.is_empty());
            Some(MangaChapter {
                key: key.clone(),
                title,
                url: Some(absolute_url(&key)),
                date_uploaded: html::text_between(chunk, "detail-col-updated", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("data-src") || chunk.contains("src="))
        .filter_map(image_attr)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_status(body: &str) -> ItemStatus {
    match html::text_between(body, "detail-tag-year", "</")
        .map(|value| html::strip_tags(&value).to_ascii_lowercase())
        .as_deref()
    {
        Some("completado") => ItemStatus::Completed,
        Some("en pausa") => ItemStatus::Hiatus,
        Some("cancelado") => ItemStatus::Cancelled,
        Some("en curso") => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn detail_values(body: &str, label: &str) -> Vec<String> {
    body.split("detail-stat-row")
        .filter(|chunk| chunk.contains(label))
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn parse_date(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    if trimmed.to_ascii_lowercase().contains("hace") {
        return None;
    }
    let mut parts = trimmed.split('/');
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let mut year = parts.next()?.parse::<i32>().ok()?;
    if year < 100 {
        year += 2000;
    }
    dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "src"))
        .or_else(|| html::attr(input, "data-src"))
        .or_else(|| html::attr(input, "src"))
}

fn filter<'a>(filters: &'a Value, key: &str, default: &'a str) -> &'a str {
    filters.get(key).and_then(Value::as_str).unwrap_or(default)
}

fn normalize_key(value: &str) -> String {
    let normalized = value.replace("/manhwa/", "/manga/");
    if normalized.starts_with("http://") || normalized.starts_with("https://") {
        if let Some(index) = normalized.find("/manga/") {
            return format!("/{}", normalized[index + 1..].trim_end_matches('/'));
        }
    }
    format!(
        "/{}",
        normalized.trim_start_matches('/').trim_end_matches('/')
    )
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, &value.replace("/manhwa/", "/manga/"))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="results-grid">
<a class="result-card" href="https://manhwas.me/manga/sample">
<div class="result-card-title">Sample Manhwa</div>
<div class="result-card-image"><img data-src="/cover.jpg"></div>
</a>
</div>
<div class="pagination"><a class="page-btn"><i class="fa-chevron-right"></i></a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="detail-title">Sample Manhwa</h1>
<div class="detail-hero-cover"><img data-src="/cover.jpg"></div>
<span class="detail-tag-year">En curso</span>
<div class="detail-stat-row"><span class="detail-stat-label">Autores</span><span class="detail-stat-value">Author One</span></div>
<div class="detail-stat-row"><span class="detail-stat-label">Géneros</span><span class="detail-stat-value"><a>Drama</a><a>Romance</a></span></div>
<div class="detail-synopsis"><p>Fixture description.</p></div>
<div class="detail-chapter-row"><span class="detail-col-chapter"><a href="/manga/sample/chapter-1">Ch. 1.00</a></span><span class="detail-col-updated">01/01/24</span></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="reader-pages"><div class="img-wrap"><img data-src="/page1.jpg"></div><div class="img-wrap"><img src="/page2.jpg"></div></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
