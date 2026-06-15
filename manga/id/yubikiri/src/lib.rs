use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Yubikiri = Yubikiri;
const BASE_URL: &str = "https://v1.kaguya.pro";

struct Yubikiri;

impl MangaSource for Yubikiri {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: manga::Madara::parse_listing(LIST_FIXTURE, &config),
                has_next_page: manga::Madara::has_next_page(LIST_FIXTURE, &config),
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.list_url(page, order),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &config),
            has_next_page: manga::Madara::has_next_page(&body, &config),
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
        if query.starts_with(BASE_URL) {
            let key = config.normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(
                    &manga::Madara::fetch_document_or_fixture(&config, query, DETAILS_FIXTURE),
                    Some(key),
                    &config,
                )],
                has_next_page: false,
            });
        }
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.search_url(page, query),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &config),
            has_next_page: manga::Madara::has_next_page(&body, &config),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config();
        let key =
            manga::request_key(&request, "manga")
                .unwrap_or_else(|| "/all-series/sample".to_string());
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            DETAILS_FIXTURE,
        );
        Ok(parse_details(&body, Some(key), &config))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config();
        let key =
            manga::request_key(&request, "manga")
                .unwrap_or_else(|| "/all-series/sample".to_string());
        let ajax = fetch_ajax_chapters(&key, &config);
        if !ajax.is_empty() {
            return Ok(ajax);
        }
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            DETAILS_FIXTURE,
        );
        Ok(manga::Madara::parse_chapters(&body, &key, &config))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config();
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/all-series/sample/chapter-1".to_string());
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body, &config))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| config().absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| config().absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = config();
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
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

fn config() -> MadaraConfig {
    MadaraConfig {
        base_url: BASE_URL,
        lang: "id",
        content_rating: "adult",
        manga_path: "all-series",
        popular_url_marker: "post-title",
        use_load_more: false,
        latest_enabled: true,
    }
}

fn parse_details(body: &str, key: Option<String>, config: &MadaraConfig) -> CatalogItem {
    let mut item = manga::Madara::parse_details(body, key, config);
    if let Some(cover) = html::attr_after(body, "property=\"og:image\"", "content")
        .or_else(|| decode_aesir_attr(body))
        .filter(|value| !value.is_empty())
    {
        item.cover = Some(config.absolute_url(&cover));
    }
    item
}

fn fetch_ajax_chapters(manga_key: &str, config: &MadaraConfig) -> Vec<MangaChapter> {
    let manga_url = config
        .absolute_url(manga_key)
        .trim_end_matches('/')
        .to_string();
    let mut chapters = Vec::new();
    for page in 1..=100 {
        let body = fetch_ajax_page(&manga_url, page, config);
        let current = manga::Madara::parse_chapters(&body, manga_key, config)
            .into_iter()
            .filter(|chapter| chapter.key != manga_key)
            .collect::<Vec<_>>();
        if current.is_empty() {
            break;
        }
        chapters.extend(current);
    }
    chapters.into_iter().fold(Vec::new(), push_unique_chapter)
}

fn fetch_ajax_page(manga_url: &str, page: u64, config: &MadaraConfig) -> String {
    manga::Madara::browser_client(config)
        .post(format!("{manga_url}/ajax/chapters?t={page}"))
        .xhr()
        .send_text()
        .unwrap_or_default()
}

fn push_unique_chapter(
    mut chapters: Vec<MangaChapter>,
    chapter: MangaChapter,
) -> Vec<MangaChapter> {
    if !chapters.iter().any(|existing| existing.key == chapter.key) {
        chapters.push(chapter);
    }
    chapters
}

fn parse_pages(body: &str, config: &MadaraConfig) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| {
            decode_aesir_attr(chunk)
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "data-cfsrc"))
                .or_else(|| html::attr(chunk, "src"))
                .or_else(|| html::attr(chunk, "content"))
        })
        .filter(|value| !value.starts_with("data:") && !value.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: config.absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(config.base_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn decode_aesir_attr(chunk: &str) -> Option<String> {
    let encoded = html::attr(chunk, "data-aesir")?;
    let bytes = STANDARD.decode(encoded.trim()).ok()?;
    String::from_utf8(bytes)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="manga__item"><h3 class="post-title"><a href="/all-series/sample/">Sample Kaguya</a></h3><img src="/cover.jpg"></div>
<div class="navigation-ajax"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<head><meta property="og:image" content="/cover-og.jpg"></head>
<h1 class="post-title">Sample Kaguya</h1><div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary">Sample description.</div>
<ul><li class="wp-manga-chapter"><a href="/all-series/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">1 January 2024</span></li></ul>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" data-aesir="L3BhZ2UxLmpwZw=="></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_madara_fixture() {
        let config = config();
        assert_eq!(manga::Madara::parse_listing(LIST_FIXTURE, &config).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE, &config).len(), 1);
        assert_eq!(decode_aesir_attr(PAGES_FIXTURE).as_deref(), Some("/page1.jpg"));
    }
}
