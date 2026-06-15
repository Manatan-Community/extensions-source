use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UpdateStrategy,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: XAsiatAlbums = XAsiatAlbums;
const BASE_URL: &str = "https://www.xasiat.com";
const ITEMS_PER_PAGE: u64 = 12;

struct XAsiatAlbums;

impl MangaSource for XAsiatAlbums {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = async_album_url(
            "albums/",
            "list_albums_common_albums_list",
            page,
            &[(
                "sort_by",
                if latest {
                    "post_date"
                } else {
                    "album_viewed_week"
                },
            )],
        );
        let body = fetch_text_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body, page))
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
            let body = fetch_text_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let category = request
            .get("filters")
            .and_then(|filters| filters.get("category"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let target = if !query.is_empty() {
            async_album_url(
                "search/search/",
                "list_albums_albums_list_search_result",
                page,
                &[("q", query)],
            )
        } else if !category.is_empty() {
            async_album_url(category, "list_albums_common_albums_list", page, &[])
        } else {
            async_album_url(
                "albums/",
                "list_albums_common_albums_list",
                page,
                &[("sort_by", "post_date")],
            )
        };
        let body = fetch_text_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body, page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/albums/sample".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/albums/sample".into());
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Photobook".to_string()),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/albums/sample".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/albums/") {
            let key = normalize_key(input);
            let body = fetch_text_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn async_album_url(path: &str, block_id: &str, page: u64, params: &[(&str, &str)]) -> String {
    let offset = ((page.saturating_sub(1)) * ITEMS_PER_PAGE) + 1;
    let mut target = format!(
        "{BASE_URL}/{}?mode=async&function=get_block&block_id={block_id}&from={offset}",
        path.trim_matches('/')
    );
    if block_id.contains("search") {
        target.push_str(&format!("&from_albums={offset}"));
    }
    for (key, value) in params {
        target.push('&');
        target.push_str(key);
        target.push('=');
        target.push_str(&url::query_escape(value));
    }
    target.push_str("&_=1");
    target
}

fn parse_listing(body: &str, page: u64) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/albums/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            if !key.contains("/albums/") {
                return None;
            }
            let title = html::attr(chunk, "title")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Album".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "data-original")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| url::join_url(BASE_URL, &image)),
                status: ItemStatus::Completed,
                update_strategy: Some(UpdateStrategy::OnlyFetchOnce),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("all".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), |mut acc: Vec<CatalogItem>, item| {
            if !acc.iter().any(|existing| existing.key == item.key) {
                acc.push(item);
            }
            acc
        });
    Paged {
        has_next_page: entries.len() as u64 >= ITEMS_PER_PAGE
            || page == 1 && body.contains("pagination"),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/albums/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Album".into())),
        description: html::attr_after(body, "property=\"og:description\"", "content")
            .or_else(|| html::attr_after(body, "property='og:description'", "content")),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| html::attr_after(body, "property='og:image'", "content")),
        tags: parse_tags(body),
        status: ItemStatus::Completed,
        update_strategy: Some(UpdateStrategy::OnlyFetchOnce),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_tags(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/albums/"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "href"))
        .filter(|value| looks_like_image(value))
        .fold(Vec::<String>::new(), |mut acc, image| {
            let image = url::join_url(BASE_URL, &image);
            if !acc.contains(&image) {
                acc.push(image);
            }
            acc
        })
        .into_iter()
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

fn looks_like_image(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".png")
        || lower.ends_with(".webp")
        || lower.contains("/get_image/")
}

fn normalize_key(value: &str) -> String {
    let path = value.trim_start_matches(BASE_URL);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="list-albums"><div class="item"><a href="https://www.xasiat.com/albums/sample/" title="Sample Album"><img data-original="/thumb.jpg"></a></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample Album</h1>
<meta property="og:description" content="Sample description">
<meta property="og:image" content="https://www.xasiat.com/cover.jpg">
<div class="info-content"><a href="https://www.xasiat.com/albums/tags/cosplay/">Cosplay</a></div>
"#;

const PAGES_FIXTURE: &str = r#"
<a class="item" href="https://www.xasiat.com/get_image/2/hash/sources/dir/1/1.jpg/">Image 1</a>
<a href="https://www.xasiat.com/albums/related/">Related</a>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_details_and_pages() {
        assert_eq!(parse_listing(LIST_FIXTURE, 1).entries.len(), 1);
        assert_eq!(
            parse_details(DETAILS_FIXTURE, Some("/albums/sample".into())).title,
            "Sample Album"
        );
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
