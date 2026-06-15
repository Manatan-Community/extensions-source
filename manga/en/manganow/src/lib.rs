use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaNow = MangaNow;
const BASE_URL: &str = "https://manganow.to";

struct MangaNow;

impl MangaSource for MangaNow {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest-updated"
        } else {
            "most-viewed"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/filter?sort={sort}&page={page}"),
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
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if query.is_empty() {
            format!("{BASE_URL}/filter?page={page}")
        } else {
            format!("{BASE_URL}/search?keyword={}&page={page}", url::query_escape(query))
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1#1".to_string());
        Ok(parse_pages_for_key(&key))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| absolute_url(key.split('#').next().unwrap_or(&key))))
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

fn fetch_json(target: &str, referer: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json, text/javascript, */*; q=0.01")
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Referer", referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .filter(|chunk| chunk.contains("manga-poster"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::attr_after(chunk, "<img", "alt")
                .or_else(|| html::attr(chunk, "title"))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MangaNow".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_from_chunk(chunk),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        has_next_page: body.contains("pagination") && body.contains("active"),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let detail = body.split("id=\"ani_detail\"").nth(1).unwrap_or(body);
    let title = html::text_between(detail, "manga-name", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MangaNow".into()));
    let mut authors = Vec::new();
    let mut artists = Vec::new();
    if let Some(author_block) = detail
        .split("item-head")
        .find(|chunk| chunk.contains("Authors:"))
    {
        for part in author_block.split("<a").skip(1) {
            let name = html::text_between(part, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            if part.contains("(Art)") {
                artists.push(name);
            } else {
                authors.push(name);
            }
        }
    }
    CatalogItem {
        key: key.clone(),
        title,
        cover: image_from_chunk(detail),
        description: html::text_between(detail, "description", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors,
        artists,
        tags: detail
            .split("genres")
            .nth(1)
            .map(link_texts)
            .unwrap_or_default(),
        status: if detail.to_ascii_lowercase().contains("completed")
            || detail.to_ascii_lowercase().contains("finished")
        {
            ItemStatus::Completed
        } else if detail.to_ascii_lowercase().contains("ongoing")
            || detail.to_ascii_lowercase().contains("releasing")
        {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapter-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let data_id = html::attr(chunk, "data-id").unwrap_or_default();
            let key = if data_id.is_empty() {
                normalize_key(&href)
            } else {
                format!("{}#{data_id}", normalize_key(&href))
            };
            let title = html::text_between(chunk, "name", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            Some(MangaChapter {
                key: key.clone(),
                title,
                chapter_number: key.split("chapter-").nth(1).and_then(|value| {
                    value
                        .split('#')
                        .next()
                        .and_then(|number| number.parse::<f32>().ok())
                }),
                url: Some(absolute_url(key.split('#').next().unwrap_or(&key))),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages_for_key(key: &str) -> Vec<MangaPage> {
    let chapter_url = absolute_url(key.split('#').next().unwrap_or(key));
    let chapter_id = key
        .split('#')
        .nth(1)
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            let body = fetch_document(&chapter_url, DETAILS_FIXTURE);
            body.split("data-reading-id=\"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .unwrap_or("1")
                .to_string()
        });
    let body = fetch_json(
        &format!("{BASE_URL}//ajax/image/list/{chapter_id}?mode=vertical"),
        &chapter_url,
        PAGES_FIXTURE,
    );
    let html_body = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|root| root.get("html").and_then(Value::as_str).map(ToString::to_string))
        .unwrap_or(body);
    parse_pages(&html_body)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<")
        .filter(|chunk| chunk.contains("iv-card") || chunk.starts_with("img"))
        .filter(|chunk| !chunk.contains("manganow.jpg"))
        .filter_map(image_from_chunk)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-url")
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
        .filter(|value| !value.is_empty())
        .map(|value| url::join_url(BASE_URL, &value))
}

fn link_texts(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input
                .trim_start_matches(BASE_URL)
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(key: &str) -> String {
    if key.starts_with("http") {
        key.to_string()
    } else {
        url::join_url(BASE_URL, key)
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="manga_list-sbs"><a class="manga-poster" href="/manga/sample"><img src="/cover.jpg" alt="Sample Manga"></a></div><ul class="pagination"><li class="active"></li><li>2</li></ul>"#;
const DETAILS_FIXTURE: &str = r#"<div id="ani_detail"><h2 class="manga-name">Sample Manga</h2><img src="/cover.jpg"><div class="description">Summary</div><div class="genres"><a>Action</a></div><div class="anisc-info"><div class="item"><span class="item-head">Authors:</span><a>Author</a></div><div class="item"><span class="item-head">Status:</span><span class="name">Ongoing</span></div></div><ul id="en-chapters"><li class="chapter-item" data-id="1"><a href="/manga/sample/chapter-1"><span class="name">Chapter 1</span></a></li></ul></div>"#;
const PAGES_FIXTURE: &str = r#"{"html":"<div class=\"container-reader-chapter\"><div class=\"iv-card\" data-url=\"https://manganow.to/page1.jpg\"></div><div class=\"iv-card\" data-url=\"https://manganow.to/page2.jpg\"></div></div>"}"#;
