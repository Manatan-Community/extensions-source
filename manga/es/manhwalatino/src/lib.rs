use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: ManhwaLatino = ManhwaLatino;
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: "https://manhwa-latino.com",
    lang: "es",
    content_rating: "adult",
    manga_path: "manga",
    popular_url_marker: "post-title",
    use_load_more: false,
    latest_enabled: true,
};

struct ManhwaLatino;

impl MangaSource for ManhwaLatino {
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
        Ok(list_from_body(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.list_url(page, order),
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
        Ok(fetch_all_chapters(&key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(manga::Madara::parse_pages(
            &manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), PAGES_FIXTURE),
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

fn list_from_body(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: manga::Madara::parse_listing(body, &CONFIG),
        has_next_page: manga::Madara::has_next_page(body, &CONFIG),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let mut item = manga::Madara::parse_details(body, key, &CONFIG);
    item.description = html::text_between(body, "Resumen", "</div>")
        .or_else(|| html::text_between(body, "summary-container", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or(item.description);
    item
}

fn fetch_all_chapters(key: &str) -> Vec<MangaChapter> {
    let manga_url = CONFIG.absolute_url(key);
    let mut chapters = Vec::new();
    for page in 1..=20 {
        let target = if page == 1 {
            manga_url.clone()
        } else {
            format!("{manga_url}?t={page}")
        };
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &target, DETAILS_FIXTURE);
        let mut current = parse_chapter_blocks(&body);
        if current.is_empty() {
            current = manga::Madara::parse_chapters(&body, key, &CONFIG);
        }
        if current.is_empty() || (current.len() == 1 && current[0].key == key) {
            break;
        }
        chapters.extend(current);
        if !body.contains("pagination") || !body.contains("current + span") {
            if !body.contains("page-numbers") && !body.contains("?t=") {
                break;
            }
        }
    }
    if chapters.is_empty() {
        manga::Madara::parse_chapters(
            &manga::Madara::fetch_document_or_fixture(&CONFIG, &manga_url, DETAILS_FIXTURE),
            key,
            &CONFIG,
        )
    } else {
        dedupe_chapters(chapters)
    }
}

fn parse_chapter_blocks(body: &str) -> Vec<MangaChapter> {
    body.split("mini-letters")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key =
                CONFIG.normalize_manga_key(href.split("?style=paged").next().unwrap_or(&href));
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(CONFIG.absolute_url(&key)),
                date_uploaded: html::attr_after(chunk, "<img", "alt")
                    .or_else(|| html::attr_after(chunk, "<span", "title"))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                is_locked: chunk.contains("premium") || chunk.contains("locked"),
                ..MangaChapter::default()
            })
        })
        .filter(|chapter| !chapter.is_locked)
        .collect()
}

fn dedupe_chapters(mut chapters: Vec<MangaChapter>) -> Vec<MangaChapter> {
    let mut out = Vec::new();
    for chapter in chapters.drain(..) {
        if !out
            .iter()
            .any(|existing: &MangaChapter| existing.key == chapter.key)
        {
            out.push(chapter);
        }
    }
    out
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div>
<div class="nav-previous"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample Manga</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<div class="post-content_item"><div>Resumen</div><div class="summary-container">Resumen</div></div>
<li class="wp-manga-chapter"><div class="mini-letters"><a href="/manga/sample/chapter-1/?style=paged">Chapter 1</a></div><span class="chapter-release-date">01/01/2024</span></li>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="page-break"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;
