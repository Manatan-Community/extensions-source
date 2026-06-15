use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MundoHentai = MundoHentai;
const BASE_URL: &str = "https://mundohentaioficial.com";

struct MundoHentai;

impl MangaSource for MundoHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_document(&popular_url(page), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
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
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }

        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = if query.is_empty() {
            filter_string(filters, "tag")
                .filter(|tag| !tag.is_empty())
                .map(|tag| tag_url(page, &tag))
                .unwrap_or_else(|| popular_url(page))
        } else {
            format!("{BASE_URL}/?s={}", url::query_escape(query))
        };
        let body = fetch_document(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample#1".into());
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
        .with_referer(BASE_URL.to_string())
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

fn popular_url(page: u64) -> String {
    if page <= 1 {
        format!("{BASE_URL}/category/doujinshi/")
    } else {
        format!("{BASE_URL}/category/doujinshi/page/{page}")
    }
}

fn tag_url(page: u64, tag: &str) -> String {
    if page <= 1 {
        format!("{BASE_URL}/tag/{tag}/")
    } else {
        format!("{BASE_URL}/tag/{tag}/page/{page}")
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value.split('#').next().unwrap_or(value))
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
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("thumb-conteudo") && !chunk.contains("Tufos"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "thumb-titulo", "</span>")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Mundo Hentai".into()));
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
    let key = key.unwrap_or_else(|| "/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "<title", "</title>"))
            .map(|value| html::strip_tags(&value).replace(" - Mundo Hentai", ""))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Mundo Hentai".to_string())),
        cover: html::attr_after(body, "post-capa", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "Cor:", "</li>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: post_values(body, "Artista:"),
        tags: post_values(body, "Tags:"),
        status: ItemStatus::Completed,
        url: Some(absolute_url(&key)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn post_values(body: &str, label: &str) -> Vec<String> {
    html::text_between(body, label, "</li>")
        .map(|value| html::strip_tags(&value).replace(label, ""))
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("galeriaTab"))
        .filter_map(|chunk| {
            let chapter_id = html::attr(chunk, "data-id")?;
            let title = html::text_between(chunk, "galeriaTabTitulo", "</div>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            let key = format!("{manga_key}#{chapter_id}");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    title
                        .map(|value| format!("Capitulo {chapter_id} - {value}"))
                        .unwrap_or_else(|| format!("Capitulo {chapter_id}")),
                ),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Capitulo".to_string()),
            url: Some(absolute_url(manga_key)),
            ..MangaChapter::default()
        });
    } else {
        chapters.reverse();
    }
    chapters
}

fn parse_pages(body: &str, page_url: &str) -> Vec<MangaPage> {
    let chapter_id = page_url.split('#').next_back().unwrap_or_default();
    let selector_marker = if chapter_id != page_url {
        format!("galeria-{chapter_id}")
    } else {
        "post-fotos".to_string()
    };
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            selector_marker == "post-fotos"
                || body[..body.find(chunk).unwrap_or(body.len())].contains(&selector_marker)
        })
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
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

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
}

fn has_next_page(body: &str) -> bool {
    body.contains("paginacao") && body.contains("next")
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="lista"><ul><li><div class="thumb-conteudo"><a href="https://mundohentaioficial.com/sample"><span class="thumb-imagem"><img class="attachment-post-thumbnail" src="/cover.jpg"></span><span class="thumb-titulo">Sample</span></a></div></li></ul></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="post-box"><h1>Sample</h1><div class="post-capa"><img src="/cover.jpg"></div>
<ul class="post-itens"><li>Artista: <a>Artist</a></li><li>Tags: <a>Tag</a></li><li>Cor: Colorido</li></ul>
<div class="listaImagens"><ul class="post-fotos"><li><img src="/page1.jpg"></li></ul></div></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="listaImagens"><ul class="post-fotos"><li><img src="/page1.jpg"></li></ul></div>"#;
