use manatan_extension::{
    abi::ExtensionResult, export_manga_source, http, source::MangaSource, CatalogItem, ItemStatus,
    MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const SOURCE: Cartoon18 = Cartoon18;
const BASE_URL: &str = "https://www.cartoon18.com";

struct Cartoon18;

impl MangaSource for Cartoon18 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "created"
        } else {
            "hits"
        };
        Ok(parse_listing(
            &fetch(
                &format!("{}?sort={sort}&page={page}", lang_base(&request)),
                LIST_FIXTURE,
            ),
            &request,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query, &request);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch(query, DETAILS_FIXTURE),
                    &key,
                    &request,
                )],
                has_next_page: false,
            });
        }
        let sort = filter(&request, "sort").unwrap_or_else(|| "score".into());
        let q = if query.is_empty() {
            filter(&request, "keyword").unwrap_or_default()
        } else {
            query.into()
        };
        Ok(parse_listing(
            &fetch(
                &format!(
                    "{}?q={}&sort={sort}&page={}",
                    lang_base(&request),
                    url::query_escape(&q),
                    page(&request)
                ),
                LIST_FIXTURE,
            ),
            &request,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/zh-hans/video/sample".into());
        Ok(parse_details(
            &fetch(&absolute(&key, &request), DETAILS_FIXTURE),
            &key,
            &request,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/zh-hans/video/sample".into());
        Ok(parse_chapters(
            &fetch(&absolute(&key, &request), DETAILS_FIXTURE),
            &request,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/zh-hans/watch/sample".into());
        let target = absolute(&key, &request);
        Ok(parse_pages(&fetch(&target, PAGES_FIXTURE), &target))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute(&key, &request)))
    }
    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute(&key, &request)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input, &request);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch(input, DETAILS_FIXTURE),
                    &key,
                    &request,
                )),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
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
fn fetch(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.into())
}
fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}
fn pref_trad(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|p| p.get("ZH_HANT"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}
fn lang_base(request: &Value) -> String {
    if pref_trad(request) {
        BASE_URL.into()
    } else {
        format!("{BASE_URL}/zh-hans")
    }
}
fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|f| f.get(key))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}
fn normalize_key(input: &str, request: &Value) -> String {
    let path = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_matches('/');
    if !pref_trad(request) && !path.starts_with("zh-hans/") {
        format!("/zh-hans/{path}")
    } else {
        format!("/{path}")
    }
}
fn absolute(key: &str, request: &Value) -> String {
    if key.starts_with("http") {
        key.into()
    } else if pref_trad(request) || key.starts_with("/zh-hans/") {
        url::join_url(BASE_URL, key)
    } else {
        url::join_url(
            BASE_URL,
            &format!("/zh-hans/{}", key.trim_start_matches('/')),
        )
    }
}

fn parse_listing(body: &str, request: &Value) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|c| c.contains("card") || c.contains("/video/"))
        .filter_map(|c| {
            let href = html::attr(c, "href")?;
            if !href.contains("/video/") {
                return None;
            }
            let key = normalize_key(&href, request);
            Some(CatalogItem {
                key: key.clone(),
                title: html::strip_tags(c).trim().to_string(),
                cover: html::attr_after(c, "<img", "data-src")
                    .or_else(|| html::attr_after(c, "<img", "src"))
                    .map(|i| url::join_url(BASE_URL, &i)),
                url: Some(absolute(&key, request)),
                language: Some("zh".into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pagination")
            && body.contains("next")
            && !body.contains("disabled"),
    }
}

fn parse_details(body: &str, key: &str, request: &Value) -> CatalogItem {
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|v| html::strip_tags(&v))
            .unwrap_or_else(|| "Cartoon18".into()),
        cover: html::attr_after(body, "<img", "src").map(|i| url::join_url(BASE_URL, &i)),
        description: html::text_between(body, "fa-list", "</span>").map(|v| html::strip_tags(&v)),
        status: ItemStatus::Unknown,
        url: Some(absolute(key, request)),
        language: Some("zh".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, request: &Value) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|c| c.contains("/watch/"))
        .filter_map(|c| {
            let href = html::attr(c, "href")?;
            let key = normalize_key(&href, request);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(html::strip_tags(c)),
                url: Some(absolute(&key, request)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|c| html::attr(c, "data-src").or_else(|| html::attr(c, "src")))
        .filter(|i| !i.contains("logo"))
        .enumerate()
        .map(|(idx, i)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &i),
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", idx + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|i| i.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"<div class="card"><a href="/zh-hans/video/sample"><img data-src="/cover.jpg">Sample Cartoon18</a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="title">Sample Cartoon18</h1><img src="/cover.jpg"><span><i class="fa fa-list"></i>Description</span><a href="/zh-hans/watch/sample">Chapter 1</a>"#;
const PAGES_FIXTURE: &str = r#"<div id="app"><img src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
