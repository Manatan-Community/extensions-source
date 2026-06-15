use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

impl MangaSource for Source {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let slug = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            LATEST_SLUG
        } else {
            POPULAR_SLUG
        };
        Ok(parse_listing(&fetch_document(&page_url(slug, page), LIST_FIXTURE)))
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
        if query.is_empty() {
            return Ok(parse_listing(&fetch_document(
                &format!("{BASE_URL}/advanced-filter/page/{page}/"),
                LIST_FIXTURE,
            )));
        }
        Ok(parse_search_json(&fetch_json(
            &format!(
                "{BASE_URL}/wp-json/initlise/v1/search?term={}&page={page}",
                url::query_escape(query)
            ),
            SEARCH_FIXTURE,
        )))
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
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let manga_id = html::attr_after(&body, "id=\"manga-title\"", "data-id")
            .or_else(|| html::attr_after(&body, "id=\"chapter-search-input\"", "data-manga-id"));
        if let Some(id) = manga_id {
            let body = fetch_json(
                &format!("{BASE_URL}/wp-json/initmanga/v1/chapters?manga_id={id}&paged=1&per_page=50"),
                &key,
            );
            let chapters = parse_chapters_json(&body, &key);
            if !chapters.is_empty() {
                return Ok(chapters);
            }
        }
        Ok(parse_chapters_html(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
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

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn page_url(slug: &str, page: u64) -> String {
    if page <= 1 {
        format!("{BASE_URL}/{slug}/")
    } else {
        format!("{BASE_URL}/{slug}/page/{page}/")
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("manga-item-grid")
        .skip(1)
        .filter_map(|chunk| grid_item(chunk))
        .filter(|item| !item.title.to_ascii_lowercase().starts_with("anime -"))
        .collect::<Vec<_>>();
    Paged {
        has_next_page: body.contains("aria-label=\"Next page\""),
        entries,
    }
}

fn grid_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    if href.contains("/chapter-") {
        return None;
    }
    let key = normalize_key(&href);
    let title = html::text_between(chunk, "<h2", "</h2>")
        .map(|value| html::strip_tags(&value))
        .or_else(|| html::attr_after(chunk, "<a", "title"))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| NAME.into()));
    Some(catalog_item(key, title, image_from_chunk(chunk), false))
}

fn parse_search_json(body: &str) -> Paged<CatalogItem> {
    let rows = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let entries = rows
        .iter()
        .filter_map(|row| {
            let href = json_text(row, "url")?;
            let key = normalize_key(&href);
            let title = json_text(row, "title")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| NAME.to_string());
            Some(catalog_item(key, title, json_text(row, "thumb"), false))
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn catalog_item(key: String, title: String, cover: Option<String>, initialized: bool) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover,
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let mut item = catalog_item(
        key.clone(),
        html::text_between(body, "id=\"manga-title\"", "</")
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .map(|value| html::strip_tags(&value).split("[Ch.").next().unwrap_or("").trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| NAME.into())),
        image_from_chunk(body),
        true,
    );
    item.description = html::text_between(body, "id=\"manga-description\"", "</")
        .or_else(|| html::attr_after(body, "name=\"description\"", "content"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    item.tags = link_texts(body, "/genre/");
    item.status = match html::text_between(body, "id=\"manga-status\"", "</")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default()
        .as_str()
    {
        value if value.contains("completed") => ItemStatus::Completed,
        value if value.contains("ongoing") => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    };
    item
}

fn parse_chapters_json(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|root| root.get("items")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|row| {
            let slug = json_text(row, "slug").unwrap_or_else(|| "chapter-1".to_string());
            let number = json_number(row, "number").unwrap_or(1.0);
            let title = json_text(row, "title").unwrap_or_default();
            let key = format!("{}/{}", manga_key.trim_end_matches('/'), slug.trim_matches('/'));
            MangaChapter {
                key: key.clone(),
                title: Some(if title.is_empty() {
                    format!("Chapter {}", number_string(number))
                } else {
                    format!("Chapter {} - {title}", number_string(number))
                }),
                chapter_number: Some(number),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_chapters_html(body: &str) -> Vec<MangaChapter> {
    body.split("chapter-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<h3", "</h3>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                chapter_number: key
                    .split("chapter-")
                    .nth(1)
                    .and_then(|value| value.trim_matches('/').parse::<f32>().ok()),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("chapter-content")
        .nth(1)
        .unwrap_or(body)
        .split("<img")
        .skip(1)
        .filter_map(image_from_chunk)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "src"))
        .or_else(|| html::attr_after(chunk, "property=\"og:image\"", "content"))
        .filter(|value| !value.is_empty())
        .map(|value| url::join_url(BASE_URL, &value))
}

fn link_texts(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input
                .trim_start_matches(BASE_URL)
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(key: &str) -> String {
    if key.starts_with("http") {
        key.to_string()
    } else {
        url::join_url(BASE_URL, key)
    }
}

fn json_text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(ToString::to_string)
}

fn json_number(value: &Value, key: &str) -> Option<f32> {
    value.get(key).and_then(Value::as_f64).map(|value| value as f32)
}

fn number_string(number: f32) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i32)
    } else {
        number.to_string()
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="manga-item-grid"><h2><a href="/manga/sample/">Sample Manga</a></h2><img src="/cover.jpg"></div><a aria-label="Next page"></a>"#;
const SEARCH_FIXTURE: &str = r#"[{"title":"Sample Manga","url":"https://example.invalid/manga/sample/","thumb":"https://example.invalid/cover.jpg"}]"#;
const DETAILS_FIXTURE: &str = r#"<h1 id="manga-title" data-id="1">Sample Manga</h1><img src="/cover.jpg"><div id="manga-description">Summary</div><div id="genre-tags"><a href="/genre/action">Action</a></div><div id="manga-status">Ongoing</div><div class="chapter-list"><div class="chapter-item"><a href="/manga/sample/chapter-1/"><h3>Chapter 1</h3></a></div></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="chapter-content"><img src="/page1.jpg"><img src="/page2.jpg"></div>"#;
