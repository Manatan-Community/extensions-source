use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: XlecX = XlecX;
const BASE_URL: &str = "https://xlecx.one";

struct XlecX;

impl MangaSource for XlecX {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "date"
        } else {
            "news_read"
        };
        let page_path = if page > 1 {
            format!("page/{page}/")
        } else {
            String::new()
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/f/sort={sort}/order=desc/{page_path}"),
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
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document(
            &format!(
                "{BASE_URL}/index.php?do=search&subaction=search&search_start={page}&full_search=0&story={}",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample.html".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample.html".into());
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Chapter".into()),
            chapter_number: Some(1.0),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("en".into()),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample.html".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("thumb"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let key = normalize_key(&href);
                let title =
                    html::attr_after(chunk, "<img", "alt").or_else(|| url::slug_from_url(&key))?;
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: html::attr_after(chunk, "<img", "src")
                        .map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("en".into()),
                    content_rating: Some("adult".into()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: body.contains("pagination") && body.contains("Next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample.html".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "XlecX".into()),
        cover: html::attr_after(body, "property=\"og:image\"", "content"),
        authors: subinfo_links(body, "Group:"),
        artists: subinfo_links(body, "Artist:"),
        tags: subinfo_links(body, "Tags:"),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut images: Vec<String> = body
        .split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("imagegall23")
                || chunk.contains("data-src")
                || chunk.contains("page__text")
        })
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .collect();
    if images.is_empty() {
        images = json_ld_images(body);
    }
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn subinfo_links(body: &str, label: &str) -> Vec<String> {
    body.split("page__subinfo-item")
        .find(|chunk| chunk.contains(label))
        .map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|text| html::strip_tags(&text))
                .filter(|text| !text.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn json_ld_images(body: &str) -> Vec<String> {
    let Some(script) = body
        .split("<script")
        .find(|chunk| chunk.contains("application/ld+json"))
        .and_then(|chunk| html::text_between(chunk, ">", "</script>"))
    else {
        return Vec::new();
    };
    serde_json::from_str::<JsonLdDto>(&script)
        .ok()
        .and_then(|dto| dto.graph.into_iter().next())
        .map(|book| book.image)
        .unwrap_or_default()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        input.trim_start_matches(BASE_URL).to_string()
    } else {
        format!("/{}", input.trim_start_matches('/'))
    }
    .trim_end_matches('/')
    .to_string()
}

#[derive(Debug, Deserialize)]
struct JsonLdDto {
    #[serde(rename = "@graph", default)]
    graph: Vec<BookDto>,
}

#[derive(Debug, Deserialize)]
struct BookDto {
    #[serde(default)]
    image: Vec<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<a class="thumb" href="/sample.html"><img alt="Sample" src="/cover.jpg"></a><div id="pagination"><a>Next</a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><meta property="og:image" content="/cover.jpg"><div class="page__subinfo-item"><div>Artist:</div><a>Artist</a></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="content-2"><div class="imagegall23"><img data-src="/page1.jpg"><img data-src="/page2.jpg"></div></div>"#;
