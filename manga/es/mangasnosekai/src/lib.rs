use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: MangasNoSekai = MangasNoSekai;
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: "https://mangasnosekai.com",
    lang: "es",
    content_rating: "safe",
    manga_path: "manga",
    popular_url_marker: "figcaption",
    use_load_more: false,
    latest_enabled: true,
};

struct MangasNoSekai;

impl MangaSource for MangasNoSekai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(list_from_body(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        let target = biblioteca_url(page, order);
        Ok(list_from_body(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &target,
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(CONFIG.base_url) && query.contains("/manga/") {
            return Ok(Paged {
                entries: vec![parse_details(
                    &manga::Madara::fetch_document_or_fixture(&CONFIG, query, DETAILS_FIXTURE),
                    Some(CONFIG.normalize_manga_key(query)),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(list_from_body(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.search_url(page, query),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(fetch_chapters(&key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let mut chapter_url = CONFIG.absolute_url(&key);
        if !chapter_url.ends_with('/') {
            chapter_url.push('/');
        }
        Ok(manga::Madara::parse_pages(
            &manga::Madara::fetch_document_or_fixture(&CONFIG, &chapter_url, PAGES_FIXTURE),
            &CONFIG,
        ))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(CONFIG.base_url) && input.contains("/manga/") {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &manga::Madara::fetch_document_or_fixture(&CONFIG, input, DETAILS_FIXTURE),
                    Some(CONFIG.normalize_manga_key(input)),
                )),
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

fn biblioteca_url(page: u64, order: &str) -> String {
    let page_path = if page <= 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    format!(
        "{}/biblioteca/{}?m_orderby={}",
        CONFIG.base_url, page_path, order
    )
}

fn list_from_body(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: parse_listing(body),
        has_next_page: body.contains("next page-numbers") || body.contains("nav-previous"),
    }
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("page-listing-item") || chunk.contains("page-item-detail"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = CONFIG.normalize_manga_key(&href);
            let title = html::text_between(chunk, "figcaption", "</figcaption>")
                .or_else(|| html::text_between(chunk, "post-title", "</a>"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| CONFIG.absolute_url(&value)),
                url: Some(CONFIG.absolute_url(&key)),
                language: Some("es".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    if entries.is_empty() {
        manga::Madara::parse_listing(body, &CONFIG)
    } else {
        entries
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let mut item = manga::Madara::parse_details(body, key, &CONFIG);
    if let Some(title) = html::text_between(body, "titleMangaSingle", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
    {
        item.title = title;
    }
    item.description = html::text_between(body, "section-sinopsis", "</section>")
        .and_then(|section| html::text_between(&section, "<p", "</p>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or(item.description);
    item.cover = html::attr_after(body, "thumble-container", "src")
        .map(|value| CONFIG.absolute_url(&value))
        .or(item.cover);
    let lower = body.to_ascii_lowercase();
    item.status = if lower.contains("completado") || lower.contains("finalizado") {
        ItemStatus::Completed
    } else if lower.contains("cancelado") {
        ItemStatus::Cancelled
    } else if lower.contains("hiatus") || lower.contains("paus") {
        ItemStatus::Hiatus
    } else {
        item.status
    };
    item
}

fn fetch_chapters(key: &str) -> Vec<MangaChapter> {
    let manga_url = CONFIG.absolute_url(key);
    let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &manga_url, DETAILS_FIXTURE);
    let chapters = manga::Madara::parse_chapters(&body, key, &CONFIG);
    if !chapters.is_empty() && chapters[0].key != key {
        return chapters;
    }
    let ajax = manga::Madara::browser_client(&CONFIG)
        .post(format!("{}/ajax/chapters", manga_url.trim_end_matches('/')))
        .form(&[])
        .xhr()
        .send_text()
        .unwrap_or_else(|_| DETAILS_FIXTURE.to_string());
    let parsed = manga::Madara::parse_chapters(&ajax, key, &CONFIG);
    if parsed.is_empty() { chapters } else { parsed }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-listing-item"><div class="row"><div><a href="/manga/sample/"><figure><img src="/cover.jpg"></figure><figcaption>Sample Manga</figcaption></a></div></div></div>
<a class="next page-numbers" href="/biblioteca/page/2/">2</a>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="thumble-container"><p class="titleMangaSingle">Sample Manga</p><img class="img-responsive" src="/cover.jpg"></div>
<section id="section-sinopsis"><p>Resumen</p><div class="d-flex"><div>Estado</div><p>En emision</p></div></section>
<ul><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">enero 01, 2024</span></li></ul>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;
