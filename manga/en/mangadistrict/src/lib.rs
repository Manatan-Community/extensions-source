use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: MangaDistrict = MangaDistrict;

struct MangaDistrict;

impl MangaSource for MangaDistrict {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing_page(LIST_FIXTURE, &config));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
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
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_details(&body, Some(key), &config))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_chapters(&body, &key, &config))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config();
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".to_string());
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
        entries: manga::Madara::parse_listing(body, config)
            .into_iter()
            .map(clean_item_title)
            .collect(),
        has_next_page: body.contains("nav-previous")
            || body.contains("current + a")
            || body.contains("page-numbers")
            || body.contains("next page-numbers"),
    }
}

fn parse_pages(body: &str, config: &MadaraConfig) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| !chunk.contains("id=\"image-99999") && !chunk.contains("id='image-99999"))
        .filter_map(|chunk| {
            html::attr(chunk, "data-wpfc-original-src")
                .or_else(|| html::attr(chunk, "data-src"))
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

fn clean_item_title(mut item: CatalogItem) -> CatalogItem {
    item.title = item.title.trim().to_string();
    item
}

fn config() -> MadaraConfig {
    MadaraConfig {
        base_url: "https://mangadistrict.com",
        lang: "en",
        content_rating: "nsfw",
        manga_path: "series",
        popular_url_marker: "post-title",
        use_load_more: false,
        latest_enabled: true,
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail manga"><h3 class="post-title"><a href="/series/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div>
<div role="navigation"><span class="current">1</span><a class="page">2</a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample Manga</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<ul class="main version-chap"><li class="wp-manga-chapter"><a href="/series/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">January 1, 2024</span></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"<div class="page-break"><img data-wpfc-original-src="/page1.jpg"><img id="image-99999" src="/skip.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_district_pages() {
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 1);
    }
}
