use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: KakuseiProject = KakuseiProject;
const BASE_URL: &str = "https://kakuseiproject.org";
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: BASE_URL,
    lang: "pt-BR",
    content_rating: "safe",
    manga_path: "manga",
    popular_url_marker: "post-title",
    use_load_more: true,
    latest_enabled: true,
};

struct KakuseiProject;

impl MangaSource for KakuseiProject {
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
        if query.starts_with(BASE_URL) {
            let key = CONFIG.normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(
                    &manga::Madara::fetch_document_or_fixture(&CONFIG, query, DETAILS_FIXTURE),
                    Some(key),
                    &CONFIG,
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
        Ok(manga::Madara::parse_details(
            &manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &CONFIG.absolute_url(&key),
                DETAILS_FIXTURE,
            ),
            Some(key),
            &CONFIG,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let chapters = fetch_ajax_chapters(&key);
        if !chapters.is_empty() {
            return Ok(chapters);
        }
        Ok(manga::Madara::parse_chapters(
            &manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &CONFIG.absolute_url(&key),
                DETAILS_FIXTURE,
            ),
            &key,
            &CONFIG,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let body = manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.absolute_url(&key),
            PAGES_FIXTURE,
        );
        let pages = parse_kakuseiproject_pages(&body);
        if pages.is_empty() {
            Ok(manga::Madara::parse_pages(&body, &CONFIG))
        } else {
            Ok(pages)
        }
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| CONFIG.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| CONFIG.absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = CONFIG.normalize_manga_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(manga::Madara::parse_details(
                    &manga::Madara::fetch_document_or_fixture(&CONFIG, input, DETAILS_FIXTURE),
                    Some(key),
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
        entries: manga::Madara::parse_listing(body, &CONFIG),
        has_next_page: manga::Madara::has_next_page(body, &CONFIG),
    }
}

fn fetch_ajax_chapters(manga_key: &str) -> Vec<MangaChapter> {
    let manga_url = CONFIG.absolute_url(manga_key);
    let body = manga::Madara::browser_client(&CONFIG)
        .post(format!("{}/ajax/chapters", manga_url.trim_end_matches('/')))
        .form(&[])
        .xhr()
        .send_text()
        .unwrap_or_default();
    if body.contains("wp-manga-chapter") {
        manga::Madara::parse_chapters(&body, manga_key, &CONFIG)
    } else {
        Vec::new()
    }
}

fn parse_kakuseiproject_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("wp-manga-chapter-img")
                || chunk.contains("page-break")
                || chunk.contains("blocks-gallery")
                || chunk.contains("reading-content")
                || chunk.contains("data-src")
                || chunk.contains("data-lazy-src")
                || chunk.contains("src=")
        })
        .filter_map(|chunk| {
            html::attr(chunk, "data-src")
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| {
            let lower = image.to_ascii_lowercase();
            !lower.starts_with("data:") && !lower.contains("avatar") && !lower.contains("logo")
        })
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: CONFIG.absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail">
  <h3 class="post-title"><a href="/manga/sample/">Sample Kakusei Project</a></h3>
  <img src="/cover.jpg">
</div>
<div class="nav-previous"></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="post-title">Sample Kakusei Project</h1>
<div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary">Sample description.</div>
<ul><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Capitulo 1</a><span class="chapter-release-date">01/01/2024</span></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="reading-content">
  <div class="page-break"><img src="/page-1.jpg"></div>
  <li class="blocks-gallery-item"><img data-src="/page-2.jpg"></li>
</div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture() {
        assert_eq!(
            list_from_body(LIST_FIXTURE).entries[0].title,
            "Sample Kakusei Project"
        );
        assert_eq!(parse_kakuseiproject_pages(PAGES_FIXTURE).len(), 2);
    }
}
