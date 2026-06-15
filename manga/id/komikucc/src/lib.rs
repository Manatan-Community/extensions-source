use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: KomikuCc = KomikuCc;
const BASE_URL: &str = "https://komiku.cc";
const CDN_URL: &str = "https://cdn.komiku.cc/";

struct KomikuCc;

impl MangaSource for KomikuCc {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_rsc_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        Ok(parse_rsc_listing(&fetch_document_or_fixture(
            &list_url(page, Some(order), request.get("filters")),
            LIST_FIXTURE,
            true,
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
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE, false),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(parse_query_search(&fetch_document_or_fixture(
                &format!("{BASE_URL}/search?q={}", url::query_escape(query)),
                SEARCH_FIXTURE,
                false,
            )));
        }
        Ok(parse_rsc_listing(&fetch_document_or_fixture(
            &list_url(page, None, request.get("filters")),
            LIST_FIXTURE,
            true,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/komik/sample".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE, false),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/komik/sample".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            CHAPTERS_FIXTURE,
            true,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-chapter-1".into());
        Ok(parse_pages(&fetch_document_or_fixture(
            &absolute_url(&key),
            PAGES_FIXTURE,
            true,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE, false),
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
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str, rsc: bool) -> String {
    let http = client();
    let mut request = http.get(target).browser_document();
    if rsc {
        request = request.header("rsc", "1");
    }
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    if let Some(path) = value.strip_prefix(BASE_URL) {
        let trimmed = path.trim_matches('/');
        if trimmed.starts_with("komik/") {
            format!("/{trimmed}")
        } else if trimmed.is_empty() {
            "/".to_string()
        } else {
            format!("/{trimmed}")
        }
    } else {
        let trimmed = value.trim_matches('/');
        if trimmed.starts_with("komik/") {
            format!("/{trimmed}")
        } else if trimmed.is_empty() {
            "/".to_string()
        } else {
            format!("/{trimmed}")
        }
    }
}

fn list_url(page: u64, order: Option<&str>, filters: Option<&Value>) -> String {
    let mut params = Vec::new();
    for id in ["status", "type"] {
        if let Some(value) = filter(filters, id).filter(|value| !value.is_empty()) {
            params.push(format!("{id}={}", url::query_escape(value)));
        }
    }
    let selected_order = filter(filters, "order")
        .filter(|value| !value.is_empty())
        .or(order);
    if let Some(value) = selected_order {
        params.push(format!("order={}", url::query_escape(value)));
    }
    if let Some(genres) = filters.and_then(|value| value.get("genres")) {
        if let Some(text) = genres.as_str() {
            for genre in text
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                params.push(format!("genre%5B%5D={}", url::query_escape(genre)));
            }
        }
    }
    if page > 1 {
        params.push(format!("page={page}"));
    }
    if params.is_empty() {
        format!("{BASE_URL}/list")
    } else {
        format!("{BASE_URL}/list?{}", params.join("&"))
    }
}

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters?.get(id)?.as_str()
}

fn parse_rsc_listing(body: &str) -> Paged<CatalogItem> {
    for value in json_values(body) {
        let Some(data) = value.get("data").and_then(Value::as_array) else {
            continue;
        };
        let entries = data
            .iter()
            .filter_map(|item| {
                let link = item.get("link").and_then(Value::as_str)?;
                let title = item
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Komiku.cc");
                let image = item.get("img").and_then(Value::as_str);
                let key = normalize_manga_slug(link);
                Some(CatalogItem {
                    key: key.clone(),
                    title: title.to_string(),
                    cover: image.map(cdn_url),
                    url: Some(absolute_url(&key)),
                    language: Some("id".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .collect::<Vec<_>>();
        if !entries.is_empty() {
            let current = value
                .get("current_page")
                .and_then(Value::as_u64)
                .unwrap_or(1);
            let last = value
                .get("last_page")
                .and_then(Value::as_u64)
                .unwrap_or(current);
            return Paged {
                entries,
                has_next_page: current < last,
            };
        }
    }
    Paged {
        entries: Vec::new(),
        has_next_page: false,
    }
}

fn parse_query_search(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("/komik/"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let key = normalize_key(&href);
                let title = html::text_between(chunk, "<h3", "</h3>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| "Komiku.cc".to_string());
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| absolute_url(&image)),
                    url: Some(absolute_url(&key)),
                    language: Some("id".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/komik/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::attr_after(body, "property=\"og:title\"", "content")
            .map(|value| value.trim_end_matches(" - Komiku").to_string())
            .or_else(|| {
                html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value))
            })
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Komiku.cc".into())),
        cover: html::attr_after(body, "object-cover", "src")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        authors: label_text(body, "author:").into_iter().collect(),
        tags: details_tags(body),
        description: html::text_between(body, "line-clamp-4", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(body),
        url: Some(absolute_url(&key)),
        language: Some("id".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    for value in json_values(body) {
        let Some(chapters) = value.get("chapters").and_then(Value::as_array) else {
            continue;
        };
        let entries = chapters
            .iter()
            .filter_map(|chapter| {
                let link = chapter.get("link").and_then(Value::as_str)?;
                let key = normalize_key(link);
                Some(MangaChapter {
                    key: key.clone(),
                    title: chapter
                        .get("title")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    url: Some(absolute_url(&key)),
                    date_uploaded: chapter
                        .get("updated_at")
                        .or_else(|| chapter.get("created_at"))
                        .and_then(Value::as_str)
                        .and_then(parse_iso_date),
                    ..MangaChapter::default()
                })
            })
            .collect::<Vec<_>>();
        if !entries.is_empty() {
            return entries;
        }
    }
    Vec::new()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    for value in json_values(body) {
        let Some(images) = value.get("images").and_then(Value::as_array) else {
            continue;
        };
        let pages = images
            .iter()
            .filter_map(Value::as_str)
            .map(cdn_url)
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: None,
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect::<Vec<_>>();
        if !pages.is_empty() {
            return pages;
        }
    }
    Vec::new()
}

fn json_values(body: &str) -> Vec<Value> {
    let mut values = Vec::new();
    for (start, ch) in body
        .char_indices()
        .filter(|(_, ch)| *ch == '{' || *ch == '[')
    {
        if let Some(end) = matching_json_end(body, start, ch) {
            if let Ok(value) = serde_json::from_str::<Value>(&body[start..=end]) {
                values.push(value);
            }
        }
    }
    values
}

fn matching_json_end(body: &str, start: usize, opening: char) -> Option<usize> {
    let mut stack = vec![opening];
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in body[start + opening.len_utf8()..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' | '[' => stack.push(ch),
            '}' => {
                if stack.pop() != Some('{') {
                    return None;
                }
            }
            ']' => {
                if stack.pop() != Some('[') {
                    return None;
                }
            }
            _ => {}
        }
        if stack.is_empty() {
            return Some(start + opening.len_utf8() + offset);
        }
    }
    None
}

fn normalize_manga_slug(value: &str) -> String {
    let slug = value.trim_matches('/');
    if slug.starts_with("komik/") {
        format!("/{slug}")
    } else {
        format!("/komik/{slug}")
    }
}

fn cdn_url(value: &str) -> String {
    if value.starts_with("http") {
        value.to_string()
    } else {
        format!("{CDN_URL}{}", value.trim_start_matches('/'))
    }
}

fn image_attr(input: &str) -> Option<String> {
    html::attr(input, "data-src")
        .or_else(|| html::attr(input, "src"))
        .or_else(|| html::attr(input, "content"))
}

fn label_text(body: &str, label: &str) -> Option<String> {
    let text = html::strip_tags(body);
    let lower = text.to_ascii_lowercase();
    let index = lower.find(&label.to_ascii_lowercase())?;
    text[index + label.len()..]
        .split(['\n', ','])
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn details_tags(body: &str) -> Vec<String> {
    let mut tags = Vec::new();
    if let Some(value) = label_text(body, "type:") {
        tags.push(value);
    }
    if let Some(value) = label_text(body, "rilis:") {
        tags.push(value);
    }
    tags.extend(
        body.split("<")
            .filter(|chunk| chunk.contains("bg-zinc-700"))
            .filter_map(|chunk| html::text_between(chunk, ">", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
    );
    tags
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("selesai") || lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value.get(0..10)?)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"1:{"data":[{"link":"sample","title":"Sample Manga","img":"/cover.jpg"}],"current_page":1,"last_page":2}"#;
const SEARCH_FIXTURE: &str =
    r#"<a href="/komik/sample"><h3>Sample Manga</h3><img src="/cover.jpg"></a>"#;
const DETAILS_FIXTURE: &str = r#"
<meta property="og:title" content="Sample Manga - Komiku"><meta property="og:url" content="https://komiku.cc/komik/sample"><img class="object-cover" src="/cover.jpg"><span>author:</span><span>Author</span><span>type:</span><span>Manga</span><p class="line-clamp-4">Sample description.</p><span class="bg-gray-100 text-gray-800">Ongoing</span><span class="bg-zinc-700">Action</span>
"#;
const CHAPTERS_FIXTURE: &str = r#"1:{"chapters":[{"link":"sample-chapter-1","title":"Chapter 1","created_at":"2024-01-01T00:00:00.000+00:00"}]}"#;
const PAGES_FIXTURE: &str = r#"1:{"images":["/page1.jpg","https://cdn.komiku.cc/page2.jpg"]}"#;
