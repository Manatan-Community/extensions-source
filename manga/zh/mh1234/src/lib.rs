use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: MH1234 = MH1234;
const BASE_URL: &str = "https://m.wmh1234.com";

struct MH1234;

impl MangaSource for MH1234 {
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
        let target = if query.is_empty() {
            let genre = filter(&request, "genre").unwrap_or("0");
            let status = filter(&request, "status").unwrap_or("0");
            let sort = filter(&request, "sort").unwrap_or("id");
            page_path(
                &format!("/category/tags/{genre}/finish/{status}/order/{sort}"),
                page(&request),
            )
        } else {
            page_path(&format!("/search/{}", url::query_escape(&query)), page(&request))
        };
        Ok(parse_listing(&fetch(&target)?))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        details_by_key(&key)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        Ok(parse_chapters(&fetch(&absolute(&key))?))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/comic/sample/1".to_string());
        let target = absolute(&key);
        Ok(parse_pages(&fetch(&target)?, &target))
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
        Ok(manga::request_key(&request, "manga").map(|key| absolute(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute(&key)))
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

fn filter<'a>(request: &'a Value, key: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
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

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| !key.contains("/category/") && !key.contains("/search/"))
}

fn normalize_key(input: &str) -> String {
    let value = input
        .trim()
        .trim_start_matches(BASE_URL)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    format!("/{}", value.trim_start_matches('/'))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("comic-card")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "comic-card__link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "comic-card__title", "</")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "comic-card__image", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| absolute(&image)),
                url: Some(absolute(&key)),
                language: Some("zh".to_string()),
                content_rating: Some("safe".to_string()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("下一页") || body.contains("pagination-wrapper") && body.contains("&gt;"),
    }
}

fn details_by_key(key: &str) -> ExtensionResult<CatalogItem> {
    Ok(parse_details(&fetch(&absolute(key))?, key))
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let meta = body.split("comic-hero__meta").nth(1).unwrap_or_default();
    let mut meta_values = meta
        .split("meta-item")
        .skip(1)
        .map(html::strip_tags)
        .filter(|value| !value.is_empty());
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "漫画".to_string())),
        cover: html::attr_after(body, "comic-hero", "data-src")
            .or_else(|| html::attr_after(body, "comic-hero", "src"))
            .map(|image| absolute(&image)),
        authors: meta_values.next().into_iter().collect(),
        tags: meta_values.next().into_iter().collect(),
        status: match html::text_between(body, "stat-item", "</div>").map(|value| html::strip_tags(&value)) {
            Some(value) if value.contains("完结") => ItemStatus::Completed,
            Some(value) if value.contains("连载") => ItemStatus::Ongoing,
            _ => ItemStatus::Unknown,
        },
        description: html::text_between(body, "comicDesc", "</")
            .map(|value| html::strip_tags(&value).trim_start_matches("介绍:").trim().to_string()),
        url: Some(absolute(key)),
        language: Some("zh".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("chapter-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "chapter-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(absolute(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter);
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("reader-image")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .map(|image| absolute(&image))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_page_paths() {
        assert_eq!(
            page_path("/category/order/hits", 2),
            "https://m.wmh1234.com/category/order/hits/page/2"
        );
    }

    #[test]
    fn parses_reader_images() {
        let pages = parse_pages(
            r#"<img class="reader-image" data-src="/a.jpg">"#,
            "https://m.wmh1234.com/read/1",
        );
        assert_eq!(pages.len(), 1);
    }
}

export_manga_source!(SOURCE);
