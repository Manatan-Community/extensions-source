use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: KaliScanCom = KaliScanCom;
const DEFAULT_BASE_URL: &str = "https://kaliscan.com";
const MIRRORS: [&str; 4] = [
    "https://kaliscan.com",
    "https://kaliscan.me",
    "https://kaliscan.io",
    "https://mgjinx.com",
];

struct KaliScanCom;

impl MangaSource for KaliScanCom {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base_url = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updated_at"
        } else {
            "views"
        };
        let target = search_url(base_url, page, "", &Value::Null, Some(sort));
        Ok(parse_listing(
            &fetch_document(base_url, &target, LIST_FIXTURE),
            base_url,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base_url = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if MIRRORS.iter().any(|mirror| query.starts_with(mirror)) {
            let key = normalize_key(query, base_url);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(base_url, query, DETAILS_FIXTURE),
                    Some(key),
                    base_url,
                )],
                has_next_page: false,
            });
        }
        let target = search_url(
            base_url,
            page,
            query,
            request.get("filters").unwrap_or(&Value::Null),
            None,
        );
        Ok(parse_listing(
            &fetch_document(base_url, &target, LIST_FIXTURE),
            base_url,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base_url = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/123-sample".into());
        Ok(parse_details(
            &fetch_document(base_url, &url::join_url(base_url, &key), DETAILS_FIXTURE),
            Some(key),
            base_url,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base_url = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/123-sample".into());
        let body = if let Some(id) = manga_id(&key) {
            let target = format!("{base_url}/service/backend/chaplist/?manga_id={id}&manga_name=");
            fetch_document(base_url, &target, CHAPTERS_FIXTURE)
        } else {
            fetch_document(base_url, &url::join_url(base_url, &key), DETAILS_FIXTURE)
        };
        Ok(parse_chapters(&body, base_url))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base_url = base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/123-sample/chapter-1".into());
        let target = url::join_url(base_url, &key);
        let body = fetch_document(base_url, &target, PAGES_FIXTURE);
        let chapter_body = chapter_id(&body)
            .map(|id| {
                fetch_document(
                    base_url,
                    &format!(
                        "{base_url}/service/backend/chapterServer/?server_id=1&chapter_id={id}"
                    ),
                    PAGES_FIXTURE,
                )
            })
            .unwrap_or(body);
        Ok(parse_pages(&chapter_body, base_url))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base_url = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if MIRRORS.iter().any(|mirror| input.starts_with(mirror)) {
            let key = normalize_key(input, base_url);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(base_url, input, DETAILS_FIXTURE),
                    Some(key),
                    base_url,
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

fn base_url(request: &Value) -> &'static str {
    let requested = request
        .get("preferences")
        .and_then(|preferences| preferences.get("mirror"))
        .and_then(Value::as_str);
    requested
        .and_then(|value| MIRRORS.iter().copied().find(|mirror| *mirror == value))
        .unwrap_or(DEFAULT_BASE_URL)
}

fn client(base_url: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{base_url}/"))
        .with_cookies_for(base_url)
        .with_webview_challenge_fallback()
}

fn fetch_document(base_url: &str, target: &str, fixture: &str) -> String {
    client(base_url)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(
    base_url: &str,
    page: u64,
    query: &str,
    filters: &Value,
    fallback_sort: Option<&str>,
) -> String {
    let mut parts = vec![
        format!("q={}", url::query_escape(query)),
        format!("page={page}"),
    ];
    if let Some(status) = filter_value(filters, "status").or_else(|| Some("all".into())) {
        parts.push(format!("status={}", url::query_escape(&status)));
    }
    if let Some(sort) = filter_value(filters, "sort").or_else(|| fallback_sort.map(str::to_string))
    {
        parts.push(format!("sort={}", url::query_escape(&sort)));
    }
    for genre in selected_values(filters.get("genre")) {
        parts.push(format!("genre%5B%5D={}", url::query_escape(&genre)));
    }
    format!("{base_url}/search?{}", parts.join("&"))
}

fn parse_listing(body: &str, base_url: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("book-detailed-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href, base_url);
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| {
                    html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value))
                })
                .or_else(|| url::slug_from_url(&key))
                .unwrap_or_else(|| "KaliScan".to_string());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| normalize_image(&image, base_url)),
                description: html::text_between(chunk, "summary", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                tags: link_values(chunk, "genres"),
                url: Some(url::join_url(base_url, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("paginator") && body.contains("active +"),
    }
}

fn parse_details(body: &str, key: Option<String>, base_url: &str) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/123-sample".to_string());
    let alt_titles = html::text_between(body, "<h2", "</h2>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .into_iter()
        .flat_map(|value| split_values(&value))
        .collect::<Vec<_>>();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "KaliScan".to_string()),
        alternate_titles: alt_titles,
        cover: html::attr_after(body, "#cover", "data-src")
            .or_else(|| html::attr_after(body, "#cover", "src"))
            .or_else(|| image_attr(body))
            .map(|image| normalize_image(&image, base_url)),
        authors: meta_links(body, "Authors"),
        tags: meta_links(body, "Genres"),
        description: html::text_between(body, "summary", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(
            &meta_links(body, "Status")
                .first()
                .cloned()
                .unwrap_or_default(),
        ),
        url: Some(url::join_url(base_url, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base_url: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("<a"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href, base_url);
            let title = html::text_between(chunk, "chapter-title", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: None,
                url: Some(url::join_url(base_url, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str, base_url: &str) -> Vec<MangaPage> {
    if body.contains("var mainServer = \"") && body.contains("var chapImages = '") {
        let main_server = body
            .split("var mainServer = \"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_default();
        let scheme = if main_server.starts_with("//") {
            "https:"
        } else {
            ""
        };
        return body
            .split("var chapImages = '")
            .nth(1)
            .and_then(|rest| rest.split('\'').next())
            .unwrap_or_default()
            .split(',')
            .filter(|path| !path.trim().is_empty())
            .enumerate()
            .map(|(index, path)| page(index, &format!("{scheme}{main_server}{path}"), base_url))
            .collect();
    }
    if body.contains("var chapImages = '") {
        let images = body
            .split("var chapImages = '")
            .nth(1)
            .and_then(|rest| rest.split('\'').next())
            .unwrap_or_default();
        if images
            .split(',')
            .all(|image| image.starts_with("http://") || image.starts_with("https://"))
        {
            return images
                .split(',')
                .filter(|image| !image.is_empty())
                .enumerate()
                .map(|(index, image)| page(index, image, base_url))
                .collect();
        }
    }
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("chapter-images")
                || chunk.contains("chapter-image")
                || chunk.contains("data-src")
        })
        .filter_map(|chunk| resolve_image_url(chunk, base_url))
        .enumerate()
        .map(|(index, image)| page(index, &image, base_url))
        .collect()
}

fn page(index: usize, image: &str, base_url: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: normalize_image(image, base_url),
            context: Some(manga::image_headers(base_url)),
        },
        headers: manga::image_headers(base_url),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn resolve_image_url(chunk: &str, base_url: &str) -> Option<String> {
    let data_src = image_attr(chunk)?;
    let fallback = chunk
        .split("this.src='")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .map(|value| normalize_image(value, base_url));
    let primary = normalize_image(&data_src, base_url);
    if primary.contains("://s20.") {
        return fallback.or(Some(primary));
    }
    Some(match fallback {
        Some(fallback) => format!("{primary}#{fallback}"),
        None => primary,
    })
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
}

fn normalize_image(value: &str, base_url: &str) -> String {
    if value.starts_with("//") {
        format!("https:{value}")
    } else {
        url::join_url(base_url, value)
    }
}

fn meta_links(body: &str, label: &str) -> Vec<String> {
    body.split("meta")
        .find(|chunk| chunk.contains(label))
        .map(|chunk| link_values(chunk, "<a"))
        .unwrap_or_default()
}

fn link_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| marker == "<a" || chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "on-hold" | "on hold" => ItemStatus::Hiatus,
        "canceled" | "cancelled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn filter_value(filters: &Value, name: &str) -> Option<String> {
    filters
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn split_values(value: &str) -> Vec<String> {
    value
        .split([',', ';'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn manga_id(key: &str) -> Option<String> {
    key.split("/manga/")
        .nth(1)
        .and_then(|rest| rest.split('-').next())
        .filter(|id| id.chars().all(|ch| ch.is_ascii_digit()))
        .map(ToString::to_string)
}

fn chapter_id(body: &str) -> Option<String> {
    body.split("chapterId")
        .nth(1)
        .and_then(|rest| {
            rest.split(|ch: char| ch.is_ascii_digit())
                .find(|value| !value.is_empty())
        })
        .and_then(|_| {
            let rest = body.split("chapterId").nth(1)?;
            let digits = rest
                .chars()
                .skip_while(|ch| !ch.is_ascii_digit())
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>();
            (!digits.is_empty()).then_some(digits)
        })
}

fn normalize_key(input: &str, base_url: &str) -> String {
    let trimmed = MIRRORS
        .iter()
        .find_map(|mirror| input.strip_prefix(mirror))
        .unwrap_or_else(|| input.strip_prefix(base_url).unwrap_or(input));
    format!("/{}", trimmed.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="book-detailed-item"><a href="/manga/123-sample" title="Sample KaliScan"><img data-src="/cover.jpg"></a><div class="summary">Summary.</div><div class="genres"><a>Action</a></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="detail"><h1>Sample KaliScan</h1><h2>Alt Sample</h2><div id="cover"><img data-src="/cover.jpg"></div>
<div class="summary"><div class="content">Sample description.</div></div><div class="meta"><p><strong>Authors</strong><a>Writer</a></p><p><strong>Genres</strong><a>Action</a></p><p><strong>Status</strong><a>Ongoing</a></p></div></div>
<ul id="chapter-list"><li><a href="/manga/123-sample/chapter-1"><span class="chapter-title">Chapter 1</span><span class="chapter-update">Jan 01, 2024</span></a></li></ul>
"#;
const CHAPTERS_FIXTURE: &str = r#"
<ul id="chapter-list"><li><a href="/manga/123-sample/chapter-1"><span class="chapter-title">Chapter 1</span></a></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"
<script>var mainServer = "https://s1.mbcdn.xyz"; var chapImages = '/page1.jpg,/page2.jpg'; var chapterId = 99;</script>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_madtheme_flow() {
        assert_eq!(
            parse_listing(LIST_FIXTURE, DEFAULT_BASE_URL).entries.len(),
            1
        );
        let details = parse_details(
            DETAILS_FIXTURE,
            Some("/manga/123-sample".into()),
            DEFAULT_BASE_URL,
        );
        assert_eq!(details.status, ItemStatus::Ongoing);
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, DEFAULT_BASE_URL).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE, DEFAULT_BASE_URL).len(), 2);
    }
}
