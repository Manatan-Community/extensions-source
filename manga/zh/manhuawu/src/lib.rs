use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Manhuawu = Manhuawu;
const BASE_URL: &str = "https://www.mhua5.com";
const MOBILE_URL: &str = "https://m.mhua5.com";

struct Manhuawu;

impl MangaSource for Manhuawu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let order = if listing(&request) == "latest" {
            "addtime"
        } else {
            "hits"
        };
        let target = page_path(&format!("/category/order/{order}"), page(&request));
        Ok(parse_listing(&fetch(&target)?))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = query(&request);
        if let Some(key) = key_from_url(&query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)?],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = if query.is_empty() {
            let category = filter(filters, "category").unwrap_or("");
            let order = filter(filters, "order").unwrap_or("hits");
            let category = category.trim_matches('/');
            if category.is_empty() {
                page_path(&format!("/category/order/{order}"), page(&request))
            } else {
                page_path(
                    &format!("/category/{category}/order/{order}"),
                    page(&request),
                )
            }
        } else {
            page_path(
                &format!("/search/{}", url::query_escape(&query)),
                page(&request),
            )
        };
        Ok(parse_listing(&fetch(&target)?))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        details_by_key(&key)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        Ok(parse_chapters(&fetch(&absolute(&key))?))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comic/sample/1.html".to_string());
        Ok(parse_pages(&fetch(&absolute(&key))?))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| mobile_absolute(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| mobile_absolute(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)?),
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

fn fetch(target: &str) -> ExtensionResult<String> {
    client().get(target).browser_document().send_text()
}

fn details_by_key(key: &str) -> ExtensionResult<CatalogItem> {
    Ok(parse_details(&fetch(&absolute(key))?, key))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("common-comic-item")
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "comic__title", "href")
                .or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(&href);
            if !key.contains("/comic/") {
                return None;
            }
            let title = title_after(chunk, "comic__title")?;
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "data-original")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| absolute(&image)),
                url: Some(mobile_absolute(&key)),
                language: Some("zh".to_string()),
                content_rating: Some("safe".to_string()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: has_next_page(body),
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let info = body.split("de-info__box").nth(1).unwrap_or(body);
    let title = html::text_between(info, "comic-title", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "漫画屋".to_string());
    let status_text =
        html::text_between(info, "comic-status", "</").map(|value| html::strip_tags(&value));
    CatalogItem {
        key: key.to_string(),
        title,
        cover: html::attr_after(info, "<img", "src").map(|image| absolute(&image)),
        url: Some(mobile_absolute(key)),
        authors: html::text_between(info, "class=\"name\"", "</")
            .or_else(|| html::text_between(info, "class='name'", "</"))
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        tags: tag_texts(info, "comic-status"),
        description: html::text_between(info, "intro-total", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        language: Some("zh".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(status_text.as_deref()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("chapter__list-box")
        .nth(1)
        .unwrap_or(body)
        .split("<li")
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let name = html::text_between(chunk, "<a", "</a")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(MangaChapter {
                key,
                title: Some(name),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .filter_map(|chunk| {
            html::attr(chunk, "data-original")
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| {
            let image = image.trim();
            !image.is_empty() && !image.starts_with("data:")
        })
        .map(|image| MangaPage {
            content: PageContent::Url {
                url: absolute(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            ..MangaPage::default()
        })
        .collect()
}

fn title_after(chunk: &str, marker: &str) -> Option<String> {
    html::text_between(chunk, marker, "</a")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn tag_texts(input: &str, marker: &str) -> Vec<String> {
    html::text_between(input, marker, "</div")
        .map(|chunk| {
            chunk
                .split("</a")
                .filter_map(|part| {
                    html::text_between(part, "<a", "<").map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default() {
        text if text.contains("连载") => ItemStatus::Ongoing,
        text if text.contains("完结") => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn has_next_page(body: &str) -> bool {
    let pagination = body
        .split("id=\"Pagination\"")
        .nth(1)
        .or_else(|| body.split("class=\"NewPages\"").nth(1))
        .unwrap_or(body);
    let hrefs = pagination
        .split("<a")
        .filter_map(|chunk| html::attr(chunk, "href"))
        .collect::<Vec<_>>();
    hrefs.len() >= 2 && hrefs[hrefs.len() - 1] != hrefs[hrefs.len() - 2]
}

fn listing(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing_id"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn query(request: &Value) -> String {
    request
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn filter<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn page_path(path: &str, page: u64) -> String {
    let path = path.trim_end_matches('/');
    if page > 1 {
        absolute(&format!("{path}/page/{page}"))
    } else {
        absolute(path)
    }
}

fn absolute(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn mobile_absolute(value: &str) -> String {
    url::join_url(MOBILE_URL, value)
}

fn normalize_key(input: &str) -> String {
    let value = input.trim();
    let path = value
        .strip_prefix(BASE_URL)
        .or_else(|| value.strip_prefix(MOBILE_URL))
        .unwrap_or(value);
    let path = path.strip_prefix("/index.php").unwrap_or(path);
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn key_from_url(input: &str) -> Option<String> {
    (input.starts_with(BASE_URL) || input.starts_with(MOBILE_URL))
        .then(|| normalize_key(input))
        .filter(|key| key.contains("/comic/"))
}

fn push_unique(mut values: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !values.iter().any(|existing| existing.key == item.key) {
        values.push(item);
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST_FIXTURE: &str = r#"<div class="common-comic-item"><a class="comic__title" href="/comic/sample">Sample Manga</a><img data-original="/cover.jpg"></div><div id="Pagination"><a href="/category/page/1">1</a><a href="/category/page/2">2</a></div>"#;
    const DETAILS_FIXTURE: &str = r#"<div class="de-info__box"><h1 class="comic-title">Sample Manga</h1><img src="/cover.jpg"><span class="name">Author</span><div class="comic-status"><a>Action</a><a>完结</a></div><div class="intro-total">Summary</div></div><ul class="chapter__list-box"><li><a href="/comic/sample/1.html">Chapter 1</a></li><li><a href="/comic/sample/2.html">Chapter 2</a></li></ul>"#;
    const PAGES_FIXTURE: &str = r#"<div class="comic-list"><img data-original="/page1.jpg"><img data-original="//img.example/page2.jpg"></div>"#;

    #[test]
    fn parses_listing() {
        let page = parse_listing(LIST_FIXTURE);
        assert_eq!(page.entries[0].key, "/comic/sample");
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_details_and_chapters() {
        let item = parse_details(DETAILS_FIXTURE, "/comic/sample");
        assert_eq!(item.title, "Sample Manga");
        assert_eq!(item.authors, vec!["Author"]);
        let chapters = parse_chapters(DETAILS_FIXTURE);
        assert_eq!(chapters[0].title.as_deref(), Some("Chapter 2"));
    }

    #[test]
    fn parses_pages() {
        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(
            pages[0].content,
            PageContent::Url {
                url: "https://www.mhua5.com/page1.jpg".to_string(),
                context: Some(manga::image_headers(BASE_URL))
            }
        );
    }
}

export_manga_source!(SOURCE);
