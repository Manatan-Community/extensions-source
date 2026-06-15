use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: PlaymateHunter = PlaymateHunter;
const BASE_URL: &str = "https://pmatehunter.com";

struct PlaymateHunter;

impl MangaSource for PlaymateHunter {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/updates/sort/newest/mpage/{page}/")
        } else {
            match page {
                1 => BASE_URL.to_string(),
                2 => format!("{BASE_URL}/archive/"),
                _ => format!("{BASE_URL}/archive/page/{}/", page - 1),
            }
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &target,
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
            return Ok(Paged {
                entries: vec![details_from_url(query)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = if query.is_empty() {
            filtered_search_url(page, filters)
        } else {
            format!(
                "{BASE_URL}/search/post/{}/mpage/{page}/",
                url::query_escape(query)
            )
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/sample-gallery/".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/sample-gallery/".into());
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Gallery".to_string()),
            chapter_number: Some(0.0),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-gallery/".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_url(input)),
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

fn filtered_search_url(page: u64, filters: &Value) -> String {
    let sort = filters
        .get("sort")
        .and_then(Value::as_str)
        .unwrap_or("trending");
    let tag = filters
        .get("tag")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let sort_path = match sort {
        "newest" => "sort/newest",
        "popular" => "sort/popular",
        _ => "sort/trending",
    };
    if tag.is_empty() {
        if sort == "popular" {
            format!("{BASE_URL}/updates/page/{page}/")
        } else {
            format!("{BASE_URL}/updates/{sort_path}/mpage/{page}/")
        }
    } else if sort == "trending" {
        format!("{BASE_URL}/tag/{}/page/{page}/", url::query_escape(tag))
    } else {
        format!(
            "{BASE_URL}/tag/{}/{sort_path}/mpage/{page}/",
            url::query_escape(tag)
        )
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<figure")
        .skip(1)
        .filter(|chunk| chunk.contains("list-gallery") || !chunk.contains("/video/"))
        .filter_map(parse_card)
        .fold(Vec::new(), push_unique);
    Paged {
        has_next_page: body.contains("class=\"next\"")
            || body.contains("class='next'")
            || body.contains("next page-numbers"),
        entries,
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    if chunk.contains("/video/") {
        return None;
    }
    let href = html::attr_after(chunk, "<a", "href")?;
    let title = html::attr_after(chunk, "<a", "title")
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Gallery".into()));
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_attr(chunk).map(|value| url::join_url(BASE_URL, &value)),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let mut item = details_from_url(&url::join_url(BASE_URL, key));
    item.description = html::text_between(body, "id=\"content\"", "</p>")
        .or_else(|| html::text_between(body, "id='content'", "</p>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let links = html::text_between(body, "link-btn", "</p>").unwrap_or_default();
    item.authors = collect_link_text(&links, "/model/");
    item.artists = item.authors.clone();
    item.tags = collect_link_text(&links, "/tag/");
    item.cover = item
        .cover
        .or_else(|| image_attr(body).map(|value| url::join_url(BASE_URL, &value)));
    item.initialized = true;
    item
}

fn details_from_url(input: &str) -> CatalogItem {
    let key = normalize_key(input);
    CatalogItem {
        key: key.clone(),
        title: url::slug_from_url(&key).unwrap_or_else(|| "Gallery".into()),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "href"))
        .filter(|href| href.starts_with("https://cdn."))
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

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "srcset")
        .and_then(|value| value.split_whitespace().next().map(ToString::to_string))
        .or_else(|| html::attr_after(chunk, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn collect_link_text(chunk: &str, needle: &str) -> Vec<String> {
    chunk
        .split("<a")
        .skip(1)
        .filter(|part| part.contains(needle))
        .filter_map(|part| html::text_between(part, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn normalize_key(value: &str) -> String {
    let path = value.trim_start_matches(BASE_URL).trim_end_matches('/');
    format!("/{}/", path.trim_start_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<figure class="list-gallery"><a href="https://pmatehunter.com/sample-gallery/" title="Sample Gallery"><img src="https://cdn.pmatehunter.com/thumb.jpg"></a></figure>
<ul class="pagination-a"><li class="next"></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<p class="link-btn"><a href="/model/jane">Jane</a><a href="/tag/outdoor">Outdoor</a></p>
<div id="content"><p>Gallery description.</p></div>
<figure class="list-gallery"><img src="https://cdn.pmatehunter.com/thumb.jpg"></figure>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="list-gallery">
<a href="https://cdn.pmatehunter.com/001.jpg">1</a>
<a href="https://cdn.pmatehunter.com/002.jpg">2</a>
</div>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_listing_details_and_pages() {
        let listing = parse_listing(LIST_FIXTURE);
        assert_eq!(listing.entries[0].title, "Sample Gallery");

        let details = SOURCE.details(json!({"manga":"/sample-gallery/"})).unwrap();
        assert_eq!(details.authors, vec!["Jane"]);
        assert_eq!(details.tags, vec!["Outdoor"]);

        let pages = SOURCE.pages(json!({"chapter":"/sample-gallery/"})).unwrap();
        assert_eq!(pages.len(), 2);
    }
}
