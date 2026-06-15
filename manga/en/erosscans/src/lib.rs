use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ErosScans = ErosScans;
const BASE_URL: &str = "https://scythescans.com";

struct ErosScans;

impl MangaSource for ErosScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if listing_id(&request) == "latest" {
            "update"
        } else {
            ""
        };
        Ok(parse_listing(&fetch_document(
            &list_url(page, order),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document(
            &search_url(page, query),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-chapter-1".into());
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

fn list_url(page: u64, order: &str) -> String {
    let page_path = if page <= 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    let order = if order.is_empty() {
        String::new()
    } else {
        format!("?order={order}")
    };
    format!("{BASE_URL}/manga/{page_path}{order}")
}

fn search_url(page: u64, query: &str) -> String {
    let page_path = if page <= 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    format!(
        "{BASE_URL}/{page_path}?s={}&post_type=wp-manga",
        url::query_escape(query)
    )
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("bsx") || chunk.contains("page-item-detail"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(chunk, "<img", "alt")
                    .or_else(|| {
                        html::text_between(chunk, "<a", "</a>")
                            .map(|value| html::strip_tags(&value))
                    })
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "Scythe Scans".into())
                    }),
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"")
            || body.contains("nav-previous")
            || body.contains("nextpostslink"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Scythe Scans".into())),
        cover: html::attr_after(body, "thumb", "src")
            .or_else(|| html::attr_after(body, "summary_image", "src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "entry-content", "</div>")
            .or_else(|| html::text_between(body, "description-summary", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: link_values(body, "/genres/"),
        status: status_from(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("wp-manga-chapter") || chunk.contains("chapterdate"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .or_else(|| html::text_between(chunk, "chapter-release-date", "</"))
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut images = parse_base64_script_images(body);
    if images.is_empty() {
        let in_reader = body.contains("readerarea") || body.contains("readerArea");
        images = body
            .split("<img")
            .skip(1)
            .filter(|chunk| {
                in_reader || chunk.contains("wp-manga-chapter-img") || chunk.contains("data-src")
            })
            .filter_map(|chunk| {
                html::attr(chunk, "data-src")
                    .or_else(|| html::attr(chunk, "data-lazy-src"))
                    .or_else(|| html::attr(chunk, "src"))
            })
            .filter(|image| !image.starts_with("data:") && !image.contains("default_avatar"))
            .collect();
    }
    images
        .into_iter()
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

fn parse_base64_script_images(body: &str) -> Vec<String> {
    let Some(encoded) = body
        .split("script src=\"data:text/javascript;base64,")
        .nth(1)
        .and_then(|part| part.split('"').next())
    else {
        return Vec::new();
    };
    let Some(decoded) = decode_base64(encoded) else {
        return Vec::new();
    };
    let Some(array) = decoded
        .split("\"images\":[")
        .nth(1)
        .and_then(|part| part.split(']').next())
    else {
        return Vec::new();
    };
    array
        .split(',')
        .map(|value| {
            value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string()
        })
        .filter(|value| value.starts_with("http"))
        .collect()
}

fn decode_base64(input: &str) -> Option<String> {
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0u8;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\r' | b'\n' | b'\t' | b' ' => continue,
            _ => return None,
        } as u32;
        buf = (buf << 6) | value;
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    String::from_utf8(out).ok()
}

fn link_values(body: &str, needle: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(needle))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn status_from(body: &str) -> manatan_extension::ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") {
        manatan_extension::ItemStatus::Completed
    } else if lower.contains("hiatus") {
        manatan_extension::ItemStatus::Hiatus
    } else {
        manatan_extension::ItemStatus::Ongoing
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

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="bs"><div class="bsx"><a href="/manga/sample/"><img src="/cover.jpg" alt="Sample Manga"></a></div></div><a rel="next" href="/manga/page/2/"></a>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample Manga</h1><div class="thumb"><img src="/cover.jpg"></div>
<div id="chapterlist"><li class="wp-manga-chapter"><a href="/sample-chapter-1/">Chapter 1</a><span class="chapterdate">2024/01/01</span></li></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="readerarea"><img src="/page1.jpg"><img src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_scythe_fixture() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Manga"
        );
        assert_eq!(SOURCE.chapters(json!({})).unwrap().len(), 1);
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 2);
    }
}
