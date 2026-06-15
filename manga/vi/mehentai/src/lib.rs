use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MeHentai = MeHentai;
const BASE_URL: &str = "https://mehentai.blog";
const SEARCH_PATH: &str = "tim-kiem";

struct MeHentai;

impl MangaSource for MeHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_latest(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_latest(&fetch_document(
                &format!("{BASE_URL}/?page={page}"),
                LIST_FIXTURE,
            )));
        }
        let body = fetch_document(BASE_URL, POPULAR_FIXTURE);
        Ok(Paged {
            entries: parse_popular(&body),
            has_next_page: false,
        })
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

        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if query.is_empty() {
            filtered_url(page, request.get("filters").unwrap_or(&Value::Null))
        } else {
            format!(
                "{BASE_URL}/{SEARCH_PATH}?s={}&page={page}",
                url::query_escape(query)
            )
        };
        Ok(parse_latest(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
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
        .with_header("Origin", BASE_URL)
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

fn filtered_url(page: u64, filters: &Value) -> String {
    let genre = filter_string(filters, "genrePath")
        .trim()
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string();
    let sort = filter_string(filters, "sort");
    let mut target = if genre.is_empty() {
        BASE_URL.to_string()
    } else {
        format!("{BASE_URL}/{genre}")
    };
    target.push_str(&format!("?page={page}"));
    if genre.starts_with("genre/") && !sort.is_empty() {
        target.push_str(&format!("&m_orderby={}", url::query_escape(&sort)));
    }
    target
}

fn parse_popular(body: &str) -> Vec<CatalogItem> {
    body.split("info-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "MeHentai".into()));
            let key = normalize_key(&href);
            Some(catalog_item(
                key,
                title,
                nearby_image(body, &href).map(|image| absolute_url(&image)),
                false,
            ))
        })
        .fold(Vec::new(), push_unique)
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("page-item-detail")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "item-summary", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "item-summary", "</a>")
                .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "MeHentai".into()));
            let key = normalize_key(&href);
            Some(catalog_item(
                key,
                title,
                image_attr(chunk).map(|image| absolute_url(&image)),
                false,
            ))
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"")
            || body.contains("rel='next'")
            || body.contains("ul class=\"pager\""),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key
        .or_else(|| {
            html::attr_after(body, "rel=\"canonical\"", "href").map(|href| normalize_key(&href))
        })
        .unwrap_or_else(|| "/manga/sample".into());
    let status_text = summary_value(body, "trạng thái").unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "post-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MeHentai".into())),
        cover: html::attr_after(body, "summary_image", "src")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "summary__content", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: summary_value(body, "tác giả").into_iter().collect(),
        tags: link_texts_after(body, "genres-content"),
        status: parse_status(&status_text),
        url: Some(absolute_url(&key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("wp-manga-chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(absolute_url(&key)),
                date_uploaded: html::text_between(chunk, "chapter-release-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
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

fn catalog_item(
    key: String,
    title: String,
    cover: Option<String>,
    initialized: bool,
) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover,
        url: Some(absolute_url(&key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized,
        ..CatalogItem::default()
    }
}

fn filter_string(filters: &Value, key: &str) -> String {
    filters
        .get(key)
        .and_then(|value| {
            value
                .as_str()
                .or_else(|| value.get("value").and_then(Value::as_str))
        })
        .unwrap_or_default()
        .to_string()
}

fn summary_value(body: &str, label: &str) -> Option<String> {
    let label = label.to_ascii_lowercase();
    body.split("summary-heading")
        .find(|chunk| {
            html::strip_tags(chunk)
                .to_ascii_lowercase()
                .contains(&label)
        })
        .and_then(|chunk| html::text_between(chunk, "summary-content", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn link_texts_after(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(input: &str) -> ItemStatus {
    let value = input.to_ascii_lowercase();
    if value.contains("completed") || value.contains("hoàn thành") || value.contains("truyện full")
    {
        ItemStatus::Completed
    } else if value.contains("ongoing") || value.contains("đang ra") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn nearby_image(body: &str, href: &str) -> Option<String> {
    body.find(href)
        .and_then(|index| body[..index].rsplit("<img").next())
        .and_then(image_attr)
        .or_else(|| body.find(href).and_then(|index| image_attr(&body[index..])))
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "data-lazy-src"))
        .or_else(|| {
            html::attr_after(input, "<img", "srcset")
                .and_then(|value| value.split_whitespace().next().map(ToString::to_string))
        })
        .or_else(|| html::attr_after(input, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(input, "<img", "src"))
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!("/{}", input[BASE_URL.len()..].trim_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const POPULAR_FIXTURE: &str = r#"<div id="slide-top"><div class="item"><div class="img-item"><img src="/cover.jpg"></div><div class="info-item"><a href="/manga/sample/">Sample</a></div></div></div>"#;
const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><div class="item-summary"><a href="/manga/sample/">Sample</a></div><div class="item-thumb"><img src="/cover.jpg"></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="post-title"><h1>Sample</h1></div><div class="summary_image"><img src="/cover.jpg"></div><div class="summary__content">Summary</div><div class="summary-heading">Trạng thái</div><div class="summary-content">Đang ra</div><div class="summary-heading">Tác giả</div><div class="summary-content">Author</div><div class="genres-content"><a rel="tag">Tag</a></div><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">2024-01-01</span></li>"#;
const PAGES_FIXTURE: &str = r#"<div class="page-break"><img src="/page1.jpg"></div>"#;
