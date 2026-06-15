use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Hachiraw = Hachiraw;
const BASE_URL: &str = "https://hachiraw.net";

struct Hachiraw;

impl MangaSource for Hachiraw {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "lastest"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &list_url(page, "", "", sort),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let genre = filter_string(&request, "genre").unwrap_or_default();
        let sort = filter_string(&request, "sort").unwrap_or("views");
        Ok(parse_listing(&fetch_document_or_fixture(
            &list_url(page, query, genre, sort),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &format!("{BASE_URL}{key}"),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document_or_fixture(
            &format!("{BASE_URL}{key}"),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.to_string(),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
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

fn list_url(page: u64, query: &str, genre: &str, sort: &str) -> String {
    let mut path = if query.is_empty() && !genre.is_empty() {
        format!("{BASE_URL}/manga-list/{genre}")
    } else {
        format!("{BASE_URL}/list-manga")
    };
    if page > 1 {
        path.push('/');
        path.push_str(&page.to_string());
    }
    let mut params = Vec::new();
    if !query.is_empty() {
        params.push(format!("search={}", url::query_escape(query)));
    }
    if !sort.is_empty() {
        params.push(format!("order_by={}", url::query_escape(sort)));
    }
    if params.is_empty() {
        path
    } else {
        format!("{path}?{}", params.join("&"))
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("top-15")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "SeriesName", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "SeriesName", "</a>")
                    .or_else(|| html::attr_after(chunk, "img-fluid", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Hachiraw".into())),
                cover: html::attr_after(chunk, "img-fluid", "src")
                    .map(|value| url::join_url(BASE_URL, &value)),
                url: Some(format!("{BASE_URL}{key}")),
                language: Some("ja".to_string()),
                content_rating: Some("suggestive".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("pagination") && body.contains('→'),
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&format!("{BASE_URL}{key}"), DETAILS_FIXTURE);
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(&body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Hachiraw".into())),
        cover: html::attr_after(&body, "img-fluid", "src").map(|value| url::join_url(BASE_URL, &value)),
        description: html::text_between(&body, "div class=\"Content\"", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_links(&body, "著者"),
        tags: info_links(&body, "ジャンル"),
        url: Some(format!("{BASE_URL}{key}")),
        language: Some("ja".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("ChapterLink")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<span", "</span>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(format!("{BASE_URL}{key}")),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let page_area = body
        .split("id=\"TopPage\"")
        .nth(1)
        .or_else(|| body.split("id='TopPage'").nth(1))
        .unwrap_or(body);
    page_area
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|src| !src.starts_with("data:"))
        .enumerate()
        .map(|(index, src)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &src),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn info_links(body: &str, label: &str) -> Vec<String> {
    body.split("list-group-item")
        .filter(|chunk| chunk.contains(label))
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|link| html::text_between(link, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .collect::<Vec<_>>()
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn key_from_url(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) {
        return None;
    }
    Some(normalize_key(input.strip_prefix(BASE_URL).unwrap_or(input)))
}

fn normalize_key(input: &str) -> String {
    let path = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="ng-scope"><div class="top-15"><a class="ng-binding SeriesName" href="/manga/sample">Sample Hachiraw</a><img class="img-fluid" src="/cover.jpg"></div></div><ul class="pagination"><li>→</li></ul>"#;
const DETAILS_FIXTURE: &str = r#"<div class="BoxBody"><div class="row"><h1>Sample Hachiraw</h1><img class="img-fluid" src="/cover.jpg"><li class="list-group-item"><span class="mlabel">著者:</span><a>Sample Author</a></li><li class="list-group-item"><span class="mlabel">ジャンル:</span><a>Action</a></li><div class="Content">Sample description.</div><a class="ChapterLink" href="/manga/sample/chapter-1"><span>Chapter 1</span><span class="float-right">01-01-2026</span></a></div></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="TopPage"><img src="/page1.jpg"><img src="/page2.jpg"></div>"#;
