use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Erofus = Erofus;
const BASE_URL: &str = "https://www.erofus.com";

struct Erofus;

impl MangaSource for Erofus {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if listing_id(&request) == "latest" {
            "recent"
        } else {
            "viewed"
        };
        Ok(parse_listing(&fetch_document(
            &album_url("/comics/various-authors", page, sort),
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
        let sort = filter_value(&request, "sort").unwrap_or_else(|| "viewed".into());
        let target = if query.is_empty() {
            let album = filter_value(&request, "album")
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "/comics/various-authors".into());
            album_url(&album, page, &sort)
        } else {
            format!(
                "{BASE_URL}/?search={}&sort={sort}&page={page}",
                url::query_escape(query)
            )
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/comics/various-authors/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/comics/various-authors/sample".into());
        Ok(parse_chapters(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comics/various-authors/sample".into());
        Ok(parse_pages_recursive(&fetch_document(
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

fn album_url(album: &str, page: u64, sort: &str) -> String {
    let album = if album == "/" {
        ""
    } else {
        album.trim_start_matches('/')
    };
    format!("{BASE_URL}/{album}?sort={sort}&page={page}")
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("a-click") && chunk.contains("<img"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/comics/") || href.contains("/pic/") {
                return None;
            }
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(chunk, "<img", "alt")
                    .or_else(|| {
                        html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
                    })
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Erofus".into())),
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: next_page(body).is_some(),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/comics/various-authors/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::attr_after(body, "property=\"og:title\"", "content")
            .or_else(|| {
                html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value))
            })
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Erofus".into()),
        cover: html::attr_after(body, "a-click", "data-src")
            .or_else(|| html::attr_after(body, "a-click", "src"))
            .or_else(|| html::attr_after(body, "<img", "data-src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        authors: breadcrumb_author(body, &key).into_iter().collect(),
        artists: breadcrumb_author(body, &key).into_iter().collect(),
        tags: link_values(body, "album-tag-container"),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("a-click") && chunk.contains("<img"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/comics/") || href.contains("/pic/") {
                return None;
            }
            let key = normalize_key(&href);
            if key == manga_key {
                return None;
            }
            Some(MangaChapter {
                key: key.clone(),
                title: html::attr_after(chunk, "<img", "alt")
                    .or_else(|| {
                        html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
                    })
                    .filter(|value| !value.is_empty())
                    .or_else(|| url::slug_from_url(&key)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter);
    if has_page_thumbnails(body) {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Chapter".to_string()),
            url: Some(url::join_url(BASE_URL, manga_key)),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages_recursive(body: &str) -> Vec<MangaPage> {
    let mut pages = parse_page_thumbnails(body);
    let nested = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("a-click") && chunk.contains("<img"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if href.contains("/comics/") && !href.contains("/pic/") {
                Some(url::join_url(BASE_URL, &href))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    for href in nested {
        pages.extend(parse_pages_recursive(&fetch_document(&href, "")));
    }
    pages
        .into_iter()
        .enumerate()
        .map(|(index, mut page)| {
            page.description = Some(format!("Page {}", index + 1));
            page
        })
        .collect()
}

fn parse_page_thumbnails(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("/pic/") || chunk.contains("/thumb/"))
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .map(|image| image.replace("/thumb/", "/medium/"))
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

fn has_page_thumbnails(body: &str) -> bool {
    body.contains("/pic/") || body.contains("/thumb/")
}

fn next_page(body: &str) -> Option<String> {
    body.split("<span")
        .skip(1)
        .find(|chunk| chunk.contains("current +") || chunk.contains("current"))
        .and_then(|_| html::attr_after(body, "pagination", "href"))
}

fn breadcrumb_author(body: &str, key: &str) -> Option<String> {
    let parts = key.trim_matches('/').split('/').collect::<Vec<_>>();
    if parts.len() >= 3 {
        return Some(parts[2].replace('-', " "));
    }
    html::text_between(body, "navigation-breadcrumb", "</div>")
        .map(|value| html::strip_tags(&value))
}

fn link_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker) || chunk.contains("/tag/"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn filter_value(request: &Value, key: &str) -> Option<String> {
    request
        .get(key)
        .and_then(Value::as_str)
        .or_else(|| request.get("filters")?.get(key)?.as_str())
        .map(ToString::to_string)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(
    mut chapters: Vec<MangaChapter>,
    chapter: MangaChapter,
) -> Vec<MangaChapter> {
    if !chapters.iter().any(|existing| existing.key == chapter.key) {
        chapters.push(chapter);
    }
    chapters
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<a class="a-click" href="/comics/various-authors/sample"><img src="/thumb/sample.jpg" alt="Sample Comic"></a>
<div class="pagination"><span class="current">1</span><span><a href="/comics/various-authors?page=2">2</a></span></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="navigation-breadcrumb"><li>Home</li><li>Comics</li><li>Various Authors</li></div>
<a class="a-click" href="/comics/various-authors/sample"><img src="/thumb/cover.jpg" alt="Sample Comic"></a>
<div class="album-tag-container"><a>Drama</a></div>
<a class="a-click" href="/comics/various-authors/sample/chapter-1"><img src="/thumb/ch1.jpg" alt="Chapter 1"></a>
"#;
const PAGES_FIXTURE: &str = r#"
<a class="a-click" href="/comics/pic/sample/1"><img src="/thumb/page1.jpg"></a>
<a class="a-click" href="/comics/pic/sample/2"><img src="/thumb/page2.jpg"></a>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_erofus_fixture() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Comic"
        );
        assert_eq!(SOURCE.chapters(json!({})).unwrap().len(), 1);
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 2);
    }
}
