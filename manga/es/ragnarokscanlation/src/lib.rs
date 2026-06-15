use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: RagnarokScanlation = RagnarokScanlation;
const BASE_URL: &str = "https://ragnarokscanlation.org";

struct RagnarokScanlation;

impl MangaSource for RagnarokScanlation {
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
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.list_url(page, order), LIST_FIXTURE);
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
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_details(&body, Some(key), &config))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let ajax = fetch_new_endpoint_chapters(&key, &config);
        if !ajax.is_empty() {
            return Ok(ajax);
        }
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_chapters(&body, &key, &config))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config();
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".to_string());
        let chapter_url = config.absolute_url(&key);
        let body = manga::Madara::fetch_document_or_fixture(&config, &chapter_url, PAGES_FIXTURE);
        let reader_pages = parse_reader_knight_pages(&body, &chapter_url, &config);
        if !reader_pages.is_empty() {
            return Ok(reader_pages);
        }
        Ok(manga::Madara::parse_pages(&body, &config))
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

fn config() -> MadaraConfig {
    MadaraConfig {
        base_url: BASE_URL,
        lang: "es",
        content_rating: "safe",
        manga_path: "series",
        popular_url_marker: "post-title",
        use_load_more: true,
        latest_enabled: true,
    }
}

fn fetch_new_endpoint_chapters(manga_key: &str, config: &MadaraConfig) -> Vec<MangaChapter> {
    let manga_url = config
        .absolute_url(manga_key)
        .trim_end_matches('/')
        .to_string();
    let response = manga::Madara::browser_client(config)
        .post(format!("{manga_url}/ajax/chapters"))
        .xhr()
        .send_text()
        .unwrap_or_default();
    if response.trim().is_empty() {
        return Vec::new();
    }
    manga::Madara::parse_chapters(&response, manga_key, config)
}

fn parse_reader_knight_pages(
    body: &str,
    chapter_url: &str,
    config: &MadaraConfig,
) -> Vec<MangaPage> {
    let nonce =
        script_containing(body, "var RK").and_then(|script| extract_json_string(script, "nonce"));
    let Some(nonce) = nonce.filter(|value| !value.is_empty()) else {
        return Vec::new();
    };
    let chapter_id = html::attr_after(body, "wp-manga-current-chap", "data-id").unwrap_or_default();
    let manga_id = html::attr_after(body, "chapter-selection", "data-manga").unwrap_or_default();
    if chapter_id.is_empty() || manga_id.is_empty() {
        return Vec::new();
    }

    let token_body = http::HttpClient::browser()
        .with_referer(format!("{}/", config.base_url.trim_end_matches('/')))
        .with_cookies_for(config.base_url)
        .with_webview_challenge_fallback()
        .post(format!("{}/wp-admin/admin-ajax.php", config.base_url))
        .xhr()
        .form(&[
            ("action", "rk_get_token"),
            ("nonce", &nonce),
            ("chapter_id", &chapter_id),
            ("manga_id", &manga_id),
        ])
        .send_text();
    let Ok(token_body) = token_body else {
        return Vec::new();
    };
    let token_json = json_or_null(&token_body);
    if !token_json
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Vec::new();
    }
    let Some(token) = string_path(&token_json, &["data", "token"]) else {
        return Vec::new();
    };
    let Some(reader_url) = string_path(&token_json, &["data", "reader_url"]) else {
        return Vec::new();
    };
    let reader_body = http::HttpClient::browser()
        .with_origin(config.base_url)
        .with_referer(chapter_url)
        .with_cookies_for(config.base_url)
        .with_webview_challenge_fallback()
        .post(reader_url)
        .form(&[
            ("rt", &token),
            ("chapter_id", &chapter_id),
            ("manga_id", &manga_id),
        ])
        .send_text();
    let Ok(reader_body) = reader_body else {
        return Vec::new();
    };
    reader_body
        .split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("rk-img"))
        .filter_map(|chunk| html::attr(chunk, "src"))
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

fn script_containing<'a>(body: &'a str, marker: &str) -> Option<&'a str> {
    body.split("<script")
        .skip(1)
        .find(|script| script.contains(marker))
}

fn extract_json_string(body: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}\"");
    let start = body.find(&marker)? + marker.len();
    let after_colon = body[start..].find(':').map(|index| start + index + 1)?;
    let rest = body[after_colon..].trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &rest[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn string_path(value: &Value, path: &[&str]) -> Option<String> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    cursor
        .as_str()
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_or_null(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/series/sample/">Sample</a></h3><img src="/cover.jpg"></div>
<div class="no-posts"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<ul><li class="wp-manga-chapter"><a href="/series/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">2024-01-01</span></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"
<script>var RK = {"nonce":"nonce"};</script>
<input id="wp-manga-current-chap" data-id="11">
<select class="chapter-selection" data-manga="22"></select>
<div class="reading-content"><img class="wp-manga-chapter-img rk-img" src="/page1.jpg"></div>
"#;
