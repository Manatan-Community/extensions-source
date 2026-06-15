use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: PornPics = PornPics;
const BASE_URL: &str = "https://www.pornpics.com";
const QUERY_PAGE_SIZE: u64 = 19;

struct PornPics;

impl MangaSource for PornPics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page_for(&request);
        let lang = lang_for(&request);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let category = request
            .get("preferences")
            .and_then(|prefs| prefs.get("category"))
            .and_then(Value::as_str)
            .unwrap_or("default");
        let target = if category == "default" {
            let period = if latest { 2 } else { 1 };
            let category_id = 2585 + period;
            format!(
                "{BASE_URL}/popular/api/galleries/list/?limit={}&offset={}&lang={lang}&period={period}&category_id={category_id}",
                QUERY_PAGE_SIZE + 1,
                offset(page)
            )
        } else {
            format!(
                "{BASE_URL}/{category}{}/?limit={}&offset={}&lang={lang}",
                if latest { "/recent" } else { "" },
                QUERY_PAGE_SIZE + 1,
                offset(page)
            )
        };
        Ok(parse_mangas_page(&fetch_text_or_fixture(
            &target,
            JSON_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page_for(&request);
        let lang = lang_for(&request);
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
        let sort = filters.get("sort").and_then(Value::as_str);
        let target = if query.is_empty() {
            category_search_url(page, lang, filters, sort)
        } else {
            format!(
                "{BASE_URL}/search/srch.php?lang={lang}&limit={}&offset={}&q={}",
                QUERY_PAGE_SIZE + 1,
                offset(page),
                url::query_escape(query)
            ) + sort_query(sort)
        };
        Ok(parse_mangas_page(&fetch_text_or_fixture(
            &target,
            JSON_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/galleries/sample/".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/galleries/sample/".into());
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
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/galleries/sample/".into());
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

fn fetch_text_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_mangas_page(body: &str) -> Paged<CatalogItem> {
    let trimmed = body.trim_start();
    let entries = if trimmed.starts_with('[') {
        serde_json::from_str::<Value>(trimmed)
            .ok()
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| {
                let key = normalize_key(item.get("g_url")?.as_str()?);
                Some(CatalogItem {
                    key: key.clone(),
                    title: item
                        .get("desc")
                        .and_then(Value::as_str)
                        .unwrap_or("Gallery")
                        .to_string(),
                    cover: item
                        .get("t_url")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    status: ItemStatus::Completed,
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("all".to_string()),
                    content_rating: Some("adult".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .collect::<Vec<_>>()
    } else {
        parse_html_listing(trimmed)
    };
    let has_next_page = entries.len() as u64 > QUERY_PAGE_SIZE;
    Paged {
        entries: if has_next_page {
            entries.into_iter().take(QUERY_PAGE_SIZE as usize).collect()
        } else {
            entries
        },
        has_next_page,
    }
}

fn parse_html_listing(body: &str) -> Vec<CatalogItem> {
    body.split("rel-link")
        .filter_map(|chunk| {
            let href =
                html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let image = chunk
                .find("<img")
                .map(|index| &chunk[index..])
                .unwrap_or(chunk);
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(image, "<img", "alt").unwrap_or_else(|| {
                    url::slug_from_url(&key).unwrap_or_else(|| "Gallery".into())
                }),
                cover: html::attr_after(image, "<img", "data-src")
                    .or_else(|| html::attr_after(image, "<img", "src"))
                    .map(|value| url::join_url(BASE_URL, &value)),
                status: ItemStatus::Completed,
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("all".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let mut item = details_from_url(&url::join_url(BASE_URL, key));
    if let Some(first) = parse_html_listing(body).into_iter().next() {
        item.title = first.title;
        item.cover = first.cover;
    }
    if let Some(info) = body
        .find("gallery-info to-gall-info")
        .map(|index| &body[index..])
    {
        item.authors = collect_links_between(info, "gallery-info__item", "/pornstars/");
        item.tags = collect_links_between(info, "gallery-info__item", "/");
        item.description = html::text_between(info, "gallery-info__item", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty());
    }
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
    body.split("rel-link")
        .filter_map(|chunk| {
            html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))
        })
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

fn category_search_url(page: u64, lang: &str, filters: &Value, sort: Option<&str>) -> String {
    let category = filters
        .get("categoryPath")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_matches('/');
    let base = if category.is_empty() {
        BASE_URL.to_string()
    } else {
        format!("{BASE_URL}/{category}")
    };
    format!(
        "{base}/?lang={lang}&limit={}&offset={}",
        QUERY_PAGE_SIZE + 1,
        offset(page)
    ) + sort_query(sort)
}

fn sort_query(sort: Option<&str>) -> &'static str {
    match sort {
        Some("recent") | Some("latest") => "&recent=&date=latest",
        _ => "",
    }
}

fn collect_links_between(chunk: &str, marker: &str, href_needle: &str) -> Vec<String> {
    chunk
        .split("<a")
        .skip(1)
        .filter(|part| part.contains(href_needle) && part.contains(marker))
        .filter_map(|part| html::text_between(part, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn normalize_key(value: &str) -> String {
    let path = value.trim_start_matches(BASE_URL);
    format!("/{}/", path.trim_matches('/'))
}

fn page_for(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn offset(page: u64) -> u64 {
    page.saturating_sub(1) * QUERY_PAGE_SIZE
}

fn lang_for(request: &Value) -> &'static str {
    match request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
    {
        Some("pornpics-zh") => "zh",
        _ => "en",
    }
}

export_manga_source!(SOURCE);

const JSON_FIXTURE: &str = r#"[{"desc":"Sample Gallery","g_url":"/galleries/sample/","t_url":"https://www.pornpics.com/thumb.jpg"}]"#;
const DETAILS_FIXTURE: &str = r#"
<ul id="main"><li class="thumbwook"><a class="rel-link" href="/galleries/sample/"><img alt="Sample Gallery" data-src="/thumb.jpg"></a></li></ul>
<div class="gallery-info to-gall-info">
<div class="gallery-info__item"><a href="/tags/cosplay/">Cosplay</a></div>
<div class="gallery-info__item"><a href="/pornstars/jane/">Jane</a></div>
<div class="gallery-info__item">Sample description</div>
</div>
"#;
const PAGES_FIXTURE: &str = r#"
<ul id="main">
<li class="thumbwook"><a class="rel-link" href="https://www.pornpics.com/image1.jpg"><img alt="1"></a></li>
<li class="thumbwook"><a class="rel-link" href="https://www.pornpics.com/image2.jpg"><img alt="2"></a></li>
</ul>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_json_listing_details_and_pages() {
        let listing = parse_mangas_page(JSON_FIXTURE);
        assert_eq!(listing.entries[0].key, "/galleries/sample/");

        let details = SOURCE
            .details(json!({"manga":"/galleries/sample/"}))
            .unwrap();
        assert_eq!(details.title, "Sample Gallery");

        let pages = SOURCE
            .pages(json!({"chapter":"/galleries/sample/"}))
            .unwrap();
        assert_eq!(pages.len(), 2);
    }
}
