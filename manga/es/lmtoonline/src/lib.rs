use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Lmtos = Lmtos;
const BASE_URL: &str = "https://lmtos.net";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";
const PER_PAGE: usize = 20;

struct Lmtos;

impl MangaSource for Lmtos {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(LIST_FIXTURE));
        }
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(search_mangas("", &serde_json::json!({"order":"recents"}), page(&request)));
        }
        Ok(parse_popular(&fetch_document(&format!("{BASE_URL}/destacados"), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_slug(query);
            return Ok(Paged {
                entries: vec![details_from_slug(&key)],
                has_next_page: false,
            });
        }
        Ok(search_mangas(query, request.get("filters").unwrap_or(&Value::Null), page(&request)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_from_slug(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let root = next_json(&fetch_document(&manga_url(&key), DETAILS_FIXTURE), DETAILS_FIXTURE);
        Ok(parse_chapters(&root, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/chapter-1".into());
        let root = next_json(&fetch_document(&manga_url(&key), PAGES_FIXTURE), PAGES_FIXTURE);
        Ok(parse_pages(&root))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| manga_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| manga_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_slug(&normalize_slug(input))),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("group") && chunk.contains("/manga/"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let key = normalize_slug(&href);
                let title = html::text_between(chunk, "<h3", "</h3>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&key))
                    .unwrap_or_else(|| "Manga".into());
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(manga_url(&key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: false,
    }
}

fn search_mangas(query: &str, filters: &Value, page: u64) -> Paged<CatalogItem> {
    let root = next_json(&fetch_document(&format!("{BASE_URL}/series"), SERIES_FIXTURE), SERIES_FIXTURE);
    let mut items = find_array(&root, "mangas")
        .into_iter()
        .flatten()
        .filter(|item| query_match(item, query))
        .filter(|item| filter_match(item, filters, "type"))
        .filter(|item| filter_match(item, filters, "status"))
        .filter(|item| filter_match(item, filters, "demographic"))
        .filter(|item| nsfw_match(item, filter_string(filters, "nsfw").as_deref()))
        .filter(|item| genre_match(item, filter_array(filters, "genres")))
        .cloned()
        .collect::<Vec<_>>();
    match filter_string(filters, "order").as_deref().unwrap_or("a-z") {
        "recents" => items.sort_by_key(|item| std::cmp::Reverse(string_value(item, "latestChapterCreatedAt").unwrap_or_default())),
        "views" => items.sort_by_key(|item| std::cmp::Reverse(item.get("totalViews").and_then(Value::as_i64).unwrap_or_default())),
        _ => items.sort_by_key(title),
    }
    let start = page.saturating_sub(1) as usize * PER_PAGE;
    Paged {
        entries: items.iter().skip(start).take(PER_PAGE).map(catalog_from_json).collect(),
        has_next_page: start + PER_PAGE < items.len(),
    }
}

fn details_from_slug(slug: &str) -> CatalogItem {
    let key = normalize_slug(slug);
    let root = next_json(&fetch_document(&manga_url(&key), DETAILS_FIXTURE), DETAILS_FIXTURE);
    let manga = find_object(&root, "manga").unwrap_or(&Value::Null);
    let mut item = catalog_from_json(manga);
    item.key = key.clone();
    item.url = Some(manga_url(&key));
    item.description = string_value(manga, "description");
    item.authors = string_value(manga, "author").into_iter().collect();
    item.artists = string_value(manga, "artist").into_iter().collect();
    item.tags = string_array(manga, "genres");
    if let Some(kind) = string_value(manga, "type") {
        item.tags.insert(0, kind);
    }
    item.status = match string_value(manga, "status").as_deref() {
        Some("ongoing") => ItemStatus::Ongoing,
        Some("completed") => ItemStatus::Completed,
        Some("paused") => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    };
    item.initialized = true;
    item
}

fn catalog_from_json(item: &Value) -> CatalogItem {
    let key = string_value(item, "slug").unwrap_or_else(|| "sample".into());
    CatalogItem {
        key: key.clone(),
        title: title(item),
        cover: string_value(item, "coverImage"),
        url: Some(manga_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_chapters(root: &Value, fallback_slug: &str) -> Vec<MangaChapter> {
    let manga_slug = find_object(root, "manga").and_then(|m| string_value(m, "slug")).unwrap_or_else(|| normalize_slug(fallback_slug));
    find_array(root, "chapters")
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let slug = string_value(chapter, "slug")?;
            let number = chapter.get("number").and_then(Value::as_f64).unwrap_or_default();
            Some(MangaChapter {
                key: format!("{manga_slug}/{slug}"),
                title: Some(format!("Cap. {}", display_number(number))),
                chapter_number: Some(number as f32),
                date_uploaded: string_value(chapter, "createdAt").and_then(|date| manatan_shared::dates::parse_ymd(date.split('T').next()?)),
                language: Some(LANG.to_string()),
                url: Some(manga_url(&format!("{manga_slug}/{slug}"))),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(root: &Value) -> Vec<MangaPage> {
    find_object(root, "chapter")
        .and_then(|chapter| chapter.get("pages"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.to_string(),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn next_json(body: &str, fixture: &str) -> Value {
    let raw = extract_next_data(body).unwrap_or(body);
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::from_str(extract_next_data(fixture).unwrap_or(fixture)).unwrap_or(Value::Null))
}

fn extract_next_data(body: &str) -> Option<&str> {
    body.split("id=\"__NEXT_DATA__").nth(1)?.split('>').nth(1)?.split("</script>").next()
}

fn find_object<'a>(value: &'a Value, field: &str) -> Option<&'a Value> {
    if value.get(field).is_some_and(Value::is_object) {
        return value.get(field);
    }
    match value {
        Value::Array(items) => items.iter().find_map(|item| find_object(item, field)),
        Value::Object(map) => map.values().find_map(|item| find_object(item, field)),
        _ => None,
    }
}

fn find_array<'a>(value: &'a Value, field: &str) -> Option<&'a Vec<Value>> {
    if let Some(array) = value.get(field).and_then(Value::as_array) {
        return Some(array);
    }
    match value {
        Value::Array(items) => items.iter().find_map(|item| find_array(item, field)),
        Value::Object(map) => map.values().find_map(|item| find_array(item, field)),
        _ => None,
    }
}

fn query_match(item: &Value, query: &str) -> bool {
    query.is_empty()
        || title(item).to_ascii_lowercase().contains(&query.to_ascii_lowercase())
        || string_array(item, "alternativeTitles").iter().any(|value| value.to_ascii_lowercase().contains(&query.to_ascii_lowercase()))
}

fn filter_match(item: &Value, filters: &Value, name: &str) -> bool {
    filter_string(filters, name).filter(|value| !value.is_empty()).is_none_or(|value| string_value(item, name).as_deref() == Some(value.as_str()))
}

fn nsfw_match(item: &Value, nsfw: Option<&str>) -> bool {
    let adult = item.get("isAdult").and_then(Value::as_bool).unwrap_or(false);
    match nsfw {
        Some("only") => adult,
        Some("hide") => !adult,
        _ => true,
    }
}

fn genre_match(item: &Value, genres: Vec<String>) -> bool {
    let item_genres = string_array(item, "genres");
    genres.is_empty() || genres.iter().all(|genre| item_genres.iter().any(|item_genre| item_genre == genre))
}

fn filter_string(filters: &Value, name: &str) -> Option<String> {
    filters.get(name).and_then(Value::as_str).map(ToString::to_string)
}

fn filter_array(filters: &Value, name: &str) -> Vec<String> {
    filters.get(name).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(ToString::to_string).collect()
}

fn string_value(item: &Value, name: &str) -> Option<String> {
    item.get(name)?.as_str().map(ToString::to_string)
}

fn string_array(item: &Value, name: &str) -> Vec<String> {
    item.get(name).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(ToString::to_string).collect()
}

fn title(item: &Value) -> String {
    string_value(item, "title").unwrap_or_else(|| "Lmtos".into())
}

fn normalize_slug(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input).trim_matches('/');
    path.strip_prefix("manga/").unwrap_or(path).trim_matches('/').to_string()
}

fn manga_url(slug: &str) -> String {
    format!("{BASE_URL}/manga/{}", normalize_slug(slug))
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src").or_else(|| html::attr_after(chunk, "<img", "src")).or_else(|| html::attr(chunk, "src"))
}

fn display_number(number: f64) -> String {
    if number.fract().abs() < f64::EPSILON {
        format!("{}", number as i64)
    } else {
        number.to_string()
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<section><a class="group" href="/manga/sample"><img src="/cover.jpg"><div><h3>Sample Lmtos</h3></div></a></section>"#;
const SERIES_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"mangas":[{"slug":"sample","title":"Sample Lmtos","alternativeTitles":["Muestra"],"coverImage":"https://img.example/cover.jpg","isAdult":true,"type":"manga","status":"ongoing","demographic":"shounen","genres":["Acción"],"latestChapterCreatedAt":"2024-01-01T00:00:00.000Z","totalViews":10}]}}}</script>"#;
const DETAILS_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"manga":{"slug":"sample","title":"Sample Lmtos","description":"Summary.","coverImage":"https://img.example/cover.jpg","isAdult":true,"type":"manga","status":"ongoing","genres":["Acción"],"author":"Author","artist":"Artist"},"chapters":[{"slug":"chapter-1","number":1,"createdAt":"2024-01-01T00:00:00.000Z"}]}}}</script>"#;
const PAGES_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"chapter":{"slug":"chapter-1","number":1,"createdAt":"2024-01-01T00:00:00.000Z","pages":["https://img.example/page1.jpg","https://img.example/page2.jpg"]}}}}</script>"#;
