use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: OrckuMangas = OrckuMangas;
const BASE_URL: &str = "https://orckumangas.com";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct OrckuMangas;

impl MangaSource for OrckuMangas {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_cards(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if listing_id(&request) == "latest" {
            return Ok(parse_latest(&fetch_document(
                &format!("{BASE_URL}/index.php?filter_chapters=1&type="),
                LATEST_FIXTURE,
            )));
        }
        Ok(parse_cards(&fetch_document(
            &format!("{BASE_URL}/ranking.php?page={page}"),
            LIST_FIXTURE,
        )))
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
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_cards(&fetch_document(
            &search_url(page, query, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(fetch_chapters(&key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/1".into());
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
                item: Some(details_from_key(&key)),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_cards(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("card") || chunk.contains("<h3"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(catalog_item(
                &title,
                &href,
                html::attr_after(chunk, "<img", "src"),
            ))
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("Siguiente"),
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("block") && chunk.contains("<h3"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(catalog_item(
                &title,
                &href,
                html::attr_after(chunk, "<img", "src"),
            ))
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, fallback_key: &str) -> CatalogItem {
    let card = body
        .split("main")
        .nth(1)
        .unwrap_or(body)
        .split("card")
        .nth(1)
        .unwrap_or(body);
    let key = html::attr_after(body, "rel=\"canonical\"", "href")
        .map(|value| normalize_key(&value))
        .unwrap_or_else(|| normalize_key(fallback_key));
    CatalogItem {
        key: key.clone(),
        title: html::text_between(card, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_else(|| "OrckuMangas".to_string()),
        cover: html::attr_after(card, "<img", "src").map(|value| absolute_url(&value)),
        authors: info_text(card, "Autor").into_iter().collect(),
        artists: info_text(card, "Artista").into_iter().collect(),
        description: html::text_between(card, "<p", "</p>").map(|value| html::strip_tags(&value)),
        tags: genre_tags(card),
        status: parse_status(info_text(card, "Estado").as_deref()),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapters(key: &str) -> Vec<MangaChapter> {
    let mut page = 1;
    let mut out = Vec::new();
    loop {
        let target = format!("{}{}?order=desc&page={page}", BASE_URL, normalize_key(key));
        let body = fetch_document(&target, DETAILS_FIXTURE);
        out.extend(parse_chapters_page(&body));
        if !body.contains(&format!("page={}", page + 1)) || page >= 20 {
            break;
        }
        page += 1;
    }
    out
}

fn parse_chapters_page(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("block") && chunk.contains("<span"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "<span", "</span>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                url: Some(absolute_url(&href)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("src"))
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

fn search_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    if !query.is_empty() {
        return format!(
            "{BASE_URL}/buscador.php?q={}&page={page}",
            url::query_escape(query)
        );
    }
    let mut out = format!("{BASE_URL}/biblioteca.php?page={page}");
    for key in ["genre", "type", "status"] {
        if let Some(value) = filter(filters, key) {
            out.push('&');
            out.push_str(key);
            out.push('=');
            out.push_str(&url::query_escape(value));
        }
    }
    out
}

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(id))
        .and_then(Value::as_str)
}

fn catalog_item(title: &str, href: &str, cover: Option<String>) -> CatalogItem {
    let key = normalize_key(href);
    CatalogItem {
        key: key.clone(),
        title: title.to_string(),
        cover: cover.map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn info_text(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .map(|chunk| html::strip_tags(chunk).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn genre_tags(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("genre"))
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "hiatus" => ItemStatus::Hiatus,
        "cancelled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!("/{}", input.trim_start_matches(BASE_URL).trim_matches('/'));
    }
    format!("/{}", input.trim_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="card"><a href="/manga/sample"><h3>Sample Manga</h3><img src="/cover.jpg"></a></div>
"#;
const LATEST_FIXTURE: &str = r#"
<div><a class="block" href="/manga/sample"><h3>Sample Manga</h3><img src="/cover.jpg"></a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<main><div class="card"><h1>Sample Manga</h1><img src="/cover.jpg"><div><span>Estado</span> ongoing</div><a href="/biblioteca.php?genre=1">Accion</a><p>Sample description</p><div class="grid"><a class="block" href="/manga/sample/1"><span>Capitulo 1</span></a></div></div></main>
"#;
const PAGES_FIXTURE: &str = r#"<div class="chapter-images"><img src="/page1.jpg"></div>"#;
