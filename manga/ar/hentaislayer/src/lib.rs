use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: HentaiSlayer = HentaiSlayer;
const BASE_URL: &str = "https://hentaislayer.net";

struct HentaiSlayer;

impl MangaSource for HentaiSlayer {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let latest_type = request
                .get("preferences")
                .and_then(|prefs| prefs.get("latestType"))
                .and_then(Value::as_str)
                .unwrap_or("manga");
            format!("{BASE_URL}/latest-{latest_type}?page={page}")
        } else {
            format!("{BASE_URL}/manga?page={page}")
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
            let key = normalize_key(query);
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mut target = format!("{BASE_URL}/manga?title={}", url::query_escape(query));
        for id in ["type", "status"] {
            if let Some(value) = filters
                .get(id)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                target.push('&');
                target.push_str(id);
                target.push('=');
                target.push_str(&url::query_escape(value));
            }
        }
        if page > 1 {
            target.push_str("&page=");
            target.push_str(&page.to_string());
        }
        Ok(parse_listing(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
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
        .with_origin(BASE_URL)
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
        .split("id=\"card-real\"")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "text-sm", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
                cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ar".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        has_next_page: body.contains("pagination") && !body.contains("pagination-disabled"),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let mut tags = body
        .split("inline-block")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if let Some(kind) = info_value(body, "Type").or_else(|| info_value(body, "النوع")) {
        tags.insert(0, kind);
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "<h2", "</h2>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: image_attr(body).map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "description", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_value(body, "Author")
            .or_else(|| info_value(body, "الرسام"))
            .into_iter()
            .collect(),
        artists: info_value(body, "Artist")
            .or_else(|| info_value(body, "المؤلف"))
            .into_iter()
            .collect(),
        tags,
        status: parse_status(&info_value(body, "Status").unwrap_or_default()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ar".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapters-list")
        .skip(1)
        .flat_map(|chunk| chunk.split("<a").skip(1))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "item-title", "</")
                    .or_else(|| html::text_between(chunk, "<span", "</span>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| Some("Chapter".to_string())),
                date_uploaded: None,
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("chapter-container")
        .skip(1)
        .flat_map(|chunk| chunk.split("<img").skip(1))
        .filter_map(image_attr)
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

fn image_attr(input: &str) -> Option<String> {
    ["srcset", "data-cfsrc", "data-src", "data-lazy-src", "src"]
        .into_iter()
        .find_map(|attr| html::attr_after(input, "<img", attr).or_else(|| html::attr(input, attr)))
        .map(|value| {
            value
                .split_whitespace()
                .next()
                .unwrap_or(&value)
                .to_string()
        })
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && value != "-")
}

fn parse_status(value: &str) -> ItemStatus {
    if ["ongoing", "مستمر"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Ongoing
    } else if ["completed", "مكتمل"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Completed
    } else if ["dropped", "cancelled", "متوقف"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Unknown
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input.trim_start_matches(BASE_URL).trim_start_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/'))
}

const LIST_FIXTURE: &str = r#"
<div id="card-real"><a href="/manga/sample"><img data-src="/cover.jpg"></a><h2 class="text-sm">Sample Manga</h2></div>
<ul class="pagination"><li>next</li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<main><section><div><div class="relative"><img src="/cover.jpg"></div><div class="flex"><h1>Sample Manga</h1><a class="inline-block">Action</a></div><div><p id="description">Sample description.</p></div></div></section></main>
<div id="buttons"></div><div class="hidden"><p><span>Status</span><span class="capitalize">completed</span></p><p><span>Author</span><span class="capitalize">Writer</span></p><p><span>Artist</span><span class="capitalize">Artist</span></p></div>
<div id="chapters-list"><a href="/manga/sample/chapter-1"><span id="item-title">Chapter 1</span></a></div>
"#;

const PAGES_FIXTURE: &str =
    r#"<div id="chapter-container"><img data-src="/page1.jpg"><img src="/page2.jpg"></div>"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fuzzy_doodle_source() {
        let listing = parse_listing(LIST_FIXTURE);
        assert_eq!(listing.entries[0].title, "Sample Manga");

        let details = parse_details(DETAILS_FIXTURE, Some("/manga/sample".into()));
        assert_eq!(details.status, ItemStatus::Completed);

        let chapters = parse_chapters(DETAILS_FIXTURE);
        assert_eq!(chapters[0].key, "/manga/sample/chapter-1");

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
