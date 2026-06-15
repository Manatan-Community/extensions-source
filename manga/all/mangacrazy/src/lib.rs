use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const SOURCE: MangaCrazy = MangaCrazy;
const CONFIG: manga::MadaraConfig = manga::MadaraConfig {
    base_url: "https://mangacrazy.net",
    lang: "all",
    content_rating: "adult",
    manga_path: "manga",
    popular_url_marker: "<a",
    use_load_more: false,
    latest_enabled: true,
};

struct MangaCrazy;

impl MangaSource for MangaCrazy {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.list_url(page, if latest { "latest" } else { "views" }), LIST_FIXTURE);
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &CONFIG),
            has_next_page: manga::Madara::has_next_page(&body, &CONFIG),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(CONFIG.base_url) && query.contains("/manga/") {
            let body = manga::Madara::fetch_document_or_fixture(&CONFIG, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(&body, Some(CONFIG.normalize_manga_key(query)), &CONFIG)],
                has_next_page: false,
            });
        }
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.search_url(page, query), LIST_FIXTURE);
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &CONFIG),
            has_next_page: manga::Madara::has_next_page(&body, &CONFIG),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_details(&body, Some(key), &CONFIG))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), PAGES_FIXTURE);
        Ok(manga::Madara::parse_pages(&body, &CONFIG))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(CONFIG.base_url) && input.contains("/manga/") {
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

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("wp-manga-chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| "Chapter".to_string());
            let key = CONFIG.normalize_manga_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(CONFIG.absolute_url(&key)),
                date_uploaded: html::text_between(chunk, "chapter-release-date", "</").map(|value| html::strip_tags(&value)).and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Read".into()),
            url: Some(CONFIG.absolute_url(manga_key)),
            ..MangaChapter::default()
        });
    }
    chapters
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail manga"><a href="https://mangacrazy.net/manga/sample/">Sample</a><img src="https://mangacrazy.net/cover.jpg"></div>
<div class="nav-previous"><a href="/manga/page/2/">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample</h1>
<div class="summary_image"><img src="https://mangacrazy.net/cover.jpg"></div>
<li class="wp-manga-chapter"><a href="https://mangacrazy.net/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">2024-01-01</span></li>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="reading-content"><img class="wp-manga-chapter-img" src="https://mangacrazy.net/page-1.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_madara_source() {
        assert_eq!(manga::Madara::parse_listing(LIST_FIXTURE, &CONFIG).len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE, "/manga/sample").len(), 1);
        assert_eq!(manga::Madara::parse_pages(PAGES_FIXTURE, &CONFIG).len(), 1);
    }
}
