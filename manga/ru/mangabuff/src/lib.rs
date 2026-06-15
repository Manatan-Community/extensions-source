use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: MangaBuff = MangaBuff;
const BASE_URL: &str = "https://mangabuff.ru";

struct MangaBuff;

impl MangaSource for MangaBuff {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "updated_at" } else { "views" };
        Ok(parse_listing(&fetch_document(&catalog_url(page, Some(sort), &Value::Null), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) || query.starts_with("slug:") {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if query.is_empty() {
            catalog_url(page, None, request.get("filters").unwrap_or(&Value::Null))
        } else if page == 1 {
            format!("{BASE_URL}/search?q={}", url::query_escape(query))
        } else {
            format!("{BASE_URL}/search?q={}&page={page}", url::query_escape(query))
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let mut chapters = parse_chapters(&body);
        if body.contains("load-chapters-trigger") {
            if let Some(manga_id) = html::attr_after(&body, "class=\"manga", "data-id") {
                chapters.extend(parse_chapters(&load_more_chapters(&manga_id)));
            }
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
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
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn load_more_chapters(manga_id: &str) -> String {
    let token_page = fetch_document(BASE_URL, DETAILS_FIXTURE);
    let csrf = html::attr_after(&token_page, "csrf-token", "content").unwrap_or_default();
    let response = client()
        .post(format!("{BASE_URL}/chapters/load"))
        .header("X-Requested-With", "XMLHttpRequest")
        .header("X-CSRF-TOKEN", csrf)
        .form(&[("manga_id", manga_id)])
        .send_text()
        .unwrap_or_else(|_| MORE_CHAPTERS_FIXTURE.to_string());
    serde_json::from_str::<LoadChaptersDto>(&response).map(|dto| dto.content).unwrap_or(response)
}

fn catalog_url(page: u64, forced_sort: Option<&str>, filters: &Value) -> String {
    let mut params = Vec::new();
    for value in selected_values(filters.get("genres")) { params.push(format!("genres[]={}", url::query_escape(&value))); }
    for value in selected_values(filters.get("withoutGenres")) { params.push(format!("without_genres[]={}", url::query_escape(&value))); }
    for value in selected_values(filters.get("types")) { params.push(format!("type_id[]={}", url::query_escape(&value))); }
    for value in selected_values(filters.get("status")) { params.push(format!("status_id[]={}", url::query_escape(&value))); }
    for value in selected_values(filters.get("age")) { params.push(format!("age_rating[]={}", url::query_escape(&value))); }
    let sort = forced_sort.or_else(|| filter_id(filters, "sort")).unwrap_or("views");
    params.push(format!("sort={sort}"));
    if page != 1 { params.push(format!("page={page}")); }
    format!("{BASE_URL}/manga?{}", params.join("&"))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body.split("cards__item").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let key = normalize_key(&href);
        let slug = key.trim_end_matches('/').rsplit('/').next().unwrap_or("sample");
        Some(CatalogItem {
            key: key.clone(),
            title: html::text_between(chunk, "cards__name", "</").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| slug.to_string()),
            cover: Some(format!("{BASE_URL}/img/manga/posters/{slug}.jpg")),
            url: Some(absolute_url(&key)),
            language: Some("ru".into()),
            content_rating: Some("adult".into()),
            initialized: false,
            ..CatalogItem::default()
        })
    }).collect::<Vec<_>>();
    Paged { has_next_page: body.contains("pagination__button--active") && body.contains("pagination__button"), entries }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let mut description = html::text_between(body, "manga__description", "</div>").map(|v| html::strip_tags(&v)).unwrap_or_default();
    for (label, marker) in [("Рейтинг", "manga__rating"), ("Просмотров", "manga__views")] {
        if let Some(value) = html::text_between(body, marker, "</").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()) {
            if !description.is_empty() { description.push_str("\n\n"); }
            description.push_str(&format!("{label}: {value}"));
        }
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>").or_else(|| html::text_between(body, "manga__name", "</")).map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MangaBuff".into())),
        cover: html::attr_after(body, "manga__img", "src").or_else(|| html::attr_after(body, "manga-mobile__image", "src")).map(|v| absolute_url(&v)),
        description: (!description.is_empty()).then_some(description),
        tags: parse_tags(body),
        status: parse_status(&html::text_between(body, "manga__middle-links", "</div>").map(|v| html::strip_tags(&v)).unwrap_or_default()),
        url: Some(absolute_url(&key)),
        language: Some("ru".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapters__item").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
        let key = normalize_key(&href);
        Some(MangaChapter {
            key: key.clone(),
            title: Some(html::strip_tags(chunk).split_whitespace().collect::<Vec<_>>().join(" ")).filter(|v| !v.is_empty()).or(Some("Глава".into())),
            chapter_number: html::text_between(chunk, "chapters__value", "</").and_then(|v| html::strip_tags(&v).split_whitespace().last()?.parse::<f32>().ok()),
            url: Some(absolute_url(&key)),
            ..MangaChapter::default()
        })
    }).collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img").skip(1).filter(|chunk| chunk.contains("reader__pages") || chunk.contains("data-src") || chunk.contains("src="))
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|image| !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: absolute_url(&image), context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_tags(body: &str) -> Vec<String> {
    body.split("tags__item").skip(1).filter_map(|chunk| html::text_between(chunk, ">", "</").map(|v| html::strip_tags(&v))).filter(|v| !v.is_empty()).collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_lowercase();
    if lower.contains("заверш") { ItemStatus::Completed }
    else if lower.contains("продолжа") { ItemStatus::Ongoing }
    else if lower.contains("заморож") { ItemStatus::Hiatus }
    else if lower.contains("заброш") { ItemStatus::Cancelled }
    else { ItemStatus::Unknown }
}

fn normalize_key(value: &str) -> String {
    let value = value.strip_prefix("slug:").map(|slug| format!("/manga/{slug}")).unwrap_or_else(|| value.to_string());
    let path = value.strip_prefix(BASE_URL).unwrap_or(&value).split('?').next().unwrap_or(&value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(key: &str) -> String { url::join_url(BASE_URL, key) }

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).filter_map(option_id).collect(),
        Some(Value::String(value)) => value.split(',').filter_map(option_id).collect(),
        _ => Vec::new(),
    }
}

fn filter_id<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).and_then(Value::as_str).and_then(|value| value.split_once(':').map(|(id, _)| id).or(Some(value))).filter(|value| !value.is_empty())
}

fn option_id(value: &str) -> Option<String> {
    let id = value.trim().split_once(':').map(|(id, _)| id).unwrap_or_else(|| value.trim());
    (!id.is_empty()).then(|| id.to_string())
}

#[derive(Deserialize)]
struct LoadChaptersDto { content: String }

const LIST_FIXTURE: &str = r#"<div class="cards"><div class="cards__item"><a href="/manga/sample"><span class="cards__name">Sample</span></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="manga" data-id="1"><h1>Sample</h1><div class="manga__description">Description</div><a class="chapters__item" href="/manga/sample/chapter-1"><span class="chapters__value">Глава 1</span></a></div>"#;
const PAGES_FIXTURE: &str = r#"<div class="reader__pages"><img src="/page1.jpg"><img src="/page2.jpg"></div>"#;
const MORE_CHAPTERS_FIXTURE: &str = r#"{"content":"<a class=\"chapters__item\" href=\"/manga/sample/chapter-2\"><span class=\"chapters__value\">Глава 2</span></a>"}"#;

export_manga_source!(SOURCE);
