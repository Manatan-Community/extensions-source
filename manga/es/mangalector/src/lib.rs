use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    html, manga,
    manga::MadaraConfig,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: Mangalector = Mangalector;

struct Mangalector;

impl MangaSource for Mangalector {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: manga::Madara::parse_listing(LIST_FIXTURE, &config),
                has_next_page: true,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if listing_id(&request) == "latest" {
            "latest-manga"
        } else {
            "popular-manga"
        };
        let target = format!("{}/{path}?page={page}", config.base_url);
        let body = manga::Madara::fetch_document_or_fixture(&config, &target, LIST_FIXTURE);
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &config),
            has_next_page: body.contains("next") || body.contains("pagination"),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(config.base_url) {
            let key = normalize_mangalector_url(query, &config);
            let body = manga::Madara::fetch_document_or_fixture(
                &config,
                &config.absolute_url(&key),
                DETAILS_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(&body, Some(key), &config)],
                has_next_page: false,
            });
        }
        let target = format!(
            "{}/search?s={}&page={page}&post_type=wp-manga",
            config.base_url,
            url::query_escape(query)
        );
        let body = manga::Madara::fetch_document_or_fixture(&config, &target, LIST_FIXTURE);
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &config),
            has_next_page: manga::Madara::has_next_page(&body, &config),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            DETAILS_FIXTURE,
        );
        Ok(manga::Madara::parse_details(&body, Some(key), &config))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            DETAILS_FIXTURE,
        );
        let chapters = fetch_ajax_chapters(&body, &key, &config);
        if chapters.is_empty() {
            Ok(manga::Madara::parse_chapters(&body, &key, &config))
        } else {
            Ok(chapters)
        }
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config();
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            PAGES_FIXTURE,
        );
        let pages = parse_arraydata_pages(&body, &config);
        if pages.is_empty() {
            Ok(manga::Madara::parse_pages(&body, &config))
        } else {
            Ok(pages)
        }
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = config();
        Ok(manga::request_key(&request, "manga").map(|key| config.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = config();
        Ok(manga::request_key(&request, "chapter").map(|key| config.absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = config();
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(config.base_url) {
            let key = normalize_mangalector_url(input, &config);
            return Ok(Some(UrlResolveResult {
                item: Some(manga::Madara::parse_details(
                    &manga::Madara::fetch_document_or_fixture(
                        &config,
                        &config.absolute_url(&key),
                        DETAILS_FIXTURE,
                    ),
                    Some(key),
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

fn config() -> MadaraConfig {
    MadaraConfig {
        base_url: "https://mangalector.com",
        lang: "es",
        content_rating: "adult",
        manga_path: "manga",
        popular_url_marker: "post-title",
        use_load_more: false,
        latest_enabled: true,
    }
}

fn fetch_ajax_chapters(body: &str, manga_key: &str, config: &MadaraConfig) -> Vec<MangaChapter> {
    let Some(manga_id) = html::attr_after(body, "manga-chapters-holder", "data-id")
        .filter(|value| !value.is_empty())
    else {
        return Vec::new();
    };
    let target = format!("{}/ajax-list-chapter?mangaID={manga_id}", config.base_url);
    let response = HttpClient::browser()
        .with_referer(config.absolute_url(manga_key))
        .with_cookies_for(config.base_url)
        .with_webview_challenge_fallback()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_default();
    manga::Madara::parse_chapters(&response, manga_key, config)
}

fn parse_arraydata_pages(body: &str, config: &MadaraConfig) -> Vec<MangaPage> {
    html::text_between(body, "id=\"arraydata\"", "</p>")
        .or_else(|| html::text_between(body, "id='arraydata'", "</p>"))
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: config.absolute_url(image),
                context: Some(manga::image_headers(config.base_url)),
            },
            headers: manga::image_headers(config.base_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_mangalector_url(input: &str, config: &MadaraConfig) -> String {
    let key = config.normalize_manga_key(input);
    if key.starts_with("/manga/") {
        return key;
    }
    let slug = key
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("sample")
        .split("-capitulo-")
        .next()
        .unwrap_or("sample");
    format!("/manga/{slug}")
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample MangaLector</a></h3><img src="/cover.jpg"></div>
<div class="nav-previous"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div id="manga-chapters-holder" data-id="123"></div>
<div class="post-title"><h1>Sample MangaLector</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<ul><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a></li></ul>
"#;
const PAGES_FIXTURE: &str =
    r#"<p id="arraydata">https://img.example/page1.jpg,https://img.example/page2.jpg</p>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fixture() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample MangaLector"
        );
        assert_eq!(parse_arraydata_pages(PAGES_FIXTURE, &config()).len(), 2);
        assert_eq!(
            normalize_mangalector_url("https://mangalector.com/sample-capitulo-1", &config()),
            "/manga/sample"
        );
    }
}
