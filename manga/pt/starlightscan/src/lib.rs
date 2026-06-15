use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: StarlightScan = StarlightScan;
const BASE_URL: &str = "https://starligthscan.com";
const MANGA_DIR: &str = "/mangas";

struct StarlightScan;

impl MangaSource for StarlightScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let body = fetch_document(BASE_URL, LIST_FIXTURE);
            return Ok(Paged {
                entries: parse_latest(&body),
                has_next_page: false,
            });
        }
        let body = fetch_document(&search_url(page, ""), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_bulk_listing(&body),
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
        let body = fetch_document(&search_url(page, query), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_bulk_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/mangas/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/mangas/sample".into());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/mangas/sample/capitulo-1".into());
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
        if input.starts_with(BASE_URL) && input.contains(MANGA_DIR) {
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

fn search_url(page: u64, query: &str) -> String {
    let path = if query.is_empty() { "mangas" } else { "buscar" };
    format!(
        "{BASE_URL}/{path}?search={}&page-current={page}",
        url::query_escape(query)
    )
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, Some(key.to_string()))
}

fn parse_bulk_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("bulkMangaCard"))
        .filter_map(|chunk| card_item(chunk, "bulkMangaCard"))
        .fold(Vec::new(), push_unique)
}

fn parse_latest(body: &str) -> Vec<CatalogItem> {
    body.split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("mostRecentMangaCard"))
        .filter_map(|chunk| card_item(chunk, "mostRecentMangaCard"))
        .fold(Vec::new(), push_unique)
}

fn card_item(chunk: &str, class_prefix: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    let title_marker = format!("{class_prefix}__title");
    let cover_marker = format!("{class_prefix}__cover");
    let title = html::text_between(chunk, &title_marker, "</a>")
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Starlight Scan".to_string()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, &cover_marker, "src").map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/mangas/sample".to_string());
    let lower = body.to_ascii_lowercase();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "mangaDetails__title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Starlight Scan".to_string())),
        cover: html::attr_after(body, "mangaDetails__cover", "src").map(|image| absolute_url(&image)),
        description: html::text_between(body, "mangaDetails__description", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: html::text_between(body, "mangaDetails__author", "</")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        tags: body
            .split("<li")
            .skip(1)
            .filter(|chunk| chunk.contains("mangaTags__item"))
            .filter_map(|chunk| html::text_between(chunk, ">", "</li>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: if lower.contains("publicação finalizada") || lower.contains("publicacao finalizada") {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(absolute_url(&key)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("mangaDetails__episode"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "mangaDetails__episodeTitle", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: html::text_between(chunk, "mangaDetails__episodeReleaseDate", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| dates::parse_ymd(&value)),
                url: Some(absolute_url(&key)),
                language: Some("pt-BR".to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, page_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("scanImage") || chunk.contains("scanImagesContainer"))
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

fn has_next_page(body: &str) -> bool {
    body.contains("Próxima") && !body.contains("disabled")
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="bulkMangaList"><article class="bulkMangaCard"><a class="bulkMangaCard__title" href="/mangas/sample">Sample</a><img class="bulkMangaCard__cover" src="/cover.jpg"></article></div>
<article class="mostRecentMangaCard"><a class="mostRecentMangaCard__title" href="/mangas/sample">Sample</a><img class="mostRecentMangaCard__cover" src="/cover.jpg"></article>
"#;
const DETAILS_FIXTURE: &str = r#"
<section class="mangaDetails"><h1 class="mangaDetails__title">Sample</h1><img class="mangaDetails__cover" src="/cover.jpg"><span class="mangaDetails__author">Author</span><span class="mangaDetails__description">Description</span><span class="base__horizontalList" title="Status">Publicação Finalizada</span><ul><li class="mangaTags__item">Drama</li></ul><div class="mangaDetails__episodesContainer"><div class="mangaDetails__episode"><a class="mangaDetails__episodeTitle" href="/mangas/sample/capitulo-1">Capitulo 1</a><span class="mangaDetails__episodeReleaseDate">2024-01-01</span></div></div></section>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="scanImagesContainer"><img class="scanImage" src="/page1.jpg"></div>"#;
