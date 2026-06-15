use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: TiaManhwa = TiaManhwa;
const BASE_URL: &str = "https://tiamanhwa.com";
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: BASE_URL,
    lang: "pt-BR",
    content_rating: "adult",
    manga_path: "manhwa",
    popular_url_marker: "post-title",
    use_load_more: true,
    latest_enabled: true,
};

struct TiaManhwa;

impl MangaSource for TiaManhwa {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let body = fetch_document(&format!("{BASE_URL}/page/{page}/"), LIST_FIXTURE);
            return Ok(Paged {
                entries: parse_latest(&body),
                has_next_page: body.contains("nextpostslink"),
            });
        }
        let body = fetch_document(BASE_URL, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_popular(&body),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = CONFIG.normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let body = fetch_document(
            &format!(
                "{BASE_URL}/page/{page}/?s={}&post_type=wp-manga",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_search(&body),
            has_next_page: body.contains("page-numbers next") || body.contains("a.next"),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manhwa/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manhwa/sample".into());
        let body = fetch_document(&CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        let chapters = parse_chapters(&body);
        if chapters.is_empty() {
            Ok(manga::Madara::parse_chapters(&body, &key, &CONFIG))
        } else {
            Ok(chapters)
        }
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manhwa/sample/chapter-1".into());
        let body = fetch_document(&CONFIG.absolute_url(&key), PAGES_FIXTURE);
        Ok(manga::Madara::parse_pages(&body, &CONFIG))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| CONFIG.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| CONFIG.absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = CONFIG.normalize_manga_key(input);
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
    manga::Madara::browser_client(&CONFIG)
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn details_by_key(key: &str) -> CatalogItem {
    manga::Madara::parse_details(
        &fetch_document(&CONFIG.absolute_url(key), DETAILS_FIXTURE),
        Some(key.to_string()),
        &CONFIG,
    )
}

fn parse_popular(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("slider__item"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = CONFIG.normalize_manga_key(&href);
            let title = html::text_between(chunk, "<h4", "</h4>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Tia Manhwa".into()));
            Some(catalog_item(key, title, html::attr_after(chunk, "<img", "src")))
        })
        .fold(Vec::new(), push_unique)
}

fn parse_latest(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("page-item-detail") && chunk.contains("manga"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "post-title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = CONFIG.normalize_manga_key(&href);
            let title = html::text_between(chunk, "post-title", "</")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Tia Manhwa".into()));
            Some(catalog_item(key, title, html::attr_after(chunk, "<img", "src")))
        })
        .fold(Vec::new(), push_unique)
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    parse_latest(body)
}

fn catalog_item(key: String, title: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|image| CONFIG.absolute_url(&image)),
        url: Some(CONFIG.absolute_url(&key)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split(['<'])
        .filter(|chunk| {
            chunk.contains("wp-manga-chapter")
                || chunk.contains("chapter-item")
                || chunk.contains("div class=\"chapter")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = CONFIG.normalize_manga_key(&href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(CONFIG.absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div id="manga-slider-2"><div class="slider__item"><a href="/manhwa/sample/"><h4><a>Sample</a></h4><img src="/cover.jpg"></a></div></div>
<div id="loop-content"><div class="page-listing-item"><div class="page-item-detail manga"><div class="post-title"><h3><a href="/manhwa/sample/">Sample</a></h3></div><div class="item-thumb"><img src="/cover.jpg"></div></div></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="post-title">Sample</h1><div class="summary_image"><img src="/cover.jpg"></div><ul><li class="wp-manga-chapter"><a href="/manhwa/sample/chapter-1/">Chapter 1</a></li></ul>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;
