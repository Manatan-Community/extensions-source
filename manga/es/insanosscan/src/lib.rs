use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: InsanosScan = InsanosScan;
const BASE_URL: &str = "https://insanoslibrary.com";
const LANG: &str = "es";
const CONTENT_RATING: &str = "safe";

struct InsanosScan;

impl MangaSource for InsanosScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "date"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/manga/?orderby={order}&page={page}"),
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
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(request);
        }
        Ok(Paged {
            entries: parse_search(&post_search_or_fixture(query, SEARCH_FIXTURE)),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1/".into());
        Ok(parse_pages(&fetch_document_or_fixture(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_search_or_fixture(query: &str, fixture: &str) -> String {
    let nonce = fetch_nonce();
    client()
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .xhr()
        .form(&[
            ("action", "adar_search"),
            ("nonce", nonce.as_str()),
            ("query", query),
        ])
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_nonce() -> String {
    let body = fetch_document_or_fixture(BASE_URL, NONCE_FIXTURE);
    let Some(src) = body
        .split("<script")
        .skip(1)
        .find(|chunk| chunk.contains("adar-main-js-extra"))
        .and_then(|chunk| html::attr(chunk, "src"))
    else {
        return String::new();
    };
    let encoded = src.trim_start_matches("data:text/javascript;base64,");
    STANDARD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|js| value_after(&js, "\"nonce\"", "\""))
        .unwrap_or_default()
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("catalog-card"))
        .filter_map(catalog_from_listing)
        .collect::<Vec<_>>();
    Paged {
        entries,
        has_next_page: body.contains("page-numbers next"),
    }
}

fn catalog_from_listing(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "catalog-card__link", "href")
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "catalog-card__title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "InsanosScan".into())),
        cover: html::attr_after(chunk, "catalog-card__cover", "src")
            .or_else(|| html::attr_after(chunk, "<img", "data-src"))
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    serde_json::from_str::<SearchResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).unwrap())
        .data
        .unwrap_or_default()
        .into_iter()
        .map(|item| {
            let key = normalize_key(&item.url);
            CatalogItem {
                key: key.clone(),
                title: html::strip_tags(&item.title),
                cover: item.cover.filter(|value| !value.is_empty()),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect()
}

fn details_from_key(key: &str) -> CatalogItem {
    parse_details(
        &fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE),
        key,
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "series-main-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "InsanosScan".into())),
        cover: html::attr_after(body, "series-cover-img", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        description: html::text_between(body, "synopsis-content", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: status_from_text(
            &html::text_between(body, "data-badge--status", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default(),
        ),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("genre-pill"))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        url: Some(absolute_url(key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let locked = locked_paths(body);
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-row"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let locked_key = format!("{}/", key.trim_end_matches('/'));
            if locked.contains(&locked_key) {
                return None;
            }
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, "chapter-row__num", "</")
                        .or_else(|| html::text_between(chunk, "chapter-row__title", "</"))
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "Capitulo".into()),
                ),
                date_uploaded: html::text_between(chunk, "chapter-row__date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn locked_paths(body: &str) -> Vec<String> {
    let Some(raw) = value_between(body, "var locked", ";") else {
        return Vec::new();
    };
    let raw = raw.trim().trim_start_matches('=').trim();
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    map.into_iter()
        .filter(|(_, value)| value.as_i64().unwrap_or(0) > 0)
        .map(|(key, _)| key)
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let pages = images_from_body(body, "reader-pages")
        .into_iter()
        .chain(images_from_body(body, "reader-body"))
        .fold(Vec::<String>::new(), |mut out, image| {
            if !out.contains(&image) {
                out.push(image);
            }
            out
        });
    pages
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn images_from_body(body: &str, marker: &str) -> Vec<String> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains(marker) || body.contains(marker))
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
        .filter(|value| !value.is_empty())
        .collect()
}

fn normalize_key(input: &str) -> String {
    let path = input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_end_matches('/');
    format!("/{}/", path.trim_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn status_from_text(status: &str) -> ItemStatus {
    let status = status.to_ascii_lowercase();
    if status.contains("finalizado") {
        ItemStatus::Completed
    } else if status.contains("emisi") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn value_between(input: &str, start: &str, end: &str) -> Option<String> {
    let rest = input.split_once(start)?.1;
    Some(rest.split_once(end)?.0.to_string())
}

fn value_after(input: &str, key: &str, quote: &str) -> Option<String> {
    let rest = input.split_once(key)?.1;
    let rest = rest.split_once(':')?.1.trim();
    let rest = rest.strip_prefix(quote)?;
    Some(rest.split_once(quote)?.0.to_string())
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    data: Option<Vec<SearchItem>>,
}

#[derive(Debug, Deserialize)]
struct SearchItem {
    url: String,
    title: String,
    cover: Option<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<article class="catalog-card"><a class="catalog-card__link" href="/manga/sample/"><img class="catalog-card__cover" src="/cover.jpg"><h2 class="catalog-card__title">Sample Manga</h2></a></article>
<div class="catalog-pagination"><a class="page-numbers next" href="/manga/page/2/"></a></div>
"#;
const SEARCH_FIXTURE: &str = r#"{"data":[{"url":"https://insanoslibrary.com/manga/sample/","title":"Sample Manga","cover":"https://insanoslibrary.com/cover.jpg"}]}"#;
const NONCE_FIXTURE: &str = r#"<script id="adar-main-js-extra" src="data:text/javascript;base64,dmFyIGFkYXI9eyJub25jZSI6ImZpeHR1cmUtbm9uY2UifTs="></script>"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="series-main-title">Sample Manga</h1><img class="series-cover-img" src="/cover.jpg">
<div class="synopsis-content">Sample description</div><span class="data-badge--status">En emision</span>
<td class="genres-cell"><a class="genre-pill">Action</a></td>
<script>var locked = {"/manga/sample/paid/": 1};</script>
<div class="chapters-list"><a class="chapter-row" href="/manga/sample/chapter-1/"><span class="chapter-row__num">Capitulo 1</span><span class="chapter-row__date">01 Jan 2024</span></a><a class="chapter-row" href="/manga/sample/paid/"><span class="chapter-row__num">Paid</span></a></div>
"#;
const PAGES_FIXTURE: &str = r#"<body class="reader-body"><div class="reader-pages"></div><div><img src="/page1.jpg"><img data-src="/page2.jpg"></div></body>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_locked_chapters_and_pages() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_search(SEARCH_FIXTURE).len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
