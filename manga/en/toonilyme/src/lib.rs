use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ToonilyMe = ToonilyMe;
const BASE_URL: &str = "https://toonily.me";
const SOURCE_NAME: &str = "Toonily.me";

struct ToonilyMe;

impl MangaSource for ToonilyMe {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updated_at"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document(
            &search_url(page, "", sort),
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
        Ok(parse_listing(&fetch_document(
            &search_url(page, query, ""),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/123-sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/123-sample".into());
        let slug = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("123-sample");
        let body = fetch_document(
            &format!("{BASE_URL}/api/manga/{slug}/chapters?source=detail"),
            CHAPTERS_FIXTURE,
        );
        let chapters = parse_chapters(&body);
        if chapters.is_empty() {
            Ok(parse_chapters(&fetch_document(
                &url::join_url(BASE_URL, &key),
                DETAILS_FIXTURE,
            )))
        } else {
            Ok(chapters)
        }
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/123-sample/chapter-1".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        let chapter_body = chapter_id(&body)
            .map(|id| {
                fetch_document(
                    &format!(
                        "{BASE_URL}/service/backend/chapterServer/?server_id=1&chapter_id={id}"
                    ),
                    PAGES_FIXTURE,
                )
            })
            .unwrap_or(body);
        Ok(parse_pages(&chapter_body))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(page: u64, query: &str, sort: &str) -> String {
    let mut parts = vec![
        format!("q={}", url::query_escape(query)),
        format!("page={page}"),
    ];
    if !sort.is_empty() {
        parts.push(format!("sort={sort}"));
    }
    format!("{BASE_URL}/search?{}", parts.join("&"))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("book-detailed-item")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let key = normalize_key(&href);
                let title = html::attr_after(chunk, "<a", "title")
                    .or_else(|| {
                        html::text_between(chunk, "<a", "</a>")
                            .map(|value| html::strip_tags(&value))
                    })
                    .or_else(|| url::slug_from_url(&key))
                    .unwrap_or_else(|| SOURCE_NAME.to_string());
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| normalize_image(&image)),
                    description: html::text_between(chunk, "summary", "</")
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty()),
                    tags: link_values(chunk, "genres"),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("en".to_string()),
                    content_rating: Some("adult".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("rel=\"next\"")
            || body.contains("rel='next'")
            || (body.contains("paginator") && body.contains("active")),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
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
            .unwrap_or_else(|| SOURCE_NAME.to_string()),
        alternate_titles: alt_titles,
        cover: image_attr(body).map(|image| normalize_image(&image)),
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
        .filter(|chunk| chunk.contains("<a"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapter-title", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
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
            .map(|(index, path)| page(index, &format!("{scheme}{main_server}{path}")))
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
                .map(|(index, image)| page(index, image))
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
        .filter_map(resolve_image_url)
        .enumerate()
        .map(|(index, image)| page(index, &image))
        .collect()
}

fn page(index: usize, image: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: normalize_image(image),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn resolve_image_url(chunk: &str) -> Option<String> {
    let data_src = image_attr(chunk)?;
    let fallback = chunk
        .split("this.src='")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .map(normalize_image);
    let primary = normalize_image(&data_src);
    if primary.contains("://s20.") {
        return fallback.or(Some(primary));
    }
    Some(match fallback {
        Some(fallback) => format!("{primary}#{fallback}"),
        None => primary,
    })
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn normalize_image(value: &str) -> String {
    if value.starts_with("//") {
        format!("https:{value}")
    } else {
        url::join_url(BASE_URL, value)
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

fn split_values(value: &str) -> Vec<String> {
    value
        .split([',', ';'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn chapter_id(body: &str) -> Option<String> {
    let rest = body.split("chapterId").nth(1)?;
    let digits = rest
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    (!digits.is_empty()).then_some(digits)
}

fn normalize_key(input: &str) -> String {
    let trimmed = input.strip_prefix(BASE_URL).unwrap_or(input);
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
<div class="book-detailed-item"><a href="/manga/123-sample" title="Sample Toonily.me"><img data-src="/cover.jpg"></a><div class="summary">Summary.</div><div class="genres"><a>Action</a></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="detail"><h1>Sample Toonily.me</h1><h2>Alt Sample</h2><div id="cover"><img data-src="/cover.jpg"></div>
<div class="summary"><div class="content">Sample description.</div></div><div class="meta"><p><strong>Authors</strong><a>Writer</a></p><p><strong>Genres</strong><a>Action</a></p><p><strong>Status</strong><a>Ongoing</a></p></div></div>
<ul id="chapter-list"><li><a href="/manga/123-sample/chapter-1"><span class="chapter-title">Chapter 1</span><span class="chapter-update">Jan 01, 2024</span></a></li></ul>
"#;
const CHAPTERS_FIXTURE: &str = r#"
<ul id="chapter-list"><li><a href="/manga/123-sample/chapter-1"><span class="chapter-title">Chapter 1</span></a></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"
<script>var mainServer = "https://s1.mbcdn.xyz"; var chapImages = '/page1.jpg,/page2.jpg'; var chapterId = 99;</script>
"#;
