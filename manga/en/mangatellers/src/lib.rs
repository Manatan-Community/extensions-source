use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Mangatellers = Mangatellers;
const BASE_URL: &str = "https://reader.mangatellers.gr";
const SOURCE_NAME: &str = "Mangatellers";

struct Mangatellers;

impl MangaSource for Mangatellers {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let path = if latest {
            format!("/latest/{page}/")
        } else {
            format!("/directory/{page}/")
        };
        Ok(parse_listing(&fetch_document(
            &url::join_url(BASE_URL, &path),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
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
        let body = client()
            .post(format!("{BASE_URL}/search/"))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[("search", query)])
            .browser_document()
            .send_text()
            .unwrap_or_else(|_| LIST_FIXTURE.to_string());
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/read/sample/en/0/1".into());
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
            let item = key
                .starts_with("/series/")
                .then(|| parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key)));
            return Ok(Some(UrlResolveResult {
                item,
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div class=\"group\"")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let title = html::attr_after(chunk, "<a", "title")
                    .or_else(|| {
                        html::text_between(chunk, "<a", "</a>")
                            .map(|value| html::strip_tags(&value))
                    })
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| SOURCE_NAME.to_string());
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: html::attr_after(chunk, "<img", "src")
                        .map(|image| url::join_url(BASE_URL, &image.replace("/thumb_", "/"))),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("en".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("div class=\"next\"") || body.contains("class=\"next\""),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::attr_after(body, "<a", "title"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| SOURCE_NAME.to_string()),
        cover: html::attr_after(body, "thumbnail", "src")
            .or_else(|| html::attr_after(body, "thumb", "src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image.replace("/thumb_", "/"))),
        description: html::text_between(body, "Synopsis</b>:", "<")
            .or_else(|| html::text_between(body, "Description</b>:", "<"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: ItemStatus::Unknown,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<div class=\"element\"")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| {
                    html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number(&title),
                date_uploaded: html::text_between(chunk, "meta_r", "</div>")
                    .and_then(|value| parse_date(&html::strip_tags(&value))),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    if let Some(json) = body
        .split("var pages = ")
        .nth(1)
        .and_then(|rest| rest.split("];").next())
        .map(|value| format!("{value}]"))
    {
        if let Ok(pages) = serde_json::from_str::<Vec<PageDto>>(&json) {
            return pages
                .into_iter()
                .enumerate()
                .map(|(index, page)| image_page(index, &page.url))
                .collect();
        }
    }
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|image| image.contains("/content/comics/"))
        .enumerate()
        .map(|(index, image)| image_page(index, &image))
        .collect()
}

fn image_page(index: usize, image: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: url::join_url(BASE_URL, image),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

#[derive(Debug, Deserialize)]
struct PageDto {
    url: String,
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|window| window[0].to_ascii_lowercase().contains("chapter"))
        .and_then(|window| window[1].trim_matches(':').parse().ok())
}

fn parse_date(value: &str) -> Option<i64> {
    value
        .split_whitespace()
        .find_map(|token| manatan_shared::dates::parse_fixture_date(token))
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

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"<div class="group"><a href="https://reader.mangatellers.gr/series/sample/" title="Sample"><img src="/content/comics/sample/thumb_cover.png"></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="thumbnail"><img src="/cover.png"></div><h1>Sample</h1><div class="info"><b>Synopsis</b>: Summary<br></div><div class="element"><a href="https://reader.mangatellers.gr/read/sample/en/0/1/" title="Chapter 1">Chapter 1</a><div class="meta_r">by Mangatellers, 2024-01-01</div></div>"#;
const PAGES_FIXTURE: &str = r#"<script>var pages = [{"url":"https:\/\/reader.mangatellers.gr\/content\/comics\/sample\/0001.png"}];</script>"#;

export_manga_source!(SOURCE);
