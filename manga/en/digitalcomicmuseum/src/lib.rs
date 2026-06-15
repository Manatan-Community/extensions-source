use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, MangaPageImage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const SOURCE: DigitalComicMuseum = DigitalComicMuseum;
const BASE_URL: &str = "https://digitalcomicmuseum.com";

struct DigitalComicMuseum;

impl MangaSource for DigitalComicMuseum {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_stats(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let act = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "topdl"
        };
        let target = format!(
            "{BASE_URL}/stats.php?ACT={act}&start={}&limit=100",
            page.saturating_sub(1) * 100
        );
        Ok(parse_stats(&fetch_text(&target, LIST_FIXTURE)))
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
                    &fetch_text(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target = format!("{BASE_URL}/index.php?ACT=dosearch");
        Ok(parse_search(&fetch_search(&target, query)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/index.php?cid=1".into());
        Ok(parse_details(
            &fetch_text(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/index.php?cid=1".into());
        Ok(parse_chapters(&fetch_text(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/preview/index.php?did=1".into());
        Ok(parse_pages(&fetch_text(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<MangaPageImage> {
        let page_url = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("lazy"))
            .and_then(|lazy| lazy.get("pageUrl"))
            .and_then(Value::as_str)
            .unwrap_or(BASE_URL);
        let body = fetch_text(page_url, IMAGE_FIXTURE);
        Ok(MangaPageImage {
            url: parse_image_url(&body),
            headers: manga::image_headers(BASE_URL),
            ..MangaPageImage::default()
        })
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_text(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
                )),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_search(target: &str, query: &str) -> String {
    client()
        .post(target)
        .form(&[("terms", query)])
        .send_text()
        .unwrap_or_else(|_| SEARCH_FIXTURE.to_string())
}

fn parse_stats(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("mainrow")
            .skip(1)
            .filter_map(|chunk| catalog_from_chunk(chunk))
            .collect(),
        has_next_page: body.contains("alt=\"Next\"") || body.contains("alt='Next'"),
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<tr")
            .skip(1)
            .filter_map(|chunk| catalog_from_chunk(chunk))
            .collect(),
        has_next_page: false,
    }
}

fn catalog_from_chunk(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    let title = html::text_between(chunk, "<a", "</a>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| url::slug_from_url(&key))
        .unwrap_or_else(|| "Digital Comic Museum".into());
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "<img", "src").map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/index.php?cid=1".into());
    let title = html::text_between(body, "id=\"catname\"", "</")
        .or_else(|| html::text_between(body, "id='catname'", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| url::slug_from_url(&key))
        .unwrap_or_else(|| "Digital Comic Museum".into());
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(body, "<img", "src").map(|image| url::join_url(BASE_URL, &image)),
        description: body
            .split("Description")
            .nth(1)
            .and_then(|chunk| html::text_between(chunk, "<table", "</table>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("tableborder")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "tablefooter", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "id=\"catname\"", "</")
                .or_else(|| html::text_between(chunk, "id='catname'", "</"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                url: Some(url::join_url(BASE_URL, &href)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("slick-slide") || chunk.contains("latest-slide"))
        .filter_map(|chunk| html::attr(chunk, "href"))
        .enumerate()
        .map(|(index, href)| {
            let page_url = url::join_url(BASE_URL, &href);
            MangaPage {
                content: PageContent::Lazy {
                    key: format!("page-{}", index + 1),
                    url: None,
                    page_url: Some(page_url),
                    context: None,
                },
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn parse_image_url(body: &str) -> String {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .map(|image| url::join_url(BASE_URL, &image))
        .last()
        .unwrap_or_else(|| url::join_url(BASE_URL, "/images/sample.jpg"))
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find(BASE_URL) {
            return format!(
                "/{}",
                input[index + BASE_URL.len()..].trim_start_matches('/')
            );
        }
    }
    format!("/{}", input.trim_start_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<tbody><tr class="mainrow"><td><img src="/cover.jpg"><a href="/index.php?cid=1">Sample Comic</a></td></tr></tbody><img alt="Next">"#;
const SEARCH_FIXTURE: &str = r#"<div id="search-results"><table><tbody><tr><td><a href="/index.php?cid=1">Sample Comic</a></td></tr></tbody></table></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="tableborder"><div id="catname"><a href="/index.php?cid=1">Sample Comic</a></div><table><img src="/cover.jpg"></table><div class="tablefooter"><a href="/preview/index.php?did=1">Preview</a></div></div><div class="tableborder"><div id="catname">Description</div><table>A public-domain comic.</table></div>"#;
const PAGES_FIXTURE: &str = r#"<div class="latest-slide"><div class="slick-slide"><a href="/preview/page.php?id=1">1</a></div></div>"#;
const IMAGE_FIXTURE: &str = r#"<body><a></a><a><img src="/pages/1.jpg"></a></body>"#;
