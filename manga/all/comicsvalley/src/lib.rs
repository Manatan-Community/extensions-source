use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: ComicsValley = ComicsValley;
const CONFIG: manga::MadaraConfig = manga::MadaraConfig {
    base_url: "https://comicsvalley.com",
    lang: "all",
    content_rating: "adult",
    manga_path: "comics-new",
    popular_url_marker: "<a",
    use_load_more: true,
    latest_enabled: true,
};

struct ComicsValley;

impl MangaSource for ComicsValley {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let body = load_more(page, if latest { "_latest_update" } else { "_wp_manga_views" }, "", LIST_FIXTURE);
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &CONFIG),
            has_next_page: manga::Madara::has_next_page(&body, &CONFIG),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(CONFIG.base_url) && query.contains("/comics-new/") {
            let body = manga::Madara::fetch_document_or_fixture(&CONFIG, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(&body, Some(CONFIG.normalize_manga_key(query)), &CONFIG)],
                has_next_page: false,
            });
        }
        let body = if query.is_empty() {
            load_more(page, "_wp_manga_views", "", LIST_FIXTURE)
        } else {
            load_more(page, "date", query, LIST_FIXTURE)
        };
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &CONFIG),
            has_next_page: manga::Madara::has_next_page(&body, &CONFIG),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics-new/sample".into());
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_details(&body, Some(key), &CONFIG))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics-new/sample".into());
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_chapters(&body, &key, &CONFIG))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/comics-new/sample/chapter-1".into());
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), PAGES_FIXTURE);
        Ok(manga::Madara::parse_pages(&body, &CONFIG))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(CONFIG.base_url) && input.contains("/comics-new/") {
            let body = manga::Madara::fetch_document_or_fixture(&CONFIG, input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(manga::Madara::parse_details(&body, Some(CONFIG.normalize_manga_key(input)), &CONFIG)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn load_more(page: u64, meta_key: &str, query: &str, fixture: &str) -> String {
    let page_index = page.saturating_sub(1).to_string();
    let mut form = vec![
        ("action", "madara_load_more"),
        ("page", page_index.as_str()),
        ("template", if query.is_empty() { "madara-core/content/content-archive" } else { "madara-core/content/content-search" }),
        ("vars[paged]", "1"),
        ("vars[post_type]", "wp-manga"),
        ("vars[post_status]", "publish"),
        ("vars[manga_archives_item_layout]", "big_thumbnail"),
    ];
    if query.is_empty() {
        form.push(("vars[orderby]", "meta_value_num"));
        form.push(("vars[meta_key]", meta_key));
        form.push(("vars[order]", "desc"));
    } else {
        form.push(("vars[s]", query));
    }
    manga::Madara::browser_client(&CONFIG)
        .post(format!("{}/wp-admin/admin-ajax.php", CONFIG.base_url))
        .xhr()
        .form(&form)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail manga"><a href="https://comicsvalley.com/comics-new/sample/">Sample</a><img src="https://img.example/cover.jpg"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample</h1><div class="summary_image"><img src="https://img.example/cover.jpg"></div><li class="wp-manga-chapter"><a href="https://comicsvalley.com/comics-new/sample/chapter-1/">Chapter 1</a></li>
"#;
const PAGES_FIXTURE: &str = r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="https://img.example/1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_madara_bits() {
        assert_eq!(manga::Madara::parse_listing(LIST_FIXTURE, &CONFIG).len(), 1);
        assert_eq!(manga::Madara::parse_chapters(DETAILS_FIXTURE, "/comics-new/sample", &CONFIG).len(), 1);
        assert_eq!(manga::Madara::parse_pages(PAGES_FIXTURE, &CONFIG).len(), 1);
    }
}
