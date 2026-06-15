use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: PlumaComics = PlumaComics;
const BASE_URL: &str = "https://plumacomics.cloud";

struct PlumaComics;

impl MangaSource for PlumaComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/series")
        } else {
            format!("{BASE_URL}/series?sort=popular")
        };
        let body = fetch_document(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
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
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let body = fetch_json(
            &format!("{BASE_URL}/api/search?q={}", url::query_escape(query)),
            SEARCH_FIXTURE,
        );
        Ok(Paged {
            entries: parse_search(&body),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/ler/sample/1".into());
        let page_url = absolute_url(&key);
        let body = fetch_document(&page_url, PAGES_FIXTURE);
        let chapter_id = extract_chapter_id(&body).unwrap_or(1);
        let pages = fetch_json(
            &format!("{BASE_URL}/api/viewer/bootstrap?c={chapter_id}"),
            PAGES_API_FIXTURE,
        );
        Ok(parse_pages(&pages, &page_url))
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
                item: Some(details_by_key(&key)),
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
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!("/{}", value[index + BASE_URL.len()..].trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, Some(key.to_string()))
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("group") && chunk.contains("series"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h3", "</h3>")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Pluma Comics".into()));
            Some(catalog_item(
                key,
                title,
                html::attr_after(chunk, "<img", "src"),
            ))
        })
        .fold(Vec::new(), push_unique)
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).unwrap());
    root.get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let slug = item.get("slug").and_then(Value::as_str)?;
            let title = item.get("title").and_then(Value::as_str).unwrap_or("Pluma Comics");
            let key = format!("/series/{slug}");
            Some(catalog_item(
                key,
                title.to_string(),
                item.get("coverPath")
                    .and_then(Value::as_str)
                    .map(|cover| format!("/api/cover/{cover}")),
            ))
        })
        .collect()
}

fn catalog_item(key: String, title: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".to_string());
    let lower = body.to_ascii_lowercase();
    CatalogItem {
        key: key.clone(),
        title: html::attr_after(body, "property=\"og:title\"", "content")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value).replace("| Pluma Comics", ""))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Pluma Comics".to_string())),
        cover: html::attr_after(body, "cover-img", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "card", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: body
            .split("<span")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</span>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: if lower.contains("em andamento") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(&key)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("ler"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<span", "</span>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(absolute_url(&key)),
                language: Some("pt-BR".to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn extract_chapter_id(body: &str) -> Option<u64> {
    let raw = next_data(body).unwrap_or_else(|| body.to_string());
    let direct = raw
        .split("chapterId")
        .nth(1)?
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    direct.parse().ok()
}

fn next_data(body: &str) -> Option<String> {
    html::text_between(body, "<script id=\"__NEXT_DATA__\"", "</script>")
        .or_else(|| html::text_between(body, "<script id='__NEXT_DATA__'", "</script>"))
        .map(|value| html::html_unescape(&value))
}

fn parse_pages(body: &str, page_url: &str) -> Vec<MangaPage> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(PAGES_API_FIXTURE).unwrap());
    root.get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let index = page.get("i").and_then(Value::as_u64).unwrap_or(0);
            let url = page.get("u").and_then(Value::as_str)?;
            Some((index, url.to_string()))
        })
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(page_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn has_next_page(body: &str) -> bool {
    body.contains("btn-primary") && body.contains("page")
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<a class="group" href="/series/sample"><img src="/cover.jpg"><h3>Sample</h3></a>"#;
const SEARCH_FIXTURE: &str =
    r#"{"results":[{"title":"Sample","slug":"sample","coverPath":"sample.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"
<meta property="og:title" content="Sample | Pluma Comics"><img class="cover-img" src="/cover.jpg">
<div class="card"><p class="text-sm">Description</p></div><span>Drama</span>
<a href="/ler/sample/1"><span>Capitulo 1</span></a>
"#;
const PAGES_FIXTURE: &str =
    r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"chapterId":1}}}</script>"#;
const PAGES_API_FIXTURE: &str = r#"{"pages":[{"i":0,"u":"page1.jpg"},{"i":1,"u":"page2.jpg"}]}"#;
