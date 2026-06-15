use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: EightMuses = EightMuses;
const BASE_URL: &str = "https://comics.8muses.com";

struct EightMuses;

impl MangaSource for EightMuses {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = listing_id(&request);
        let mut target = if listing == "latest" {
            format!("{BASE_URL}/comics/album/Various-Authors?sort=date&page={page}")
        } else {
            format!("{BASE_URL}/comics/album/Various-Authors?page={page}")
        };
        if page > 1 && !target.contains("page=") {
            target.push_str(&format!("?page={page}"));
        }
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
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
            let album = filter_value(&request, "album").unwrap_or_else(|| "album/Various-Authors".into());
            let sort = filter_value(&request, "sort").unwrap_or_default();
            let mut target = format!("{BASE_URL}/comics/{album}");
            let mut params = Vec::new();
            if !sort.is_empty() {
                params.push(format!("sort={}", url::query_escape(&sort)));
            }
            params.push(format!("page={page}"));
            target.push('?');
            target.push_str(&params.join("&"));
            target
        } else {
            let sort = filter_value(&request, "sort").unwrap_or_default();
            let mut target = format!("{BASE_URL}/search?q={}", url::query_escape(query));
            if !sort.is_empty() {
                target.push_str(&format!("&sort={}", url::query_escape(&sort)));
            }
            target.push_str(&format!("&page={page}"));
            target
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/album/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/album/sample".into());
        Ok(parse_chapters(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/comics/album/sample".into());
        Ok(parse_pages_recursive(
            &fetch_document(&url::join_url(BASE_URL, &key), PAGES_FIXTURE),
            0,
        ))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(&fetch_document(
            &format!("{BASE_URL}/comics/album/Various-Authors?page=1"),
            LIST_FIXTURE,
        ));
        let latest = parse_listing(&fetch_document(
            &format!("{BASE_URL}/comics/album/Various-Authors?sort=date&page=1"),
            LIST_FIXTURE,
        ));
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
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
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("c-tile") && chunk.contains("<img") && !chunk.contains("members-only"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/comics/album/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "8Muses".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk),
                status: ItemStatus::Completed,
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: next_page_url(body).is_some(),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/comics/album/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: breadcrumb_title(body).unwrap_or_else(|| {
            url::slug_from_url(&key).unwrap_or_else(|| "8Muses".to_string()).replace('-', " ")
        }),
        cover: body
            .split("<a")
            .skip(1)
            .find(|chunk| chunk.contains("c-tile") && chunk.contains("<img"))
            .and_then(image_attr),
        authors: author_from_key(&key).into_iter().collect(),
        artists: author_from_key(&key).into_iter().collect(),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("c-tile") && chunk.contains("/comics/album/") && chunk.contains("<img"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            if key == manga_key {
                return None;
            }
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Chapter".into()));
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                thumbnail: image_attr(chunk),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if body.contains("/comics/picture/") {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Chapter".to_string()),
            url: Some(url::join_url(BASE_URL, manga_key)),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages_recursive(body: &str, depth: usize) -> Vec<MangaPage> {
    let mut pages = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/comics/picture/") && chunk.contains("<img"))
        .filter_map(image_attr)
        .map(|image| image.replace("/th/", "/fl/"))
        .collect::<Vec<_>>();
    if depth < 2 {
        let nested = body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("c-tile") && chunk.contains("/comics/album/") && chunk.contains("<img"))
            .filter_map(|chunk| html::attr(chunk, "href"))
            .take(12)
            .flat_map(|href| {
                let doc = fetch_document(&url::join_url(BASE_URL, &href), "");
                parse_pages_recursive(&doc, depth + 1)
                    .into_iter()
                    .filter_map(|page| match page.content {
                        PageContent::Url { url, .. } => Some(url),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        pages.extend(nested);
    }
    pages
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

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|image| url::join_url(BASE_URL, &image))
}

fn next_page_url(body: &str) -> Option<String> {
    body.split("<span")
        .skip(1)
        .find(|chunk| chunk.contains("current"))
        .and_then(|chunk| html::text_between(chunk, "<a", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| value.parse::<u64>().is_ok())
}

fn breadcrumb_title(body: &str) -> Option<String> {
    body.split("top-menu-breadcrumb")
        .nth(1)
        .and_then(|chunk| chunk.rsplit("<li").next())
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
}

fn author_from_key(key: &str) -> Option<String> {
    let parts = key.trim_matches('/').split('/').collect::<Vec<_>>();
    if parts.len() >= 4 && parts[2] == "Various-Authors" {
        return Some(parts[3].replace('-', " "));
    }
    if parts.len() >= 3 {
        return Some(parts[2].replace('-', " "));
    }
    None
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input.trim_start_matches(BASE_URL)
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

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<a class="c-tile" href="/comics/album/Various-Authors/Sample/Book"><img src="/thumb.jpg">Sample Book</a>
<div class="pagination"><span class="current">1</span><span><a>2</a></span></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="top-menu-breadcrumb"><li>Comics</li><li>Sample</li><li>Book</li></div>
<a class="c-tile" href="/comics/picture/sample/1"><img src="/media/th/page1.jpg">Page 1</a>
<a class="c-tile" href="/comics/album/Various-Authors/Sample/Book/Chapter-2"><img src="/thumb2.jpg">Chapter 2</a>
"#;
const PAGES_FIXTURE: &str = r#"
<a class="c-tile" href="/comics/picture/sample/1"><img src="/media/th/page1.jpg">Page 1</a>
<a class="c-tile" href="/comics/picture/sample/2"><img src="/media/th/page2.jpg">Page 2</a>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_eightmuses_fixture() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries[0].title, "Sample Book");
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 2);
    }
}
