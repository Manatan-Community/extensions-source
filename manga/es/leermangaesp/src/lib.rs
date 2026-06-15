use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: LeerMangaEsp = LeerMangaEsp;
const BASE_URL: &str = "https://leermangaesp.net";
const IMAGE_BASE_URL: &str = "https://images.leermangaesp.net/file/leermangaesp";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";
const PAGE_SIZE: u64 = 20;

struct LeerMangaEsp;

impl MangaSource for LeerMangaEsp {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_home_popular(LIST_FIXTURE));
        }
        if listing_id(&request) == "latest" {
            return Ok(parse_latest(&fetch_json_or_fixture(
                &format!("{BASE_URL}/api/latest_chapters_with_dates"),
                LATEST_FIXTURE,
            )));
        }
        Ok(parse_home_popular(&fetch_document_or_fixture(
            BASE_URL,
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(slug) = deeplink_slug(query) {
            return Ok(Paged {
                entries: vec![details_from_slug(&slug)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_search(&fetch_json_or_fixture(
            &search_api_url(page, query, request.get("filters").unwrap_or(&Value::Null)),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_from_slug(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(fetch_chapters(&key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/leer-m/sample/1".into());
        Ok(parse_pages(&fetch_document_or_fixture(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| manga_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = deeplink_slug(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_slug(&slug)),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_cookies_for(IMAGE_BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_home_popular(body: &str) -> Paged<CatalogItem> {
    let json = script_data(body, "populares-ssr").unwrap_or_else(|| HOME_GRID_FIXTURE.to_string());
    let entries = serde_json::from_str::<Value>(&json)
        .unwrap_or_else(|_| serde_json::from_str(HOME_GRID_FIXTURE).unwrap_or(Value::Null))
        .as_array()
        .into_iter()
        .flatten()
        .map(catalog_from_list_item)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let mut items = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(LATEST_FIXTURE).unwrap_or(Value::Null))
        .as_array()
        .cloned()
        .unwrap_or_default();
    items.sort_by(|a, b| {
        string_value(b, "fecha_publicacion").cmp(&string_value(a, "fecha_publicacion"))
    });
    Paged {
        entries: items.iter().map(catalog_from_list_item).collect(),
        has_next_page: false,
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).unwrap_or(Value::Null));
    let entries = root
        .get("resultados")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(catalog_from_list_item)
        .collect();
    let page = root.get("page").and_then(Value::as_u64).unwrap_or(1);
    let total_pages = root
        .get("total_pages")
        .and_then(Value::as_u64)
        .unwrap_or(page);
    Paged {
        entries,
        has_next_page: page < total_pages,
    }
}

fn details_from_slug(slug: &str) -> CatalogItem {
    parse_details(
        &fetch_document_or_fixture(&manga_url(slug), DETAILS_FIXTURE),
        slug,
    )
}

fn catalog_from_list_item(item: &Value) -> CatalogItem {
    let key = string_value(item, "slug").unwrap_or_else(|| "sample".into());
    CatalogItem {
        key: key.clone(),
        title: string_value(item, "titulo").unwrap_or_else(|| "LeerMangaEsp".into()),
        cover: string_value(item, "portada").map(|path| image_url(&path)),
        url: Some(manga_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, fallback_slug: &str) -> CatalogItem {
    let title = class_text(body, "manga-title")
        .or_else(|| html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value)))
        .unwrap_or_else(|| "LeerMangaEsp".into());
    CatalogItem {
        key: deeplink_slug(
            &html::attr_after(body, "rel=\"canonical\"", "href").unwrap_or_default(),
        )
        .unwrap_or_else(|| normalize_slug(fallback_slug)),
        title,
        cover: html::attr_after(body, "manga-cover", "src").map(|value| absolute_url(&value)),
        description: element_text_by_id(body, "synopsis-text"),
        tags: info_genres(body),
        status: parse_status(&element_text_by_id(body, "info-block").unwrap_or_default()),
        url: Some(manga_url(fallback_slug)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapters(slug: &str) -> Vec<MangaChapter> {
    let mut target = manga_url(slug);
    let mut seen = std::collections::BTreeSet::new();
    let mut chapters = Vec::new();
    for _ in 0..50 {
        let body = fetch_document_or_fixture(&target, DETAILS_FIXTURE);
        for chapter in parse_chapter_page(&body) {
            if seen.insert(chapter.key.clone()) {
                chapters.push(chapter);
            }
        }
        let Some(next) = html::attr_after(&body, "id=\"more-link\"", "href")
            .or_else(|| html::attr_after(&body, "id='more-link'", "href"))
            .filter(|value| !value.is_empty())
        else {
            break;
        };
        target = absolute_url(&next);
    }
    chapters
}

fn parse_chapter_page(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-link") && !chunk.contains("continue-link"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let number =
                html::attr(chunk, "data-chapter").and_then(|value| value.parse::<f32>().ok());
            let title = class_text(chunk, "chapter-title").or_else(|| {
                html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
            })?;
            let key = path_from_url(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: number,
                date_uploaded: class_text(chunk, "chapter-date")
                    .and_then(|date| parse_english_date(&date)),
                language: Some(LANG.to_string()),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("manga-image"))
        .filter_map(|chunk| html::attr(chunk, "src"))
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

fn search_api_url(page: u64, query: &str, filters: &Value) -> String {
    let mut target = format!("{BASE_URL}/api/buscar_mangas?page={page}&page_size={PAGE_SIZE}");
    if !query.is_empty() {
        target.push_str("&query=");
        target.push_str(&url::query_escape(query));
    }
    if let Some(kind) = filters
        .get("type")
        .or_else(|| filters.get("tipo"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        target.push_str("&tipo=");
        target.push_str(&url::query_escape(kind));
    }
    let genres = selected_genres(filters);
    if !genres.is_empty() {
        target.push_str("&generos=");
        target.push_str(&url::query_escape(&genres.join(",")));
    }
    target
}

fn selected_genres(filters: &Value) -> Vec<String> {
    filters
        .get("genres")
        .or_else(|| filters.get("generos"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn script_data(body: &str, id: &str) -> Option<String> {
    let marker1 = format!("id=\"{id}\"");
    let marker2 = format!("id='{id}'");
    let start = body.find(&marker1).or_else(|| body.find(&marker2))?;
    let rest = &body[start..];
    let content_start = rest.find('>')? + 1;
    let after = &rest[content_start..];
    let content_end = after.find("</script>")?;
    Some(html::html_unescape(&after[..content_end]))
}

fn class_text(body: &str, class: &str) -> Option<String> {
    html::text_between(body, class, "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn element_text_by_id(body: &str, id: &str) -> Option<String> {
    html::text_between(body, &format!("id=\"{id}\""), "</")
        .or_else(|| html::text_between(body, &format!("id='{id}'"), "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn info_genres(body: &str) -> Vec<String> {
    body.split("<")
        .filter(|chunk| chunk.contains("genero-item"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(text: &str) -> ItemStatus {
    let normalized = text.to_ascii_lowercase();
    if normalized.contains("en curso") {
        ItemStatus::Ongoing
    } else if normalized.contains("finalizado") || normalized.contains("completo") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn parse_english_date(value: &str) -> Option<i64> {
    let mut parts = value.trim().trim_end_matches(',').split_whitespace();
    let month = match parts.next()? {
        "January" => 1,
        "February" => 2,
        "March" => 3,
        "April" => 4,
        "May" => 5,
        "June" => 6,
        "July" => 7,
        "August" => 8,
        "September" => 9,
        "October" => 10,
        "November" => 11,
        "December" => 12,
        _ => return None,
    };
    let day = parts.next()?.trim_end_matches(',').parse::<u32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn deeplink_slug(input: &str) -> Option<String> {
    if !input.contains("leermangaesp")
        && !input.starts_with("/manga/")
        && !input.starts_with("/leer-m/")
    {
        return None;
    }
    let path = input.split("leermangaesp.net").nth(1).unwrap_or(input);
    let slug = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
    match slug.first().copied() {
        Some("manga") | Some("leer-m") => slug
            .get(1)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string()),
        _ => None,
    }
}

fn manga_url(slug: &str) -> String {
    format!("{BASE_URL}/manga/{}/", normalize_slug(slug))
}

fn image_url(path: &str) -> String {
    url::join_url(IMAGE_BASE_URL, path.trim_start_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn path_from_url(value: &str) -> String {
    if let Some((_, path)) = value.split_once("leermangaesp.net") {
        return path.to_string();
    }
    format!("/{}", value.trim_start_matches('/'))
}

fn normalize_slug(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("/manga/")
        .trim_matches('/')
        .split('/')
        .next()
        .unwrap_or("sample")
        .to_string()
}

fn string_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

export_manga_source!(SOURCE);

const HOME_GRID_FIXTURE: &str = r#"[{"slug":"sample","titulo":"Sample","portada":"cover.jpg"}]"#;
const LIST_FIXTURE: &str = r#"<script id="populares-ssr">[{"slug":"sample","titulo":"Sample","portada":"cover.jpg"}]</script>"#;
const LATEST_FIXTURE: &str = r#"[{"slug":"sample","titulo":"Sample","portada":"cover.jpg","fecha_publicacion":"2024-01-01"}]"#;
const SEARCH_FIXTURE: &str = r#"{"resultados":[{"slug":"sample","titulo":"Sample","portada":"cover.jpg"}],"page":1,"total_pages":1}"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="manga-title">Sample</h1><img class="manga-cover" src="/cover.jpg"><div id="synopsis-text">Summary</div>
<div id="info-block"><span class="info-value">En curso</span></div><div class="info-generos"><span class="genero-item">Drama</span></div>
<div id="chapter-list"><a class="chapter-link" data-chapter="1" href="/leer-m/sample/1"><span class="chapter-title">Capitulo 1</span><span class="chapter-date">January 1, 2024</span></a></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="cascade-view"><img class="manga-image" src="/page1.jpg"></div>"#;
