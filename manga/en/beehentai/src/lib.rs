use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: BeeHentai = BeeHentai;
const BASE_URL: &str = "https://beehentai.com";

struct BeeHentai;

impl MangaSource for BeeHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updated_at"
        } else {
            "views"
        };
        Ok(parse_search_page(&fetch_document(
            &format!("{BASE_URL}/search?q=&page={page}&sort={sort}"),
            SEARCH_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(normalize_key(query)),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_search_page(&fetch_document(
            &format!(
                "{BASE_URL}/search?q={}&page={}&sort=views",
                url::query_escape(query),
                page(&request)
            ),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let slug = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample");
        let body = fetch_document(
            &format!("{BASE_URL}/api/manga/{slug}/chapters?source=detail"),
            CHAPTERS_FIXTURE,
        );
        let chapters = parse_chapters(&body);
        if chapters.is_empty() {
            Ok(parse_chapters(&fetch_document(
                &url::join_url(BASE_URL, &key),
                DETAILS_FIXTURE,
            )))
        } else {
            Ok(chapters)
        }
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
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
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
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

fn client() -> HttpClient {
    HttpClient::browser()
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

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn parse_search_page(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("book-detailed-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| {
                    html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value))
                })
                .or_else(|| url::slug_from_url(&href))
                .unwrap_or_else(|| "BeeHentai".to_string());
            Some(CatalogItem {
                key: normalize_key(&href),
                title,
                cover: image_from_chunk(chunk)
                    .map(|image| format!("{}#image-request", url::join_url(BASE_URL, &image))),
                description: html::text_between(chunk, "summary", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                tags: link_texts(chunk, "genres"),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        entries,
        has_next_page: body.contains("paginator")
            && body.contains("active")
            && body.contains("rel=\"next\""),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| {
        normalize_key(
            html::attr_after(body, "rel=\"canonical\"", "href")
                .as_deref()
                .unwrap_or("/manga/sample"),
        )
    });
    let title = html::text_between(body, ".detail h1", "</h1>")
        .or_else(|| html::text_between(body, "<h1", "</h1>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| url::slug_from_url(&key))
        .unwrap_or_else(|| "BeeHentai".to_string());
    CatalogItem {
        key: key.clone(),
        title: title.clone(),
        alternate_titles: html::text_between(body, "<h2", "</h2>")
            .map(|value| {
                html::strip_tags(&value)
                    .split([',', ';'])
                    .map(str::trim)
                    .filter(|value| !value.is_empty() && *value != title)
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        cover: image_from_marker(body, "cover")
            .map(|image| format!("{}#image-request", url::join_url(BASE_URL, &image))),
        description: html::text_between(body, "summary", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: meta_links(body, "Authors"),
        tags: meta_links(body, "Genres"),
        status: match meta_text(body, "Status").to_ascii_lowercase().as_str() {
            "ongoing" => ItemStatus::Ongoing,
            "completed" => ItemStatus::Completed,
            "on-hold" | "on hold" => ItemStatus::Hiatus,
            "canceled" | "cancelled" => ItemStatus::Cancelled,
            _ => ItemStatus::Unknown,
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            Some(MangaChapter {
                key: normalize_key(&href),
                title: html::text_between(chunk, "chapter-title", "</")
                    .or_else(|| html::text_between(chunk, "<a", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                date_uploaded: None,
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut images = Vec::new();
    if body.contains("var chapImages = '") && body.contains("var mainServer = \"") {
        let server = body
            .split("var mainServer = \"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_default();
        let prefix = if server.starts_with("//") {
            "https:"
        } else {
            ""
        };
        if let Some(raw) = body
            .split("var chapImages = '")
            .nth(1)
            .and_then(|rest| rest.split('\'').next())
        {
            images.extend(raw.split(',').map(|path| format!("{prefix}{server}{path}")));
        }
    }
    if images.is_empty() {
        images = body
            .split("<img")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("chapter-images")
                    || chunk.contains("chapter-image")
                    || chunk.contains("data-src")
            })
            .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
            .map(|image| image_with_fallback(&image, body))
            .collect();
    }
    images
        .into_iter()
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
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

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        format!("/{}", input.trim_start_matches(BASE_URL).trim_matches('/'))
    } else {
        format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src").or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn image_from_marker(body: &str, marker: &str) -> Option<String> {
    html::attr_after(body, marker, "data-src")
        .or_else(|| html::attr_after(body, marker, "src"))
        .or_else(|| image_from_chunk(body))
}

fn image_with_fallback(image: &str, body: &str) -> String {
    let fallback = body
        .split("this.src='")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .map(|value| {
            if value.starts_with("//") {
                format!("https:{value}")
            } else {
                value.to_string()
            }
        });
    if let Some(fallback) = fallback {
        if image.contains("://s20.") {
            fallback
        } else {
            format!("{image}#{fallback}")
        }
    } else {
        image.to_string()
    }
}

fn link_texts(chunk: &str, marker: &str) -> Vec<String> {
    chunk
        .split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|part| html::text_between(part, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn meta_links(body: &str, label: &str) -> Vec<String> {
    body.split("<p")
        .find(|chunk| chunk.contains(label))
        .map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| {
                    html::strip_tags(&value)
                        .trim_matches(',')
                        .trim()
                        .to_string()
                })
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn meta_text(body: &str, label: &str) -> String {
    body.split("<p")
        .find(|chunk| chunk.contains(label))
        .map(html::strip_tags)
        .map(|text| text.replace(label, "").trim_matches([':', ' ']).to_string())
        .unwrap_or_default()
}

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"
<div class="book-detailed-item"><a href="/manga/sample" title="Sample BeeHentai"></a><img data-src="/cover.jpg"><div class="summary">Sample summary</div><div class="genres"><a>Adult</a></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="detail"><h1>Sample BeeHentai</h1><h2>Sample Alt</h2><div id="cover"><img data-src="/cover.jpg"></div>
<div class="summary"><div class="content">Sample summary.</div></div>
<p><strong>Authors</strong> <a>Bee</a></p><p><strong>Genres</strong> <a>Adult</a></p><p><strong>Status</strong> <a>Ongoing</a></p></div>
<ul id="chapter-list"><li><a href="/manga/sample/chapter-1"><span class="chapter-title">Chapter 1</span></a></li></ul>
"#;
const CHAPTERS_FIXTURE: &str = r#"<li><a href="/manga/sample/chapter-1"><span class="chapter-title">Chapter 1</span></a></li>"#;
const PAGES_FIXTURE: &str =
    r#"<div id="chapter-images"><img data-src="/page1.jpg"><img data-src="/page2.jpg"></div>"#;
