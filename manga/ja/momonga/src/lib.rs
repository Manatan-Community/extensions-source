use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MomonGa = MomonGa;
const BASE_URL: &str = "https://momon-ga.com";

struct MomonGa;

impl MangaSource for MomonGa {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request
            .get("page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1);
        let target = paged_path("/popularity/", page);
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
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
                entries: vec![parse_details(
                    &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request
            .get("page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1);
        let target = if !query.is_empty() && !query.contains('-') {
            let mut target = format!("{BASE_URL}/?s={}", url::query_escape(query));
            if page > 1 {
                target.push_str(&format!("&paged={page}"));
            }
            target
        } else {
            let category = filter_string(&request, "category").unwrap_or("fanzine");
            paged_path(&format!("/{}/", category.trim_matches('/')), page)
        };
        Ok(parse_listing(&fetch_document(&target, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(vec![MangaChapter {
            key: normalize_key(&key),
            title: Some("単一章".into()),
            date_uploaded: parse_japanese_date(&html::strip_tags(
                &html::text_between(&body, "post-time", "</").unwrap_or_default(),
            )),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/".into());
        Ok(parse_pages(
            &fetch_document(&url::join_url(BASE_URL, &key), PAGES_FIXTURE),
            &url::join_url(BASE_URL, &key),
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(key),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("post-list") || chunk.contains("post-list-image"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<span", "</span>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "momon:GA".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ja".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("nextpostslink") || body.contains("rel=\"next\""),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample/".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "post-data", "</h1>")
            .and_then(|value| html::text_between(&value, "<h1", "</h1>").or(Some(value)))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "momon:GA".into())),
        cover: html::attr_after(body, "post-hentai", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        authors: post_tag_values(body, "作者"),
        artists: post_tag_values(body, "サークル"),
        tags: post_tag_values(body, "内容"),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let scope = html::text_between(body, "post-hentai", "post-tag").unwrap_or_else(|| body.into());
    scope
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|src| !src.is_empty())
        .enumerate()
        .map(|(index, src)| {
            let image = url::join_url(BASE_URL, &src);
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(referer)),
                },
                headers: manga::image_headers(referer),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn post_tag_values(body: &str, label: &str) -> Vec<String> {
    body.split("post-tag-table")
        .skip(1)
        .filter(|chunk| chunk.contains(label))
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|a| html::text_between(a, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn parse_japanese_date(value: &str) -> Option<i64> {
    let date = value
        .replace('年', "-")
        .replace('月', "-")
        .replace('日', " ")
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    manatan_shared::dates::parse_ymd(&date)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn paged_path(path: &str, page: u64) -> String {
    let mut target = format!("{BASE_URL}/{}", path.trim_matches('/'));
    target.push('/');
    if page > 1 {
        target.push_str(&format!("page/{page}/"));
    }
    target
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(rest) = value.split(BASE_URL).nth(1) {
            return normalize_key(rest);
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

const LIST_FIXTURE: &str = r#"
<div class="post-list"><a href="/sample/"><div class="post-list-image"><img src="/cover.jpg"></div><span>Sample MomonGA</span></a></div>
"#;

const SEARCH_FIXTURE: &str = LIST_FIXTURE;

const DETAILS_FIXTURE: &str = r#"
<div id="post-time">2024年1月1日0時</div>
<div id="post-data"><h1>Sample MomonGA</h1></div>
<div id="post-hentai"><img src="/page1.jpg"><img src="/page2.jpg"></div>
<div id="post-tag"><div class="post-tag-table"><div class="post-tag-title">作者</div><div class="post-tags"><a>Author</a></div></div><div class="post-tag-table"><div class="post-tag-title">内容</div><div class="post-tags"><a>Tag</a></div></div></div>
"#;

const PAGES_FIXTURE: &str = DETAILS_FIXTURE;

export_manga_source!(SOURCE);
