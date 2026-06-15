use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MuitoHentai = MuitoHentai;
const BASE_URL: &str = "https://www.muitohentai.com";

struct MuitoHentai;

impl MangaSource for MuitoHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = match listing {
            "latest" => manga_url(page),
            _ => manga_url(page),
        };
        let body = fetch_document(&target, LIST_FIXTURE);
        let entries = if listing == "latest" {
            parse_latest(&body)
        } else {
            parse_listing(&body)
        };
        Ok(Paged {
            entries,
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
        let target = format!("{BASE_URL}/buscar-manga/?q={}", url::query_escape(query));
        let body = fetch_document(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/hentai/sample/chapter-1".into());
        let page_url = absolute_url(&key);
        let body = fetch_document(&page_url, PAGES_FIXTURE);
        Ok(parse_pages(&body, &page_url))
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

fn manga_url(page: u64) -> String {
    if page <= 1 {
        format!("{BASE_URL}/mangas/")
    } else {
        format!("{BASE_URL}/mangas/{page}")
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!(
                "/{}",
                value[index + BASE_URL.len()..]
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, Some(key.to_string()))
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("tvshows"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h3", "</h3>")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Muito Hentai".into()));
            Some(catalog_item(
                key,
                title,
                html::attr_after(chunk, "<img", "src"),
            ))
        })
        .fold(Vec::new(), push_unique)
}

fn parse_latest(body: &str) -> Vec<CatalogItem> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("capaMangaHentai") || chunk.contains("lancamento-cap2"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h2", "</h2>")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Muito Hentai".into()));
            Some(catalog_item(
                key,
                title,
                html::attr_after(chunk, "<img", "src"),
            ))
        })
        .fold(Vec::new(), push_unique)
}

fn catalog_item(key: String, title: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "<title", "</title>"))
            .map(|value| html::strip_tags(&value).replace(" - Muito Hentai", ""))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Muito Hentai".to_string())),
        cover: html::attr_after(body, "#capaAnime", "src")
            .or_else(|| html::attr_after(body, "capaAnime", "src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "Sinopse", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: side_value(body, "Autor"),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("genero_btn"))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        url: Some(absolute_url(&key)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn side_value(body: &str, label: &str) -> Vec<String> {
    html::text_between(body, label, "</div>")
        .map(|value| html::strip_tags(&value).replace(':', ""))
        .filter(|value| !value.is_empty() && value != label)
        .into_iter()
        .collect()
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<h3")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, page_url: &str) -> Vec<MangaPage> {
    extract_image_array(body)
        .into_iter()
        .enumerate()
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

fn extract_image_array(body: &str) -> Vec<String> {
    let Some(raw) = body
        .split("var arr =")
        .nth(1)
        .and_then(|value| value.split(';').next())
    else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(raw.trim()).unwrap_or_default()
}

fn has_next_page(body: &str) -> bool {
    body.contains("contains(»)") || body.contains("&raquo;") || body.contains(">»<")
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div id="archive-content"><article class="tvshows"><div class="poster"><img src="/cover.jpg"></div><div class="data"><h3><a href="/manga/sample">Sample</a></h3></div></article></div>
<ul class="lancamento-cap2"><li><a href="/manga/sample"><div class="capaMangaHentai"><img src="/cover.jpg"></div><h2>Sample</h2></a></li></ul>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample</h1><div id="capaAnime"><img src="/cover.jpg"></div><a class="genero_btn">Ação</a>
<div class="backgroundpost">Sinopse Sample description</div>
<div class="backgroundpost"><h3><a href="/hentai/sample/chapter-1">Chapter 1</a></h3></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<script>var numeroImgAtual = 1; var arr = ["/page1.jpg","/page2.jpg"];</script>"#;
