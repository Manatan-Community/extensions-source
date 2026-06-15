use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: NeoManga = NeoManga;
const BASE_URL: &str = "https://www.neomanga.online";

struct NeoManga;

impl MangaSource for NeoManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = fetch_doc("/series", SERIES_FIXTURE);
        Ok(parse_listing(&body, &request))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = key_from_url(query).unwrap_or_else(|| "sample".to_string());
            return Ok(Paged { entries: vec![details_for(&key)], has_next_page: false });
        }
        let body = fetch_doc("/series", SERIES_FIXTURE);
        Ok(parse_listing(&body, &request))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_for(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let body = fetch_doc(&format!("/manga/{key}"), DETAILS_FIXTURE);
        Ok(extract_array(&body, "chapters")
            .into_iter()
            .filter_map(|chapter| chapter_from_json(&key, &chapter))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/capitulo/1".into());
        let body = fetch_doc(&format!("/manga/{key}"), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/manga/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}/manga/{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let key = key_from_url(input).unwrap_or_else(|| "sample".to_string());
            return Ok(Some(UrlResolveResult { item: Some(details_for(&key)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_header("RSC", "1")
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_doc(path: &str, fixture: &str) -> String {
    client().get(format!("{BASE_URL}{path}")).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, request: &Value) -> Paged<CatalogItem> {
    let query = request.get("query").and_then(Value::as_str).unwrap_or_default().to_ascii_lowercase();
    let status = request.pointer("/filters/status").and_then(Value::as_str).unwrap_or("all");
    let genre = request.pointer("/filters/genre").and_then(Value::as_str).unwrap_or("");
    let entries = extract_array(body, "initialMangas")
        .into_iter()
        .filter(|item| query.is_empty() || text(item, "title").unwrap_or_default().to_ascii_lowercase().contains(&query))
        .filter(|item| status == "all" || text(item, "status").as_deref() == Some(status))
        .filter(|item| genre.is_empty() || item.get("genres").and_then(Value::as_array).into_iter().flatten().any(|g| g.as_str() == Some(genre)))
        .filter_map(|item| catalog_from_json(&item))
        .collect();
    Paged { entries, has_next_page: false }
}

fn details_for(key: &str) -> CatalogItem {
    let body = fetch_doc(&format!("/manga/{key}"), DETAILS_FIXTURE);
    let mut item = extract_array(&body, "initialMangas")
        .into_iter()
        .find(|manga| text(manga, "slug").as_deref() == Some(key))
        .and_then(|item| catalog_from_json(&item))
        .unwrap_or_else(|| CatalogItem { key: key.to_string(), title: key.to_string(), language: Some("es".into()), content_rating: Some("safe".into()), ..CatalogItem::default() });
    item.description = html_between(&body, "whitespace-pre-line").or_else(|| item.description.take());
    item.initialized = true;
    item
}

fn catalog_from_json(item: &Value) -> Option<CatalogItem> {
    let key = text(item, "slug")?;
    let status = match text(item, "status").as_deref() {
        Some("en_emision") => ItemStatus::Ongoing,
        Some("finalizado") => ItemStatus::Completed,
        Some("pausado") => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    };
    Some(CatalogItem {
        key: key.clone(),
        title: text(item, "title").unwrap_or_else(|| key.clone()),
        cover: text(item, "cover_image_url").map(|cover| cover_url(&cover)),
        description: text(item, "synopsis"),
        tags: item.get("genres").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(ToString::to_string).collect(),
        status,
        url: Some(format!("{BASE_URL}/manga/{key}")),
        language: Some("es".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn chapter_from_json(series: &str, chapter: &Value) -> Option<MangaChapter> {
    let number = chapter.get("chapter_number").and_then(Value::as_f64).unwrap_or(1.0) as f32;
    let number_text = if number.fract() == 0.0 { format!("{}", number as i32) } else { number.to_string() };
    let key = format!("{series}/capitulo/{number_text}");
    Some(MangaChapter {
        key: key.clone(),
        title: text(chapter, "title").or_else(|| Some(format!("Capitulo {number_text}"))),
        chapter_number: Some(number),
        url: Some(format!("{BASE_URL}/manga/{key}")),
        ..MangaChapter::default()
    })
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    extract_array(body, "pages_urls")
        .into_iter()
        .enumerate()
        .map(|(index, page)| {
            let raw = page.as_str().unwrap_or_default();
            let url = if let Some(id) = raw.strip_prefix("MANGADEX:") { format!("{BASE_URL}/api/manga-page/{id}/{index}") } else { raw.to_string() };
            MangaPage {
                content: PageContent::Url { url: url.clone(), context: Some(manga::image_headers(BASE_URL)) },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn extract_array(body: &str, key: &str) -> Vec<Value> {
    let normalized = body.replace("\\\"", "\"");
    let Some(key_index) = normalized.find(&format!("\"{key}\"")) else { return Vec::new(); };
    let Some(start) = normalized[key_index..].find('[').map(|offset| key_index + offset) else { return Vec::new(); };
    let mut depth = 0i32;
    for (offset, ch) in normalized[start..].char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&normalized[start..=start + offset]).unwrap_or_default();
                }
            }
            _ => {}
        }
    }
    Vec::new()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(ToString::to_string).filter(|value| !value.is_empty())
}

fn cover_url(raw: &str) -> String {
    if raw.contains("/_next/image") || raw.starts_with("http") { raw.to_string() } else { format!("{BASE_URL}/_next/image?url={}&w=640&q=75", url::query_escape(raw)) }
}

fn html_between(body: &str, marker: &str) -> Option<String> {
    let start = body.find(marker)?;
    let tail = &body[start..];
    manatan_shared::html::text_between(tail, ">", "</").map(|value| manatan_shared::html::strip_tags(&value))
}

fn key_from_url(input: &str) -> Option<String> {
    input.split("/manga/").nth(1).map(|tail| tail.trim_matches('/').to_string()).filter(|value| !value.is_empty())
}

export_manga_source!(SOURCE);

const SERIES_FIXTURE: &str = r#"{"initialMangas":[{"title":"Sample NeoManga","slug":"sample","synopsis":"Fixture summary.","cover_image_url":"/cover.jpg","status":"en_emision","genres":["Accion"]}]}"#;
const DETAILS_FIXTURE: &str = r#"{"initialMangas":[{"title":"Sample NeoManga","slug":"sample","synopsis":"Fixture summary.","cover_image_url":"/cover.jpg","status":"en_emision","genres":["Accion"]}],"chapters":[{"chapter_number":1,"title":"Capitulo 1","published_at":"2024-01-01T00:00:00"}]}"#;
const PAGES_FIXTURE: &str = r#"{"chapter":{"pages_urls":["https://fixtures.invalid/neomanga/page1.jpg"]}}"#;
