use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, ImageRequest, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: BarManga = BarManga;
const BASE_URL: &str = "https://archiviumbar.com";

struct BarManga;

impl MangaSource for BarManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE, &config),
                has_next_page: manga::Madara::has_next_page(LIST_FIXTURE, &config),
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.list_url(page, order), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body, &config),
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
            let body = manga::Madara::fetch_document_or_fixture(&config, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key), &config)],
                has_next_page: false,
            });
        }
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.search_url(page, query),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_listing(&body, &config),
            has_next_page: manga::Madara::has_next_page(&body, &config),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key), &config))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_chapters(&body, &key, &config))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config();
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let chapter_url = config.absolute_url(&key);
        let body = manga::Madara::fetch_document_or_fixture(&config, &chapter_url, PAGES_FIXTURE);
        let pages = parse_ajax_pages(&body, &chapter_url);
        if pages.is_empty() {
            return Ok(manga::Madara::parse_pages(&body, &config));
        }
        Ok(pages)
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
        lang: "es",
        content_rating: "adult",
        manga_path: "manga",
        popular_url_marker: "mp-card-title",
        use_load_more: false,
        latest_enabled: true,
    }
}

fn parse_listing(body: &str, config: &MadaraConfig) -> Vec<CatalogItem> {
    let entries = body
        .split("mp-card")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "mp-card-title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = config.normalize_manga_key(&href);
            let title = html::text_between(chunk, "mp-card-title", "</a>")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| config.absolute_url(&value)),
                url: Some(config.absolute_url(&key)),
                language: Some(config.lang.to_string()),
                content_rating: Some(config.content_rating.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        manga::Madara::parse_listing(body, config)
    } else {
        entries
    }
}

fn parse_details(body: &str, key: Option<String>, config: &MadaraConfig) -> CatalogItem {
    let mut item = manga::Madara::parse_details(body, key, config);
    if let Some(title) = html::text_between(body, "breadcrumb", "</ol>")
        .or_else(|| html::text_between(body, "breadcrumb", "</ul>"))
        .and_then(|value| value.rsplit("<a").next().map(ToString::to_string))
        .and_then(|value| html::text_between(&value, "<a", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
    {
        item.title = title;
    }
    item
}

fn parse_ajax_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let Some(tokens_json) = between_after(body, "_tokens", "=", ";") else {
        return Vec::new();
    };
    let Ok(tokens) = serde_json::from_str::<Value>(tokens_json.trim()) else {
        return Vec::new();
    };
    let nonce = quoted_after(body, "nonce:").unwrap_or_default();
    let action = quoted_after(body, "action:").unwrap_or_default();
    let chapter_key = quoted_after(body, "chapterKey:").unwrap_or_default();
    let Some(object) = tokens.as_object() else {
        return Vec::new();
    };
    object
        .iter()
        .filter_map(|(page, token)| token.as_str().map(|token| (page, token)))
        .enumerate()
        .map(|(index, (page, token))| {
            let body = form_body(&[
                ("action", action.as_str()),
                ("token", token),
                ("page", page.as_str()),
                ("nonce", nonce.as_str()),
                ("chapter_key", chapter_key.as_str()),
            ]);
            let mut headers = manga::image_headers(chapter_url);
            headers.insert("Accept".to_string(), "*/*".to_string());
            headers.insert(
                "Content-Type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            );
            headers.insert("Origin".to_string(), BASE_URL.to_string());
            headers.insert("X-Requested-With".to_string(), "XMLHttpRequest".to_string());
            MangaPage {
                content: PageContent::Request {
                    request: ImageRequest {
                        url: format!("{BASE_URL}/wp-admin/admin-ajax.php"),
                        method: Some("POST".to_string()),
                        headers: headers.clone(),
                        body_base64: Some(STANDARD.encode(body.as_bytes())),
                        credentials: Some("include".to_string()),
                        referrer: Some(chapter_url.to_string()),
                        ..ImageRequest::default()
                    },
                },
                headers,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn form_body(values: &[(&str, &str)]) -> String {
    values
        .iter()
        .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn between_after<'a>(
    input: &'a str,
    marker: &str,
    start_marker: &str,
    end_marker: &str,
) -> Option<&'a str> {
    let after_marker = input.get(input.find(marker)? + marker.len()..)?;
    let after_start = after_marker.get(after_marker.find(start_marker)? + start_marker.len()..)?;
    let end = after_start.find(end_marker)?;
    Some(&after_start[..end])
}

fn quoted_after(input: &str, marker: &str) -> Option<String> {
    let rest = input.get(input.find(marker)? + marker.len()..)?;
    let quote_index = rest.find('"').or_else(|| rest.find('\''))?;
    let quote = rest.as_bytes().get(quote_index).copied()? as char;
    let value_start = quote_index + 1;
    let value_rest = rest.get(value_start..)?;
    let value_end = value_rest.find(quote)?;
    Some(value_rest[..value_end].to_string())
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div id="loop-content"><div class="mp-card"><h3 class="mp-card-title"><a href="/manga/sample/">Sample Manga</a></h3><img data-src="/cover.jpg"></div></div>
<div class="nav-previous"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<ol class="breadcrumb"><li><a href="/">Home</a></li><li><a href="/manga/sample/">Sample Manga</a></li></ol>
<div class="summary_image"><img src="/cover.jpg"></div>
<ul class="main version-chap"><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">01/01/2024</span></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"
<div class="manga-reader-container"></div>
<script>
const _tokens = {"1":"tok-one","2":"tok-two"};
window.reader = { nonce: "nonce-one", action: "bar_reader_image", chapterKey: "chapter-one" };
</script>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_ajax_image_requests() {
        let pages = parse_ajax_pages(
            PAGES_FIXTURE,
            "https://archiviumbar.com/manga/sample/chapter-1/",
        );
        assert_eq!(pages.len(), 2);
        assert!(matches!(pages[0].content, PageContent::Request { .. }));
    }
}
