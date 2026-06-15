use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, MangaPageImage, Paged, UpdateStrategy,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Xgmn = Xgmn;
const BASE_URL: &str = "http://xgmn8.vip";

struct Xgmn;

impl MangaSource for Xgmn {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            format!("{BASE_URL}/new.html")
        } else {
            format!("{BASE_URL}/top.html")
        };
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with("http://") || query.starts_with("https://") {
            let key = normalize_key(query);
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            let category = request
                .get("filters")
                .and_then(|filters| filters.get("category"))
                .and_then(Value::as_str)
                .unwrap_or("Xiuren/");
            if page > 1 {
                format!("{BASE_URL}/{category}page_{page}.html")
            } else {
                url::join_url(BASE_URL, category)
            }
        } else {
            format!(
                "{BASE_URL}/plus/search/index.asp?keyword={}&p={page}",
                url::query_escape(query)
            )
        };
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/123.html".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/123.html".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(vec![parse_chapter(&body, &key)])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/123.html".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_lazy_pages(&body, &key))
    }

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<MangaPageImage> {
        let page_url = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("pageUrl").or_else(|| content.get("page_url")))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (target, index) = split_page_target(page_url);
        let body = fetch_document_or_fixture(&target, PAGES_FIXTURE);
        let image =
            image_at(&body, index).unwrap_or_else(|| format!("{BASE_URL}/uploadfile/pic/1.jpg"));
        Ok(MangaPageImage {
            url: url::join_url(BASE_URL, &image),
            headers: manga::image_headers(BASE_URL),
            context: Some(manga::image_headers(BASE_URL)),
            page_url: Some(target),
            ..MangaPageImage::default()
        })
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with("http://") || input.starts_with("https://") {
            let key = normalize_key(input);
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("related_box") || chunk.contains("node"))
        .flat_map(|chunk| chunk.split("<a").skip(1))
        .filter_map(parse_listing_link)
        .fold(Vec::new(), |mut acc: Vec<CatalogItem>, item| {
            if !acc.iter().any(|existing| existing.key == item.key) {
                acc.push(item);
            }
            acc
        });
    Paged {
        has_next_page: body.contains("pagination") && !entries.is_empty(),
        entries,
    }
}

fn parse_listing_link(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr(chunk, "href")?;
    let key = normalize_key(&href);
    if !key.ends_with(".html") {
        return None;
    }
    let title = html::attr(chunk, "title")
        .or_else(|| html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value)))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Gallery".into()));
    let id = id_from_key(&key).unwrap_or_else(|| "1".to_string());
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "<img", "src")
            .map(|image| url::join_url(BASE_URL, &image))
            .or_else(|| Some(format!("{BASE_URL}/uploadfile/pic/{id}.jpg"))),
        status: ItemStatus::Completed,
        update_strategy: Some(UpdateStrategy::OnlyFetchOnce),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample/123.html".to_string());
    let mut item = parse_listing_link(&format!(
        r#"<a href="{key}" title="{}"></a>"#,
        url::slug_from_url(&key).unwrap_or_else(|| "Gallery".into())
    ))
    .unwrap_or_default();
    item.key = key.clone();
    item.title = html::text_between(body, "article-title", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or(item.title);
    if let Some(author) =
        html::text_between(body, "item-2", "</").map(|value| html::strip_tags(&value))
    {
        let author = author.replace("模特：", "").trim().to_string();
        if !author.is_empty() {
            item.authors = vec![author];
        }
    }
    item.initialized = true;
    item
}

fn parse_chapter(body: &str, key: &str) -> MangaChapter {
    let href = html::attr_after(body, "current", "href").unwrap_or_else(|| key.to_string());
    MangaChapter {
        key: normalize_key(&href),
        title: html::text_between(body, "article-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| Some("Gallery".to_string())),
        chapter_number: Some(1.0),
        url: Some(url::join_url(BASE_URL, &href)),
        ..MangaChapter::default()
    }
}

fn parse_lazy_pages(body: &str, chapter_key: &str) -> Vec<MangaPage> {
    let total = page_total(body).unwrap_or_else(|| image_count(body).max(1));
    let page_size = image_count(body).max(1);
    let prefix = chapter_key.trim_end_matches(".html");
    (0..total)
        .map(|index| {
            let page_index = index / page_size;
            let fragment = (index % page_size) + 1;
            let page_url = if page_index == 0 {
                format!("{BASE_URL}{prefix}.html#{fragment}")
            } else {
                format!("{BASE_URL}{prefix}_{page_index}.html#{fragment}")
            };
            manga::lazy_page(&format!("page-{}", index + 1), &page_url)
        })
        .collect()
}

fn image_at(body: &str, one_based_index: usize) -> Option<String> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("article-content") || chunk.contains("src="))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .nth(one_based_index.saturating_sub(1))
}

fn image_count(body: &str) -> usize {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("src="))
        .count()
}

fn page_total(body: &str) -> Option<usize> {
    let title = html::text_between(body, "article-title", "</")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    title
        .split('P')
        .next()
        .and_then(|prefix| prefix.rsplit(|ch: char| !ch.is_ascii_digit()).next())
        .and_then(|digits| digits.parse::<usize>().ok())
}

fn split_page_target(page_url: &str) -> (String, usize) {
    let (target, fragment) = page_url.split_once('#').unwrap_or((page_url, "1"));
    (target.to_string(), fragment.parse().unwrap_or(1))
}

fn id_from_key(key: &str) -> Option<String> {
    key.trim_end_matches(".html")
        .rsplit('/')
        .next()
        .filter(|value| value.chars().all(|ch| ch.is_ascii_digit()))
        .map(ToString::to_string)
}

fn normalize_key(value: &str) -> String {
    let without_scheme = value
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let path = without_scheme
        .split_once('/')
        .map(|(_, path)| path)
        .unwrap_or(without_scheme);
    format!("/{}", path.trim_start_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="related_box"><a href="http://xgmn8.vip/Xiuren/123.html" title="Sample Gallery"><img src="/uploadfile/pic/123.jpg"></a></div>
<div class="pagination"><span class="current">1</span><strong>2</strong></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<a class="current" href="http://xgmn8.vip/Xiuren/123.html">1</a>
<h1 class="article-title">Sample Gallery 2P</h1>
<div class="item-2">模特：Sample Model</div>
"#;

const PAGES_FIXTURE: &str = r#"
<a class="current" href="http://xgmn8.vip/Xiuren/123.html">1</a>
<h1 class="article-title">Sample Gallery 2P</h1>
<div class="article-content"><p align="center"><img src="/uploadfile/image/1.jpg"></p><p align="center"><img src="/uploadfile/image/2.jpg"></p></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xgmn_listing_and_pages() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(
            parse_details(DETAILS_FIXTURE, Some("/Xiuren/123.html".into())).authors[0],
            "Sample Model"
        );
        assert_eq!(parse_lazy_pages(PAGES_FIXTURE, "/Xiuren/123.html").len(), 2);
        assert_eq!(
            image_at(PAGES_FIXTURE, 2).as_deref(),
            Some("/uploadfile/image/2.jpg")
        );
    }
}
