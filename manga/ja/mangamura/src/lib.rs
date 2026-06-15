use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaMura = MangaMura;
const BASE_URL: &str = "https://mangamura.me";
const CONTENT_RATING: &str = "suggestive";

struct MangaMura;

impl MangaSource for MangaMura {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .or_else(|| request.get("listing"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let path = if listing == "latest" {
            format!("{BASE_URL}/latest-update?page={page}")
        } else {
            format!("{BASE_URL}/hot-manga?page={page}")
        };
        Ok(parse_listing(&fetch_document(&path, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query).filter(|key| key.starts_with("/manga/")) {
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut target = format!("{BASE_URL}/?q={}&p={page}", url::query_escape(query));
        for id in ["type", "status", "language", "sort"] {
            if let Some(value) =
                filter_string(&request, id).filter(|value| *value != "all" && *value != "default")
            {
                target.push('&');
                target.push_str(id);
                target.push('=');
                target.push_str(&url::query_escape(value));
            }
        }
        Ok(parse_listing(&fetch_document(&target, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-chapter".into());
        let chapter_id = key.rsplit('/').next().unwrap_or("sample");
        let body = fetch_document(
            &format!("{BASE_URL}/json/chapter?mode=vertical&id={chapter_id}"),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body, &absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input).filter(|key| key.starts_with("/manga/")) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
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
        .unwrap_or_else(|_| fixture.into())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("bsx") || chunk.contains("listupd") || chunk.contains("postbody")
        })
        .filter_map(parse_listing_item)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("next page-numbers") || body.contains("rel=\"next\""),
    }
}

fn parse_listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = key_from_url(&href)?;
    let title = html::attr_after(chunk, "<a", "title")
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .or_else(|| html::text_between(chunk, "tt", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga Mura".into()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_from_chunk(chunk),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn details_from_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga Mura".into())),
        cover: image_from_chunk(body),
        authors: values_by_rel(body, "author"),
        tags: values_by_rel(body, "tag"),
        description: html::text_between(body, "entry-content", "</div>")
            .or_else(|| html::text_between(body, "summary", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: if body.contains("Finished") {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("wp-manga-chapter") || chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = key_from_url(&href)?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Chapter".into()));
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let mut pages = Vec::new();
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        collect_page_urls(&value, &mut pages);
    }
    if pages.is_empty() {
        pages.extend(
            body.split("<img")
                .skip(1)
                .filter_map(|chunk| {
                    html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
                })
                .map(|image| url_page(&image, referer)),
        );
    }
    if pages.is_empty() {
        pages.push(manga::text_page(
            "No readable pages were returned for this chapter.",
        ));
    }
    pages
}

fn collect_page_urls(value: &Value, pages: &mut Vec<MangaPage>) {
    match value {
        Value::String(text) if text.starts_with("http") => pages.push(url_page(text, BASE_URL)),
        Value::Array(values) => values
            .iter()
            .for_each(|value| collect_page_urls(value, pages)),
        Value::Object(map) => {
            for key in ["url", "src", "image", "imageUrl"] {
                if let Some(text) = map
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|text| text.starts_with("http"))
                {
                    pages.push(url_page(text, BASE_URL));
                }
            }
            map.values()
                .for_each(|value| collect_page_urls(value, pages));
        }
        _ => {}
    }
}

fn url_page(image: &str, referer: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: absolute_any(image),
            context: Some(manga::image_headers(referer)),
        },
        headers: manga::image_headers(referer),
        ..MangaPage::default()
    }
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|image| absolute_any(&image))
}

fn values_by_rel(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with("/manga/") {
        return Some(normalize_key(input));
    }
    input
        .find("/manga/")
        .map(|index| normalize_key(&input[index..]))
        .or_else(|| input.strip_prefix(BASE_URL).map(normalize_key))
}

fn normalize_key(input: &str) -> String {
    let mut key = input
        .split('#')
        .next()
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim()
        .to_string();
    if key.starts_with(BASE_URL) {
        key = key[BASE_URL.len()..].to_string();
    }
    if !key.starts_with('/') {
        key.insert(0, '/');
    }
    while key.len() > 1 && key.ends_with('/') {
        key.pop();
    }
    key
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, &normalize_key(key))
}

fn absolute_any(input: &str) -> String {
    if input.starts_with("http") {
        input.into()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .or_else(|| request.get(id))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="bsx"><a href="/manga/sample" title="Sample Manga Mura"><img src="https://img.example.test/mangamura.jpg"></a></div>"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"<h1>Sample Manga Mura</h1><img src="https://img.example.test/mangamura.jpg"><div class="entry-content">Sample description.</div><li class="wp-manga-chapter"><a href="/sample-chapter">Chapter 1</a></li>"#;
const PAGES_FIXTURE: &str = r#"{"images":["https://img.example.test/mangamura-page.jpg"]}"#;
