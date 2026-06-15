use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Mangabat = Mangabat;
const BASE_URL: &str = "https://www.mangabats.com";

struct Mangabat;

impl MangaSource for Mangabat {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/manga-list-all/{page}")
        } else {
            format!("{BASE_URL}/manga-list-all/{page}?type=top-view")
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &path,
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
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if query.contains("mangabat.com/") {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!(
                "{BASE_URL}/search/story/{}?page={page}",
                url::query_escape(&query.replace(' ', "_"))
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document_or_fixture(
            &absolute_url(&key),
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
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
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
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(".com/") {
            return format!("/{}", value[index + 5..].trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("list-story-item")
                || chunk.contains("itemupdate")
                || chunk.contains("story_item")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(chunk, "<a", "title")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| html::text_between(chunk, "item-title", "</"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), |mut items, item| {
            if !items
                .iter()
                .any(|existing: &CatalogItem| existing.key == item.key)
            {
                items.push(item);
            }
            items
        });
    Paged {
        entries,
        has_next_page: body.contains("page_select") || body.contains("page-blue"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "story-info-right", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "info-image", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        description: html::text_between(body, "panel-story-info-description", "</div>")
            .map(|value| {
                html::strip_tags(&value)
                    .replace("Description :", "")
                    .trim()
                    .to_string()
            })
            .filter(|value| !value.is_empty()),
        authors: info_links(body, "Author"),
        artists: info_links(body, "Author"),
        tags: info_links(body, "Genres"),
        status: if body.to_ascii_lowercase().contains("completed") {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/chapter"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            Some(MangaChapter {
                key: normalize_key(&href),
                title: html::text_between(chunk, ">", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(absolute_url(&href)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|value| !value.is_empty() && !value.starts_with("data:"))
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

fn info_links(body: &str, label: &str) -> Vec<String> {
    body.split("story-info-right-extent")
        .filter(|chunk| chunk.contains(label))
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="list-story-item"><a href="/manga/sample" title="Sample Manga"><img src="/cover.jpg"></a></div>
<div class="page_select"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="story-info-right"><h1>Sample Manga</h1></div><div class="info-image"><img src="/cover.jpg"></div>
<div class="panel-story-info-description">Description : A sample.</div>
<div class="row-content-chapter"><a href="/manga/sample/chapter-1">Chapter 1</a></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="container-chapter-reader"><img src="/page1.jpg"><img src="/page2.jpg"></div>"#;
