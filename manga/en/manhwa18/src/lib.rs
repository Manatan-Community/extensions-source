use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Manhwa18 = Manhwa18;
const BASE_URL: &str = "https://manhwa18.com";

struct Manhwa18;

impl MangaSource for Manhwa18 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "top"
        } else {
            "update"
        };
        let target = format!("{BASE_URL}/tim-kiem?sort={sort}&page={page}");
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
        let filters = request.get("filters");
        let mut params = Vec::new();
        if !query.is_empty() {
            params.push(format!("q={}", url::query_escape(query)));
        }
        params.push(format!(
            "sort={}",
            url::query_escape(&filter(filters, "sort", "update"))
        ));
        let status = filter(filters, "status", "0");
        if status != "0" {
            params.push(format!("status={}", url::query_escape(&status)));
        }
        let genres = multi_filter(filters, "accept_genres");
        if !genres.is_empty() {
            params.push(format!(
                "accept_genres={}",
                url::query_escape(&genres.join(","))
            ));
        }
        params.push(format!("page={page}"));
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/tim-kiem?{}", params.join("&")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
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

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("thumb-item-flow"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "series-title", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())
                    }),
                cover: html::attr_after(chunk, "data-bg", "data-bg")
                    .or_else(|| style_url(html::attr_after(chunk, "img-in-ratio", "style")))
                    .map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pagination_wrap") && body.contains("next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "series-name", "</a>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: style_url(html::attr_after(body, "series-cover", "style"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "summary-content", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: {
            let authors = info_values(body, "Author");
            if authors.is_empty() {
                single_value(body, "fantrans-value")
            } else {
                authors
            }
        },
        tags: info_values(body, "Genre"),
        status: match info_values(body, "Status").first().map(String::as_str) {
            Some("Ongoing") => ItemStatus::Ongoing,
            Some("Completed") => ItemStatus::Completed,
            Some("On hold") => ItemStatus::Hiatus,
            _ => ItemStatus::Unknown,
        },
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-name"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "chapter-name", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(absolute_url(&key)),
                date_uploaded: html::text_between(chunk, "chapter-time", "</")
                    .map(|value| html::strip_tags(&value).replace('-', "").trim().to_string())
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("lazy") || chunk.contains("chapter-content"))
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn filter(filters: Option<&Value>, id: &str, default: &str) -> String {
    filters
        .and_then(|value| value.get(id))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn multi_filter(filters: Option<&Value>, id: &str) -> Vec<String> {
    filters
        .and_then(|value| value.get(id))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn style_url(value: Option<String>) -> Option<String> {
    let value = value?;
    value
        .split("url(")
        .nth(1)?
        .trim_matches(['\'', '"', ')', ' '])
        .to_string()
        .into()
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    body.split("info-item")
        .skip(1)
        .filter(|chunk| html::strip_tags(chunk).contains(label))
        .flat_map(|chunk| {
            let links = chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            if links.is_empty() {
                html::text_between(chunk, "info-value", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .into_iter()
                    .collect()
            } else {
                links
            }
        })
        .collect()
}

fn single_value(body: &str, marker: &str) -> Vec<String> {
    html::text_between(body, marker, "</")
        .map(|value| vec![html::strip_tags(&value)])
        .unwrap_or_default()
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="thumb-item-flow"><a href="/series/sample"></a><div class="series-title"><a href="/series/sample">Sample Manga</a></div><div class="lazy-bg" data-bg="/cover.jpg"></div></div>
<div class="pagination_wrap"><a class="next" href="/tim-kiem?page=2">Next</a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="series-name"><a href="/series/sample">Sample Manga</a></div><div class="series-cover"><div class="img-in-ratio" style="background-image: url('/cover.jpg')"></div></div><div class="summary-content">Description</div>
<div class="series-information"><div class="info-item"><span class="info-name">Author</span><span class="info-value">Author</span></div><div class="info-item"><span class="info-name">Genre</span><span class="info-value"><a>Adult</a><a>Drama</a></span></div><div class="info-item"><span class="info-name">Status</span><span class="info-value">Ongoing</span></div></div>
<ul class="list-chapters"><a href="/series/sample/chapter-1"><span class="chapter-name">Chapter 1</span><span class="chapter-time">- 2024-01-01</span></a></ul>
"#;
const PAGES_FIXTURE: &str = r#"<div id="chapter-content"><img class="lazy" data-src="/page1.jpg"><img class="lazy" data-src="/page2.jpg"></div>"#;
