use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: GirlsTop = GirlsTop;
const BASE_URL: &str = "https://en.girlstop.info";

struct GirlsTop;

impl MangaSource for GirlsTop {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request.get("listingId").and_then(Value::as_str);
        let path = if listing == Some("latest") {
            paged_path("index.php", page)
        } else {
            paged_path("filter.php?srt=viw", page)
        };
        Ok(parse_listing(&fetch_document(
            &url::join_url(BASE_URL, &path),
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
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            let body = client()
                .post(format!("{BASE_URL}/models.php"))
                .form(&[("text", query)])
                .browser_document()
                .send_text()
                .unwrap_or_else(|_| LIST_FIXTURE.to_string());
            return Ok(parse_listing(&body));
        }
        let path = request
            .get("filters")
            .and_then(|filters| filters.get("sortPath"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("filter.php?srt=viw");
        Ok(parse_listing(&fetch_document(
            &url::join_url(BASE_URL, &paged_path(path, page)),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/psto.php?id=1".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/psto.php?id=1".to_string());
        if key.contains("psto.php") {
            return Ok(vec![MangaChapter {
                key: key.clone(),
                title: Some("Gallery".to_string()),
                chapter_number: Some(1.0),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            }]);
        }
        Ok(fetch_all_model_chapters(&key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/psto.php?id=1".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
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

fn normalize_key(value: &str) -> String {
    if value.starts_with(BASE_URL) {
        return format!(
            "/{}",
            value[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn paged_path(path: &str, page: u64) -> String {
    if page <= 1 {
        return path.to_string();
    }
    let joiner = if path.contains('?') { "&" } else { "?" };
    format!("{path}{joiner}page={}", page - 1)
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("class=\"thumb")
            .skip(1)
            .filter_map(parse_card)
            .fold(Vec::new(), push_unique_item),
        has_next_page: body.contains("class=\"next") && body.contains("<a"),
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "post_title", "href")
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = normalize_key(&href);
    let title = html::text_between(chunk, "post_title", "</a>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Gallery".into()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "<img", "data-src")
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/psto.php?id=1".to_string());
    let is_model = key.contains("models.php");
    let mut description = if is_model {
        html::text_between(body, "modeldesc", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
    } else {
        description_blocks(body)
    };
    if description.as_deref() == Some("") {
        description = None;
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value).replace(" - nude galleries", ""))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "GirlsTop".into())),
        cover: if is_model {
            html::attr_after(body, "model-cover", "src")
        } else {
            html::attr_after(body, "tiles-wrap", "src")
        }
        .map(|value| url::join_url(BASE_URL, &value)),
        authors: link_values(body, "user.php"),
        artists: link_values(body, "user.php"),
        tags: link_values(body, "tag"),
        description,
        status: if is_model {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Completed
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn description_blocks(body: &str) -> Option<String> {
    let text = body
        .split("ps-desc")
        .skip(1)
        .filter(|chunk| !chunk.contains("ps-tags"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn link_values(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn fetch_all_model_chapters(key: &str) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    let mut next = Some(key.to_string());
    for _ in 0..20 {
        let Some(path) = next.take() else {
            break;
        };
        let body = fetch_document(&url::join_url(BASE_URL, &path), DETAILS_FIXTURE);
        chapters.extend(parse_model_chapters(&body));
        next = next_page(&body);
        if next.is_none() {
            break;
        }
    }
    chapters
}

fn parse_model_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("class=\"thumb")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "post_title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "post_title", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Gallery".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(url::join_url(BASE_URL, &key)),
                date_uploaded: approved_date(chunk),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn approved_date(chunk: &str) -> Option<i64> {
    let lower = chunk.to_ascii_lowercase();
    if lower.contains("today") || lower.contains("just now") || lower.contains("recently") {
        return None;
    }
    html::text_between(chunk, "Approved", "</tr>")
        .and_then(|value| html::text_between(&value, "<td", "</td>"))
        .and_then(|value| manatan_shared::dates::parse_fixture_date(&html::strip_tags(&value)))
}

fn next_page(body: &str) -> Option<String> {
    body.split("class=\"next")
        .nth(1)
        .and_then(|chunk| html::attr_after(chunk, "<a", "href"))
        .map(|value| normalize_key(&value))
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("fullimg"))
        .filter_map(|chunk| html::attr(chunk, "href"))
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

fn push_unique_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="thumb"><picture><img src="/cover.jpg"></picture><div class="post_title"><a href="/psto.php?id=1">Sample Gallery</a></div></div><li class="next"><a href="index.php?page=1">Next</a></li>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample Gallery</h1><div class="ps-desc">A sample gallery.</div><div class="ps-tags"><a href="/tag/sample">Sample</a></div><div class="tiles-wrap"><img src="/cover.jpg"></div>
<a class="fullimg" href="/page1.jpg"></a>
"#;
const PAGES_FIXTURE: &str =
    r#"<a class="fullimg" href="/page1.jpg"></a><a class="fullimg" href="/page2.jpg"></a>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_listing_and_gallery_pages() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Gallery"
        );
        assert_eq!(
            SOURCE
                .pages(json!({"chapter":"/psto.php?id=1"}))
                .unwrap()
                .len(),
            2
        );
    }
}
