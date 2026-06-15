use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    html,
    manga::{self, Gattsu, GattsuConfig},
    sdk::http::HttpClient,
    url,
};
use serde_json::Value;

const SOURCE: UniversoHentai = UniversoHentai;
const CONFIG: GattsuConfig = GattsuConfig {
    base_url: "https://universohentai.com",
    name: "Universo Hentai",
    lang: "pt-BR",
    content_rating: "adult",
};

struct UniversoHentai;

impl MangaSource for UniversoHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            let body = fetch_document(CONFIG.base_url, LIST_FIXTURE);
            return Ok(Paged {
                entries: parse_latest(&body),
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_document(&CONFIG.list_url(page), LIST_FIXTURE);
        Ok(Paged {
            entries: Gattsu::parse_listing(&body, &CONFIG),
            has_next_page: Gattsu::has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(CONFIG.base_url) {
            let key = CONFIG.normalize_key(query);
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let body = fetch_document(&CONFIG.search_url(page, query), LIST_FIXTURE);
        Ok(Paged {
            entries: Gattsu::parse_listing(&body, &CONFIG),
            has_next_page: Gattsu::has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document(&CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/galeria".into());
        let body = fetch_document(&CONFIG.absolute_url(&key), PAGES_FIXTURE);
        Ok(Gattsu::parse_pages(&body, &CONFIG))
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
        if input.starts_with(CONFIG.base_url) {
            let key = CONFIG.normalize_key(input);
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
    Gattsu::browser_client(&CONFIG)
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_document(&CONFIG.absolute_url(key), DETAILS_FIXTURE);
    let mut item = Gattsu::parse_details(&body, Some(key.to_string()), &CONFIG);
    item.authors = info_values(&body, "Artista");
    item.tags = info_values(&body, "Categorias");
    item.status = ItemStatus::Completed;
    item
}

fn parse_latest(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("video") && chunk.contains("video-titulo") && !chunk.contains("selo-hd"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = CONFIG.normalize_key(&href);
            let title = html::text_between(chunk, "video-titulo", "</")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| CONFIG.name.into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|image| CONFIG.absolute_url(&image)),
                url: Some(CONFIG.absolute_url(&key)),
                language: Some(CONFIG.lang.to_string()),
                content_rating: Some(CONFIG.content_rating.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let href = html::attr_after(body, "title=\"Abrir galeria\"", "href")
        .or_else(|| html::attr_after(body, "Abrir galeria", "href"))
        .unwrap_or_else(|| manga_key.to_string());
    let key = CONFIG.normalize_key(&href);
    vec![MangaChapter {
        key: key.clone(),
        title: Some("Capitulo unico".to_string()),
        scanlators: info_values(body, "Tradutor"),
        date_uploaded: html::attr_after(body, "article:published_time", "content")
            .and_then(|value| crate_date(&value)),
        url: Some(CONFIG.absolute_url(&key)),
        ..MangaChapter::default()
    }]
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains(label))
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn crate_date(value: &str) -> Option<i64> {
    manatan_shared::dates::parse_ymd(value.split('T').next().unwrap_or(value))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<ul><li><span class="thumb-titulo">Sample</span><a href="/sample"><span class="thumb-imagem"><img src="/cover.jpg"></span></a></li></ul>
<div class="meio"><div class="videos"><div class="video"><a href="https://universohentai.com/sample"><span class="video-titulo">Sample</span><img class="wp-post-image" src="/cover.jpg"></a></div></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="meio"><div class="post" itemscope><h1 class="post-titulo">Sample</h1><div class="paginaPostThumb"><img class="wp-post-image" src="/cover.jpg"></div><ul class="paginaPostItens"><li>Artista <a>Artist</a></li><li>Categorias <a>Hentai</a></li><li>Tradutor <a>Scan</a></li></ul><a title="Abrir galeria" href="/sample/galeria">Abrir galeria</a></div></div>
"#;
const PAGES_FIXTURE: &str = r#"<div class="meio"><div class="galeria"><div class="galeria-foto"><a><img src="/page1.jpg"></a></div></div></div>"#;
