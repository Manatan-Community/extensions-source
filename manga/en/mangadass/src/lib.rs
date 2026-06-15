use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: MangaDass = MangaDass;

struct MangaDass;

impl MangaSource for MangaDass {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing_page(LIST_FIXTURE, &config));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "trending"
        };
        Ok(parse_listing_page(
            &manga::Madara::fetch_document_or_fixture(&config, &config.list_url(page, order), LIST_FIXTURE),
            &config,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(config.base_url) {
            let key = config.normalize_manga_key(query);
            let body = manga::Madara::fetch_document_or_fixture(&config, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(&body, Some(key), &config)],
                has_next_page: false,
            });
        }
        Ok(parse_listing_page(
            &manga::Madara::fetch_document_or_fixture(
                &config,
                &config.search_url(page, query),
                LIST_FIXTURE,
            ),
            &config,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_details(&body, Some(key), &config))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key, &config))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config();
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body, &config))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = config();
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(config.base_url) {
            return Ok(Some(UrlResolveResult {
                item: Some(manga::Madara::parse_details(
                    &manga::Madara::fetch_document_or_fixture(&config, input, DETAILS_FIXTURE),
                    Some(config.normalize_manga_key(input)),
                    &config,
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

fn parse_listing_page(body: &str, config: &MadaraConfig) -> Paged<CatalogItem> {
    Paged {
        entries: manga::Madara::parse_listing(body, config),
        has_next_page: manga::Madara::has_next_page(body, config),
    }
}

fn parse_chapters(body: &str, manga_key: &str, config: &MadaraConfig) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("row-content-chapter")
                || chunk.contains("chapter-time")
                || chunk.contains("wp-manga-chapter")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = config.normalize_manga_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(config.absolute_url(&key)),
                date_uploaded: html::text_between(chunk, "chapter-time", "</")
                    .or_else(|| html::text_between(chunk, "chapter-release-date", "</"))
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        chapters = manga::Madara::parse_chapters(body, manga_key, config);
    }
    chapters
}

fn parse_pages(body: &str, config: &MadaraConfig) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "data-src")
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|value| !value.starts_with("data:") && !value.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: config.absolute_url(&image),
                context: Some(manga::image_headers(config.base_url)),
            },
            headers: manga::image_headers(config.base_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn config() -> MadaraConfig {
    MadaraConfig {
        base_url: "https://mangadass.com",
        lang: "en",
        content_rating: "nsfw",
        manga_path: "manga",
        popular_url_marker: "<h3",
        use_load_more: false,
        latest_enabled: true,
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail manga"><h3><a href="/manga/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div>
<div class="nav-previous"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample Manga</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<ul class="row-content-chapter"><li><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-time">01 Jan 2024</span></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"<div class="read-content"><img src="/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_custom_chapters_and_pages() {
        assert_eq!(
            SOURCE.chapters(json!({})).unwrap()[0].title.as_deref(),
            Some("Chapter 1")
        );
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 1);
    }
}
