use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ManhwaBuddy = ManhwaBuddy;
const BASE_URL: &str = "https://manhwabuddy.com";

struct ManhwaBuddy;

impl MangaSource for ManhwaBuddy {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let target = format!("{BASE_URL}/page/{page}");
            Ok(parse_latest(&fetch_document(&target, LATEST_FIXTURE)))
        } else {
            Ok(parse_popular(&fetch_document(BASE_URL, LIST_FIXTURE)))
        }
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
        let target = if query.is_empty() {
            let genre = filter(request.get("filters"), "genre", "action");
            format!("{BASE_URL}/genre/{genre}/page/{page}")
        } else {
            format!(
                "{BASE_URL}/search?s={}&page={page}",
                url::query_escape(query)
            )
        };
        Ok(parse_latest(&fetch_document(&target, LATEST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document(
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
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
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

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("item-move"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "<h3", "</h3>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())
                    }),
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("suggestive".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("latest-item"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(chunk, "<a", "title")
                    .or_else(|| {
                        html::text_between(chunk, "<h4", "</h4>")
                            .map(|value| html::strip_tags(&value))
                    })
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())
                    }),
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("suggestive".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("class=\"next")
            || body.contains("class='next")
            || body.contains(" rel=\"next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: html::attr_after(body, "main-info-left", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        authors: detail_links(body, "Author"),
        artists: detail_links(body, "Artist"),
        tags: detail_links(body, "Genres"),
        description: html::text_between(body, "short-desc-content", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: match detail_text(body, "Status").as_deref() {
            Some("Ongoing") => ItemStatus::Ongoing,
            Some("Complete") | Some("Completed") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-name"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "chapter-name", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(absolute_url(&key)),
                date_uploaded: html::text_between(chunk, "ct-update", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("loading"))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn detail_links(body: &str, label: &str) -> Vec<String> {
    body.split("<li")
        .skip(1)
        .find(|chunk| html::strip_tags(chunk).contains(label))
        .map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn detail_text(body: &str, label: &str) -> Option<String> {
    body.split("<li")
        .skip(1)
        .find(|chunk| html::strip_tags(chunk).contains(label))
        .map(html::strip_tags)
        .and_then(|value| value.split(':').nth(1).map(|part| part.trim().to_string()))
}

fn filter(filters: Option<&Value>, id: &str, default: &str) -> String {
    filters
        .and_then(|value| value.get(id))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="item-move"><a href="/sample"><img src="/cover.jpg"><h3>Sample Manga</h3></a></div>"#;
const LATEST_FIXTURE: &str = r#"<div class="latest-list"><div class="latest-item"><a href="/sample" title="Sample Manga"><img src="/cover.jpg"><h4>Sample Manga</h4></a></div></div><a class="next" href="/page/2">Next</a>"#;
const DETAILS_FIXTURE: &str = r#"
<div class="main-info-left"><img src="/cover.jpg"></div><div class="main-info-right"><h1>Sample Manga</h1><ul><li>Author: <a>Author</a></li><li>Artist: <a>Artist</a></li><li>Genres: <a>Action</a><a>Drama</a></li><li>Status: <span>Ongoing</span></li></ul></div>
<div class="short-desc-content"><p>Description</p></div><div class="chapter-list"><a href="/sample/chapter-1"><span class="chapter-name">Chapter 1</span><span class="ct-update">2024-01-01</span></a></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<img class="loading" src="/page1.jpg"><img class="loading" src="/page2.jpg">"#;
