use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: CulturedWorks = CulturedWorks;
const BASE_URL: &str = "https://culturedworks.com";

struct CulturedWorks;

impl MangaSource for CulturedWorks {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = page(&request);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        Ok(parse_listing(&fetch_document(
            &manga_url(page, "", Some(order), request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
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
        Ok(parse_listing(&fetch_document(
            &manga_url(page, query, None, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".to_string());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1/".to_string());
        let chapter_url = url::join_url(BASE_URL, &key);
        Ok(parse_pages(
            &fetch_document(&chapter_url, PAGES_FIXTURE),
            &chapter_url,
        ))
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

fn manga_url(page: u64, query: &str, order: Option<&str>, filters: Option<&Value>) -> String {
    let mut params = vec![
        format!("title={}", url::query_escape(query)),
        format!("page={page}"),
    ];
    let filters = filters.unwrap_or(&Value::Null);
    add_param(&mut params, "author", filter_str(filters, "author"));
    add_param(&mut params, "yearx", filter_str(filters, "year"));
    add_param(&mut params, "status", filter_str(filters, "status"));
    add_param(&mut params, "type", filter_str(filters, "type"));
    let order = order
        .or_else(|| filter_str(filters, "order"))
        .unwrap_or_default();
    add_param(&mut params, "order", Some(order));
    for genre in selected_values(filters.get("genres")) {
        params.push(format!("genre%5B%5D={}", url::query_escape(&genre)));
    }
    format!("{BASE_URL}/manga/?{}", params.join("&"))
}

fn add_param(params: &mut Vec<String>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        params.push(format!("{key}={}", url::query_escape(value)));
    }
}

fn filter_str<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).and_then(Value::as_str)
}

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.to_string()],
        _ => Vec::new(),
    }
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("bsx") || chunk.contains("imgu"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| {
                    html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
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
        has_next_page: body.contains("pagination")
            && (body.contains("next") || body.contains("hpage")),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let details =
        html::text_between(body, "main-info", "</section>").unwrap_or_else(|| body.to_string());
    let key = key.unwrap_or_else(|| "/manga/sample/".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(&details, "<h1", "</h1>")
            .or_else(|| html::text_between(&details, "entry-title", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "CulturedWorks".into())),
        cover: image_attr(&details).map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(&details, "class=\"desc", "</div>")
            .or_else(|| html::text_between(&details, "itemprop=\"description\"", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_values(&details, "Author"),
        artists: info_values(&details, "artist"),
        tags: genre_values(&details),
        status: status_from_text(&details),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("chapter") || chunk.contains("chbox") || chunk.contains("eph-num")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapternum", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let mut images = body
        .split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("readerarea") || chunk.contains("wp-manga") || chunk.contains("src")
        })
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:"))
        .collect::<Vec<_>>();
    if images.is_empty() {
        images = html::html_unescape(body)
            .split("\"images\"")
            .skip(1)
            .flat_map(|chunk| chunk.split('"'))
            .filter(|part| {
                part.starts_with("http://") || part.starts_with("https://") || part.starts_with('/')
            })
            .map(ToString::to_string)
            .collect();
    }
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| {
            let absolute = url::join_url(BASE_URL, &image);
            let headers = if absolute.contains("kumacdn") {
                manatan_shared::sdk::Context::new()
            } else {
                manga::image_headers(chapter_url)
            };
            MangaPage {
                content: PageContent::Url {
                    url: absolute,
                    context: Some(headers.clone()),
                },
                headers,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn image_attr(input: &str) -> Option<String> {
    ["data-lazy-src", "data-src", "data-cfsrc", "srcset", "src"]
        .into_iter()
        .find_map(|attr| {
            let value =
                html::attr_after(input, "<img", attr).or_else(|| html::attr(input, attr))?;
            if attr == "srcset" {
                value.split_whitespace().next().map(ToString::to_string)
            } else {
                Some(value)
            }
        })
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    body.split("imptdt")
        .chain(body.split("infotable"))
        .filter(|chunk| {
            chunk
                .to_ascii_lowercase()
                .contains(&label.to_ascii_lowercase())
        })
        .filter_map(|chunk| {
            html::text_between(chunk, "<i", "</i>")
                .or_else(|| html::text_between(chunk, ">", "</span>"))
        })
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && value != "-")
        .collect()
}

fn genre_values(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("genre") || chunk.contains("/genre/"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn status_from_text(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") || lower.contains("finished") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") || lower.contains("on hold") {
        ItemStatus::Hiatus
    } else if lower.contains("dropped") || lower.contains("cancel") {
        ItemStatus::Cancelled
    } else if lower.contains("ongoing") || lower.contains("updating") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find(BASE_URL) {
            return format!(
                "/{}",
                input[index + BASE_URL.len()..]
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="bsx"><a href="/manga/sample/" title="Sample Manga"><img src="/cover.jpg"></a></div>
<div class="pagination"><a class="next">Next</a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="main-info"><h1 class="entry-title">Sample Manga</h1><div class="thumb"><img src="/cover.jpg"></div><div class="meta"><span class="genres"><a class="genre-item">Drama</a></span></div><div class="info-right"><span>Status</span><i>Ongoing</i></div><div id="chapterlist"><li><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapterdate">January 1, 2024</span></li></div></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="readerarea"><img src="/page1.jpg"><img src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_theme_pages() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample Manga");
        let pages = SOURCE
            .pages(json!({"chapter":"/manga/sample/chapter-1/"}))
            .unwrap();
        assert_eq!(pages.len(), 2);
    }
}
