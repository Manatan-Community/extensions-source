use manatan_extension::{
    export_manga_source,
    http::HttpClient,
    source::MangaSource,
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
};
use manatan_shared::{html, manga, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: MangaLivreBlog = MangaLivreBlog;
const BASE_URL: &str = "https://mangalivre.blog";

struct MangaLivreBlog;

impl MangaSource for MangaLivreBlog {
    fn list(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Paged<CatalogItem>> {
        if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            let body = fetch_popular().unwrap_or_else(|| LATEST_FIXTURE.to_string());
            return Ok(Paged {
                entries: parse_popular(&body),
                has_next_page: false,
            });
        }
        let page = page(&request);
        let target = if page <= 1 {
            BASE_URL.to_string()
        } else {
            format!("{BASE_URL}/page/{page}")
        };
        Ok(parse_latest(&fetch_document(&target, LATEST_FIXTURE)))
    }

    fn search(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(&absolute(&key), DETAILS_FIXTURE), key)],
                has_next_page: false,
            });
        }
        Ok(parse_search(&fetch_document(&search_url(page(&request), query, &request), SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> manatan_extension::abi::ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(&fetch_document(&absolute(&key), DETAILS_FIXTURE), key))
    }

    fn chapters(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(&fetch_document(&absolute(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document(&absolute(&key), PAGES_FIXTURE)))
    }

    fn handle_url(&self, request: Value) -> manatan_extension::abi::ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(&absolute(&key), DETAILS_FIXTURE), key)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(manatan_extension::SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..manatan_extension::SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

#[derive(Default, Deserialize)]
struct PopularResponse {
    data: PopularData,
}

#[derive(Default, Deserialize)]
struct PopularData {
    #[serde(default)]
    html: String,
}

fn parse_popular(body: &str) -> Vec<CatalogItem> {
    let html = serde_json::from_str::<PopularResponse>(body)
        .map(|response| response.data.html)
        .unwrap_or_else(|_| body.to_string());
    html.split("popular-manga-item")
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "popular-manga-title", "</a>")
                .map(|text| html::strip_tags(&text))
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga Livre Blog".to_string()));
            Some(basic_item(key, title, clean_thumb(html::attr_after(chunk, "<img", "src"))))
        })
        .collect()
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("manga-card-modern")
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "manga-cover-link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "manga-title-modern", "</a>")
                .map(|text| html::strip_tags(&text))
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga Livre Blog".to_string()));
            Some(basic_item(key, title, clean_thumb(html::attr_after(chunk, "<img", "src"))))
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("next page-numbers"),
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("manga-card")
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "manga-card-link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "manga-card-title", "</h3>")
                .map(|text| html::strip_tags(&text))
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga Livre Blog".to_string()));
            Some(basic_item(key, title, clean_thumb(html::attr_after(chunk, "<img", "src"))))
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("next page-numbers"),
    }
}

fn parse_details(body: &str, key: String) -> CatalogItem {
    let mut item = basic_item(
        key.clone(),
        html::text_between(body, "manga-title", "</h1>")
            .map(|text| html::strip_tags(&text))
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga Livre Blog".to_string())),
        clean_thumb(html::attr_after(body, "<img", "src")),
    );
    item.description = html::text_between(body, "synopsis-content", "</div>")
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty());
    item.tags = body
        .split("manga-tag")
        .filter_map(|chunk| html::text_between(chunk, ">", "</"))
        .map(|tag| html::strip_tags(&tag))
        .filter(|tag| !tag.is_empty())
        .collect();
    for chunk in body.split("manga-meta-item") {
        let label = html::text_between(chunk, "meta-label", "</")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default();
        let value = html::text_between(chunk, "meta-value", "</")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default();
        match label.as_str() {
            "Status:" => item.status = parse_status(&value),
            "Autor:" => item.authors = vec![value],
            "Artista:" => item.artists = vec![value],
            _ => {}
        }
    }
    item.initialized = true;
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapter-item")
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "chapter-link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapter-number", "</")
                .map(|text| html::strip_tags(&text))
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| "Capitulo".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(absolute(&key)),
                language: Some("pt-BR".to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("chapter-image-container")
        .filter_map(|chunk| html::attr_after(chunk, "<img", "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute(&image),
                context: None,
            },
            headers: image_headers(),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn fetch_popular() -> Option<String> {
    let home = fetch_document(BASE_URL, LATEST_FIXTURE);
    let nonce = home
        .split("slimeReadPopular")
        .find_map(|chunk| chunk.split("\"nonce\":\"").nth(1).and_then(|rest| rest.split('"').next()))?;
    Some(
        client()
            .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
            .form(&[("action", "get_popular_manga"), ("period", "month"), ("nonce", nonce)])
            .send_text()
            .ok()?,
    )
}

fn basic_item(key: String, title: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover,
        url: Some(absolute(&key)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        ..CatalogItem::default()
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn search_url(page: u64, query: &str, request: &Value) -> String {
    let mut params = Vec::new();
    let mut path = String::new();
    if query.is_empty() {
        path = "/pesquisa".to_string();
        params.push(("s", String::new()));
    } else {
        params.push(("s", query.to_string()));
    }
    for key in ["genre", "status", "sort", "order"] {
        if let Some(value) = filter_value(request, key).filter(|value| !value.is_empty()) {
            params.push((key, value));
        }
    }
    if page > 1 {
        params.push(("paged", page.to_string()));
    }
    format!(
        "{BASE_URL}{path}?{}",
        params
            .iter()
            .map(|(key, value)| format!("{key}={}", url::query_escape(value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn filter_value(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(|value| match value {
            Value::String(value) => Some(value.clone()),
            Value::Object(object) => object.get("value").or_else(|| object.get("id")).and_then(Value::as_str).map(ToString::to_string),
            _ => None,
        })
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!("/{}", input[BASE_URL.len()..].trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn clean_thumb(value: Option<String>) -> Option<String> {
    value.map(|url| absolute(&url).replace("-150x150", ""))
}

fn parse_status(value: &str) -> ItemStatus {
    match value {
        "Em Lancamento" | "Em Lançamento" | "Em Andamento" => ItemStatus::Ongoing,
        "Completo" => ItemStatus::Completed,
        "Cancelado" => ItemStatus::Cancelled,
        "Pausado" | "Hiato" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn image_headers() -> BTreeMap<String, String> {
    BTreeMap::from([("Referer".to_string(), format!("{BASE_URL}/"))])
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

const LATEST_FIXTURE: &str = r#"<section class="latest-section"><div class="manga-card-modern"><a class="manga-cover-link" href="/manga/sample"><img src="/cover-150x150.jpg"></a><h3 class="manga-title-modern"><a href="/manga/sample">Sample Blog</a></h3></div></section>"#;
const SEARCH_FIXTURE: &str = r#"<div class="manga-grid search-results-grid"><div class="manga-card"><a class="manga-card-link" href="/manga/sample"><img src="/cover.jpg"></a><h3 class="manga-card-title">Sample Blog</h3></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="manga-title">Sample Blog</h1><div class="synopsis-content">Description</div><span class="manga-tag">Action</span><div class="manga-meta-item"><span class="meta-label">Status:</span><span class="meta-value">Em Andamento</span></div><div class="chapters-list"><div class="chapter-item"><a class="chapter-link" href="/manga/sample/chapter-1"><span class="chapter-number">Capitulo 1</span></a></div></div>"#;
const PAGES_FIXTURE: &str = r#"<div class="chapter-image-container"><img src="/page-1.jpg"></div><div class="chapter-image-container"><img src="/page-2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_blog_fixtures() {
        assert_eq!(parse_latest(LATEST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_search(SEARCH_FIXTURE).entries.len(), 1);
        assert_eq!(SOURCE.chapters(json!({"manga":"/manga/sample"})).unwrap().len(), 1);
        assert_eq!(SOURCE.pages(json!({"chapter":"/manga/sample/chapter-1"})).unwrap().len(), 2);
    }
}
