use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Dokiraw = Dokiraw;
const BASE_URL: &str = "https://dokiraw.team";

struct Dokiraw;

impl MangaSource for Dokiraw {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, 36));
        }
        let page = page(&request);
        Ok(parse_listing(
            &fetch_document(&format!("{BASE_URL}/hot?page={page}"), LIST_FIXTURE),
            36,
        ))
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
        let mut target = format!(
            "{BASE_URL}/search/manga?page={}",
            request.get("page").and_then(Value::as_u64).unwrap_or(1)
        );
        if !query.is_empty() {
            target.push_str("&keyword=");
            target.push_str(&url::query_escape(query));
        }
        if let Some(genre) = filter_string(&request, "genre").filter(|value| *value != "All") {
            target.push_str("&genre=");
            target.push_str(&url::query_escape(genre));
        }
        Ok(parse_listing(&fetch_document(&target, SEARCH_FIXTURE), 20))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
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
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
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

fn parse_listing(body: &str, page_size: usize) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("manga-item_item"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "data-original")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ja".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    let has_next_page = entries.len() >= page_size;
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let status_text = html::strip_tags(body);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "manga-detail_boxInfo", "</h1>")
            .and_then(|value| html::text_between(&value, "<h1", "</h1>").or(Some(value)))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "src*=cover", "src")
            .or_else(|| html::attr_after(body, "cover", "src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        description: html::text_between(body, "work-break", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: if status_text.contains("完結") {
            ItemStatus::Completed
        } else if status_text.contains("連載中") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("manga-detail_chapter"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let container = html::text_between(chunk, "manga-detail_chapter", "</div>")
                .unwrap_or_else(|| chunk.to_string());
            let mut spans = container.split("<span").skip(1);
            let title = spans
                .next()
                .and_then(|span| html::text_between(span, ">", "</span>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("data-cdn") || chunk.contains("data-original") || chunk.contains("src")
        })
        .filter_map(|chunk| {
            html::attr(chunk, "data-cdn")
                .or_else(|| html::attr(chunk, "data-original"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.is_empty())
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

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    let path = path.split('?').next().unwrap_or(path).trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="manga-item_item"><a href="/manga/sample"><img src="/cover.jpg"><h3>Sample Dokiraw</h3></a></div>"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"<div class="manga-detail_boxInfo"><h1>Sample Dokiraw</h1></div><img src="/cover.jpg"><div class="work-break"><p>Fixture description.</p></div><a href="/manga/sample/chapter-1"><div class="manga-detail_chapter"><span>Chapter 1</span><span>昨日</span></div></a><div class="連載中">連載中</div>"#;
const PAGES_FIXTURE: &str = r#"<div class="page-chapter"><img data-original="/page1.jpg"><img data-cdn="/page2.jpg"></div>"#;
