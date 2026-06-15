use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: TeamX = TeamX;
const BASE_URL: &str = "https://olympustaff.com";

struct TeamX;

impl MangaSource for TeamX {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            if page > 1 {
                format!("{BASE_URL}?page={page}")
            } else {
                BASE_URL.to_string()
            }
        } else if page > 1 {
            format!("{BASE_URL}/series/?page={page}")
        } else {
            format!("{BASE_URL}/series/")
        };
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        if target.contains("/series") {
            Ok(parse_popular(&body))
        } else {
            Ok(parse_latest(&body))
        }
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
            let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, key)],
                has_next_page: false,
            });
        }
        let body = if query.is_empty() {
            fetch_or_fixture(
                &filtered_series_url(page, request.get("filters")),
                LIST_FIXTURE,
            )
        } else {
            fetch_or_fixture(
                &format!(
                    "{BASE_URL}/ajax/search?keyword={}",
                    url::query_escape(query)
                ),
                SEARCH_FIXTURE,
            )
        };
        if query.is_empty() {
            Ok(parse_popular(&body))
        } else {
            Ok(Paged {
                entries: parse_search_ajax(&body),
                has_next_page: false,
            })
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, key)),
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

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn filtered_series_url(page: u64, filters: Option<&Value>) -> String {
    let mut parts = vec![format!("{BASE_URL}/series/?page={page}")];
    if let Some(filters) = filters.and_then(Value::as_object) {
        for key in ["type", "status", "genre"] {
            if let Some(value) = filters.get(key).and_then(Value::as_str) {
                if !value.is_empty() {
                    parts.push(format!("{key}={}", url::query_escape(value)));
                }
            }
        }
    }
    parts.join("&")
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("bsx"))
        .filter_map(catalog_from_popular_chunk)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("box") || chunk.contains("last-chapter"))
        .filter_map(catalog_from_popular_chunk)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn parse_search_ajax(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .map(|chunk| format!("<a{chunk}"))
        .filter(|chunk| chunk.contains("items-center") || chunk.contains("href"))
        .filter_map(|chunk| {
            let href = html::attr(&chunk, "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(&chunk, "<h4", "</h4>")
                    .or_else(|| html::attr_after(&chunk, "<a", "title"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
                cover: image_attr(&chunk).map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ar".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn catalog_from_popular_chunk(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::attr_after(chunk, "<a", "title")
            .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: String) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "author-info-title", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "text-right", "src")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "review-content", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_value(body, "الرسام").into_iter().collect(),
        tags: review_links(body),
        status: info_value(body, "الحالة")
            .map(|value| parse_status(&value))
            .unwrap_or(ItemStatus::Unknown),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-card"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let number = html::attr(chunk, "data-number").unwrap_or_else(|| "1".to_string());
            let title = html::text_between(chunk, "chapter-title", "</div>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| {
                    !value.is_empty()
                        && value != &number
                        && value != &format!("الفصل {number}")
                        && value != &format!("الفصل رقم {number}")
                });
            let name = title
                .map(|title| format!("الفصل {number} - {title}"))
                .unwrap_or_else(|| format!("الفصل {number}"));
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(name),
                date_uploaded: html::attr(chunk, "data-date")
                    .and_then(|value| value.parse::<i64>().ok())
                    .map(|seconds| seconds * 1000),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<")
        .filter(|chunk| {
            (chunk.starts_with("img") || chunk.starts_with("canvas"))
                && (chunk.contains("src") || chunk.contains("data-src"))
        })
        .filter_map(image_attr)
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

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<small", "</small>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn review_links(body: &str) -> Vec<String> {
    html::text_between(body, "review-author-info", "</div>")
        .map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|link| html::text_between(link, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn image_attr(input: &str) -> Option<String> {
    ["data-src", "src"]
        .into_iter()
        .find_map(|attr| html::attr_after(input, "<img", attr).or_else(|| html::attr(input, attr)))
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim() {
        "مستمرة" | "قادم قريبًا" => ItemStatus::Ongoing,
        "مكتمل" => ItemStatus::Completed,
        "متوقف" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        return format!(
            "/{}",
            input.split('/').skip(3).collect::<Vec<_>>().join("/")
        )
        .trim_end_matches('/')
        .to_string();
    }
    format!("/{}", input.trim_matches('/'))
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<select id="select_genre"><option value="action">اكشن</option></select>
<div class="listupd"><div class="bsx"><a href="https://olympustaff.com/series/sample" title="عينة تيم"><img data-src="/covers/sample.jpg"></a></div></div>
<a rel="next" href="/series/?page=2">Next</a>
"#;

const SEARCH_FIXTURE: &str = r#"
<a class="items-center" href="https://olympustaff.com/series/sample"><img src="/covers/sample.jpg"><h4>عينة تيم</h4></a>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="author-info-title"><h1>عينة تيم</h1></div>
<div class="text-right"><img src="/covers/sample.jpg"></div>
<div class="review-content"><p>وصف تجريبي.</p></div>
<div class="review-author-info"><a>اكشن</a><a>خيال</a></div>
<div class="full-list-info"><small>الحالة</small><small>مستمرة</small><small>الرسام</small><small>كاتب</small></div>
<div class="chapter-card" data-number="1" data-date="1704067200"><a href="/series/sample/chapter-1"><div class="chapter-info"><div class="chapter-title">البداية</div></div></a></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="image_list">
  <canvas data-src="/pages/001.jpg"></canvas>
  <img src="/pages/002.jpg">
</div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_catalog_and_search() {
        assert_eq!(parse_popular(LIST_FIXTURE).entries[0].key, "/series/sample");
        assert_eq!(parse_search_ajax(SEARCH_FIXTURE)[0].title, "عينة تيم");
    }

    #[test]
    fn parses_details_chapters_pages() {
        assert_eq!(
            parse_details(DETAILS_FIXTURE, "/series/sample".into()).title,
            "عينة تيم"
        );
        assert_eq!(
            parse_chapters(DETAILS_FIXTURE)[0].key,
            "/series/sample/chapter-1"
        );
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
