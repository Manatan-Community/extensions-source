use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: EsMi2Manga = EsMi2Manga;
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: "https://es.mi2manga.com",
    lang: "es",
    content_rating: "adult",
    manga_path: "manga",
    popular_url_marker: "post-title",
    use_load_more: false,
    latest_enabled: true,
};

struct EsMi2Manga;

impl MangaSource for EsMi2Manga {
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(CONFIG.base_url) && query.contains("/manga/") {
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(
                    &manga::Madara::fetch_document_or_fixture(&CONFIG, query, DETAILS_FIXTURE),
                    Some(CONFIG.normalize_manga_key(query)),
                    &CONFIG,
                )],
                has_next_page: false,
            });
        }
        Ok(search_from_body(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.search_url(page, query),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(manga::Madara::parse_details(
            &manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
            &CONFIG,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(manga::Madara::parse_chapters(
            &manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE),
            &key,
            &CONFIG,
        ))
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
                item: Some(manga::Madara::parse_details(
                    &manga::Madara::fetch_document_or_fixture(&CONFIG, input, DETAILS_FIXTURE),
                    Some(CONFIG.normalize_manga_key(input)),
                    &CONFIG,
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
        entries: filter_entries(manga::Madara::parse_listing(body, &CONFIG)),
        has_next_page: manga::Madara::has_next_page(body, &CONFIG),
    }
}

fn search_from_body(body: &str) -> Paged<CatalogItem> {
    let mut entries = manga::Madara::parse_listing(body, &CONFIG);
    if entries.is_empty() {
        entries = parse_tab_search_listing(body);
    }
    Paged {
        entries: filter_entries(entries),
        has_next_page: manga::Madara::has_next_page(body, &CONFIG),
    }
}

fn parse_tab_search_listing(body: &str) -> Vec<CatalogItem> {
    body.split("c-tabs-item__content")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = CONFIG.normalize_manga_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "post-title", "</a>")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Manga".into())),
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| CONFIG.absolute_url(&value)),
                url: Some(CONFIG.absolute_url(&key)),
                language: Some(CONFIG.lang.to_string()),
                content_rating: Some(CONFIG.content_rating.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn filter_entries(entries: Vec<CatalogItem>) -> Vec<CatalogItem> {
    entries
        .into_iter()
        .filter(|entry| {
            entry
                .url
                .as_deref()
                .is_none_or(|value| !value.contains("bilibilicomics.com"))
        })
        .collect()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="site-content">
<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div>
<div class="page-item-detail"><h3 class="post-title"><a href="https://www.bilibilicomics.com/detail/sample">Bilibili</a></h3></div>
</div><div class="nav-previous"></div>
"#;

const SEARCH_FIXTURE: &str = r#"
<div class="site-content"><div class="c-tabs-item__content"><h3 class="post-title"><a href="/manga/sample/">Sample Manga</a></h3><img data-src="/cover.jpg"></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample Manga</h1></div>
<div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary"><div class="summary__content">Sample summary.</div></div>
<ul class="main version-chap"><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">enero 01, 2024</span></li></ul>
"#;

const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn filters_bilibili_and_parses_search_tabs() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries.len(), 1);
        assert_eq!(
            SOURCE
                .search(json!({"query":"sample"}))
                .unwrap()
                .entries
                .len(),
            1
        );
    }
}
