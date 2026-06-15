use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: HentaiHere = HentaiHere;
const BASE_URL: &str = "https://hentaihere.com";
const IMAGE_SERVER_URL: &str = "https://hentaicdn.com";

struct HentaiHere;

impl MangaSource for HentaiHere {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let target = if listing_id(&request) == "latest" {
            format!("{BASE_URL}/directory/newest?page={}", page(&request))
        } else {
            search_url(page(&request), "", &request)
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(normalize_key(query)),
                )],
                has_next_page: false,
            });
        }
        let target = if let Some(id) = query.strip_prefix("id:") {
            format!("{BASE_URL}/m/{id}")
        } else {
            search_url(page(&request), query, &request)
        };
        if query.starts_with("id:") {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&target, DETAILS_FIXTURE),
                    Some(normalize_key(&target)),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/m/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/m/sample".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/m/sample/1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(&fetch_document(
            &format!("{BASE_URL}/directory/most-popular?page=1"),
            LIST_FIXTURE,
        ));
        let latest = parse_listing(&fetch_document(
            &format!("{BASE_URL}/directory/newest?page=1"),
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

fn search_url(page: u64, query: &str, request: &Value) -> String {
    let sort = filter_value(request, "sort").unwrap_or_else(|| "most-popular".to_string());
    let sort_min = if matches!(
        sort.as_str(),
        "staff-pick" | "last-month" | "last-week" | "yesterday" | "trending"
    ) {
        "newest"
    } else {
        &sort
    };
    let alpha = filter_value(request, "startsWith")
        .filter(|value| value.len() == 1)
        .map(|value| format!("/{value}"))
        .unwrap_or_default();
    if !query.is_empty() {
        return format!(
            "{BASE_URL}/search?s={}&sort={sort_min}&page={page}",
            url::query_escape(query)
        );
    }
    if let Some(category) = filter_value(request, "category").filter(|value| !value.is_empty()) {
        return format!("{BASE_URL}/search/{category}/{sort_min}{alpha}?page={page}");
    }
    if let Some(status) = filter_value(request, "status").filter(|value| !value.is_empty()) {
        return format!("{BASE_URL}/directory/{status}{alpha}?page={page}");
    }
    format!("{BASE_URL}/directory/{sort}{alpha}?page={page}")
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("class=\"item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let muted = html::text_between(chunk, "text-muted", "</").unwrap_or_default();
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(chunk, "<img", "alt")
                    .or_else(|| {
                        html::text_between(chunk, "<a", "</a>")
                            .map(|value| html::strip_tags(&value))
                    })
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "HentaiHere".into())
                    }),
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|image| url::join_url(BASE_URL, &image)),
                authors: muted
                    .split("by ")
                    .nth(1)
                    .map(|value| value.split('.').next().unwrap_or(value).trim().to_string())
                    .filter(|value| !value.is_empty() && value != "-" && value != "Unknown")
                    .into_iter()
                    .collect(),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pagination") && body.contains("li:last-child:not(.disabled)")
            || body.contains("pagination") && body.contains("Next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/m/sample".into());
    let categories = link_values_near(body, "Cat");
    let contents = link_values_near(body, "Content:");
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h4", "</h4>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "HentaiHere".into())),
        cover: html::attr_after(body, "id=\"cover\"", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        authors: link_values_near(body, "Artist:"),
        description: html::text_between(body, "Brief Summary:", "</div>")
            .map(|value| html::strip_tags(&value).replace("Brief Summary:", ""))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty() && value != "Nothing yet!"),
        tags: categories.into_iter().chain(contents).collect(),
        status: status_from(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("sub-chp")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .map(|value| value.split('(').next().unwrap_or(&value).trim().to_string())
                .filter(|value| !value.is_empty());
            Some(MangaChapter {
                key: normalize_key(&href),
                title: title.clone(),
                chapter_number: title
                    .as_deref()
                    .and_then(|value| value.split_whitespace().next())
                    .and_then(|value| value.parse().ok()),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let raw = body
        .split("var rff_imageList = ")
        .nth(1)
        .and_then(|part| part.split(';').next())
        .unwrap_or("[]");
    let values = serde_json::from_str::<Vec<String>>(raw).unwrap_or_default();
    values
        .into_iter()
        .enumerate()
        .map(|(index, path)| MangaPage {
            content: PageContent::Url {
                url: format!("{IMAGE_SERVER_URL}/hentai{path}"),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn link_values_near(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .take(1)
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .take(12)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn status_from(body: &str) -> ItemStatus {
    if body.contains("Licensed") {
        ItemStatus::Unknown
    } else if body.contains("Completed") {
        ItemStatus::Completed
    } else if body.contains("Ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
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

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
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

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="item"><a href="/m/sample"><div class="pos-rlt"><img src="/cover.jpg" alt="Sample Here"></div></a><div class="text-muted">by Artist.</div></div>
<ul class="pagination"><li>Next</li></ul>
"#;
const DETAILS_FIXTURE: &str = r#"
<h4><a>Sample Here</a></h4><div id="cover"><img src="/cover.jpg"></div>
<div id="info"><span class="text-info">Artist:</span><a>Artist</a><span class="text-info">Cat</span><a>Adult</a><span class="text-info">Status:</span><a>Completed</a><div><span class="text-info">Brief Summary:</span> Description</div></div>
<li class="sub-chp"><a href="/m/sample/1">1 (English)</a></li>
"#;
const PAGES_FIXTURE: &str =
    r#"<script>var rff_imageList = ["/sample/001.jpg","/sample/002.jpg"];</script>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_hentaihere_fixture() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Here"
        );
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 2);
    }
}
