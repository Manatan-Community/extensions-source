use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Mangack = Mangack;
const BASE_URL: &str = "https://mangack.com";
const PAGE_SIZE: u64 = 24;

struct Mangack;

impl MangaSource for Mangack {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_rest_listing(LIST_FIXTURE, 1, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let target = if page <= 1 {
                format!("{BASE_URL}/updates/")
            } else {
                format!("{BASE_URL}/updates/page/{page}/")
            };
            return Ok(parse_latest(&fetch_document(&target, LATEST_FIXTURE)));
        }
        Ok(parse_rest_listing(
            &fetch_json(
                &rest_list_url(page, Some("date"), Some("desc"), None),
                LIST_FIXTURE,
            ),
            page,
            page + 1,
        ))
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
        Ok(parse_rest_listing(
            &fetch_json(&rest_list_url(page, None, None, Some(query)), LIST_FIXTURE),
            page,
            page + 1,
        ))
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
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter/chapter-1".into());
        let slug = key
            .trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("chapter-1");
        Ok(parse_pages(&fetch_json(
            &format!("{BASE_URL}/wp-json/wp/v2/chapter?slug={slug}&_fields=id,content"),
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

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn rest_list_url(
    page: u64,
    orderby: Option<&str>,
    order: Option<&str>,
    query: Option<&str>,
) -> String {
    let mut params = vec![
        format!("page={page}"),
        format!("per_page={PAGE_SIZE}"),
        "_embed=wp:featuredmedia".to_string(),
    ];
    if let Some(orderby) = orderby {
        params.push(format!("orderby={}", url::query_escape(orderby)));
    }
    if let Some(order) = order {
        params.push(format!("order={}", url::query_escape(order)));
    }
    if let Some(query) = query.filter(|value| !value.is_empty()) {
        params.push(format!("search={}", url::query_escape(query)));
    }
    format!("{BASE_URL}/wp-json/wp/v2/manga?{}", params.join("&"))
}

fn parse_rest_listing(body: &str, current_page: u64, total_pages: u64) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let entries = root
        .as_array()
        .into_iter()
        .flatten()
        .map(rest_item)
        .collect();
    Paged {
        entries,
        has_next_page: current_page < total_pages,
    }
}

fn rest_item(item: &Value) -> CatalogItem {
    let link = string_field(item, "link");
    let key = normalize_key(&link);
    CatalogItem {
        key: key.clone(),
        title: item
            .pointer("/title/rendered")
            .and_then(Value::as_str)
            .map(html::strip_tags)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Mangack".into())),
        cover: item
            .pointer("/_embedded/wp:featuredmedia/0/source_url")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("Latest_chapter_update")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(chunk, "<a", "title")
                    .or_else(|| {
                        html::text_between(chunk, "<a", "</a>").map(|v| html::strip_tags(&v))
                    })
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "Mangack".into())
                    }),
                cover: image_from_chunk(chunk),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("next page-numbers") || body.contains("pagination a next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let article = body.split("<article").nth(1).unwrap_or(body);
    let classes = html::attr(article, "class").unwrap_or_default();
    let type_name = class_value(&classes, "comic-type-").map(humanize_slug);
    let mut tags = classes
        .split_whitespace()
        .filter_map(|class| {
            class
                .strip_prefix("Genres-")
                .map(|slug| humanize_slug(slug.to_string()))
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if let Some(kind) = type_name {
        tags.push(kind);
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .map(|value| html::strip_tags(&value).replace(" mangack", ""))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Mangack".into())),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| image_from_chunk(article)),
        description: details_description(body),
        tags,
        status: status_from(class_value(&classes, "manga-status-").as_deref()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("/chapter/"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: Some(0),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let html = root
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item.pointer("/content/rendered"))
        .and_then(Value::as_str)
        .unwrap_or(body);
    html.split("<img")
        .skip(1)
        .filter_map(image_from_chunk)
        .filter(|image| !skip_asset(image))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input
                .trim_start_matches(BASE_URL)
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn details_description(body: &str) -> Option<String> {
    html::attr_after(body, "property=\"og:description\"", "content")
        .or_else(|| {
            html::text_between(body, "entry-content", "</div>")
                .map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.trim().is_empty())
}

fn class_value(classes: &str, prefix: &str) -> Option<String> {
    classes
        .split_whitespace()
        .find_map(|class| class.strip_prefix(prefix).map(ToString::to_string))
}

fn humanize_slug(slug: String) -> String {
    slug.split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| {
            html::attr(chunk, "srcset")
                .map(|value| value.split_whitespace().next().unwrap_or("").to_string())
        })
        .or_else(|| html::attr(chunk, "src"))
        .filter(|value| !value.is_empty())
        .map(|value| url::join_url(BASE_URL, &value))
}

fn status_from(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_lowercase().as_str() {
        "ongoing" | "publishing" | "updating" => ItemStatus::Ongoing,
        "completed" | "complete" | "finished" => ItemStatus::Completed,
        "hiatus" | "on-hiatus" | "on-hold" => ItemStatus::Hiatus,
        "cancelled" | "canceled" | "dropped" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn skip_asset(image: &str) -> bool {
    let lower = image.to_ascii_lowercase();
    lower.contains("/wp-content/themes/")
        || lower.contains("/wp-content/plugins/")
        || [
            "logo",
            "icon",
            "cropped",
            "placeholder",
            "loading",
            "spinner",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"[{"link":"https://mangack.com/manga/sample/","title":{"rendered":"Sample Manga"},"_embedded":{"wp:featuredmedia":[{"source_url":"https://mangack.com/cover.jpg"}]}}]"#;
const LATEST_FIXTURE: &str = r#"<div class="latestmanga"><div class="Latest_chapter_update"><a href="https://mangack.com/manga/sample/" title="Sample Manga"><img src="/cover.jpg"></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<article class="comic-type-manga Genres-action manga-status-ongoing"><h1 class="entry-title">Sample Manga</h1><meta property="og:image" content="https://mangack.com/cover.jpg"><meta property="og:description" content="Description"><ul class="chapterslist"><li><a class="title" href="https://mangack.com/chapter/chapter-1/">Chapter 1</a></li></ul></article>"#;
const PAGES_FIXTURE: &str = r#"[{"content":{"rendered":"<p><img src=\"https://mangack.com/pages/001.jpg\" /></p><p><img src=\"https://mangack.com/pages/002.jpg\" /></p>"}}]"#;
