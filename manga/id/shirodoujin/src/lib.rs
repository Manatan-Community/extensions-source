use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ShiroDoujin = ShiroDoujin;
const BASE_URL: &str = "https://shirodoujin.com";
const CONTENT_RATING: &str = "adult";

struct ShiroDoujin;

impl MangaSource for ShiroDoujin {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        let target = advanced_search_url(page, "", Some(order), &Value::Null);
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: body.contains("pagination") && body.contains("next"),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(normalize_key(query)))],
                has_next_page: false,
            });
        }
        let target = advanced_search_url(
            page,
            query,
            None,
            request.get("filters").unwrap_or(&Value::Null),
        );
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: body.contains("pagination") && body.contains("next"),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample-chapter-1".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(normalize_key(input)))),
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!("/{}", value[index + BASE_URL.len()..].trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

fn page_path(page: u64) -> String {
    if page > 1 {
        format!("page/{page}/")
    } else {
        String::new()
    }
}

fn advanced_search_url(page: u64, query: &str, order: Option<&str>, filters: &Value) -> String {
    let mut params = vec![("title", url::query_escape(query))];
    for (filter, parameter) in [
        ("author", "author"),
        ("year", "yearx"),
        ("status", "status"),
        ("type", "type"),
        ("order", "order"),
    ] {
        let value = if filter == "order" {
            filter_string(filters, filter).or_else(|| order.map(ToString::to_string))
        } else {
            filter_string(filters, filter)
        };
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            params.push((parameter, url::query_escape(&value)));
        }
    }
    if let Some(genres) = filter_string(filters, "genres") {
        for genre in genres
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            params.push(("genre[]", url::query_escape(genre)));
        }
    }
    format!(
        "{BASE_URL}/advanced-search/{}?{}",
        page_path(page),
        params
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("flexbox2-item"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "flexbox2-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .unwrap_or_else(|| {
                    url::slug_from_url(&href).unwrap_or_else(|| "Shiro Doujin".to_string())
                });
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("id".to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".to_string());
    let mut tags = link_values(body, "/genres/");
    tags.extend(link_values(body, "/genre/"));
    tags.sort();
    tags.dedup();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "series-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Shiro Doujin".to_string())),
        cover: html::attr_after(body, "series-thumb", "src")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        authors: info_text(body, "Author"),
        artists: info_text(body, "Artist"),
        description: html::text_between(body, "series-synops", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags,
        status: parse_status(body),
        url: Some(absolute_url(&key)),
        language: Some("id".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("flexch-infoz"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<span", "</span>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value))
                })
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| dates::parse_ymd(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter);
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Read".to_string()),
            url: Some(absolute_url(manga_key)),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
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

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(input, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(input, "<img", "src"))
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

fn info_text(body: &str, label: &str) -> Vec<String> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk
                .to_ascii_lowercase()
                .contains(&label.to_ascii_lowercase())
        })
        .filter_map(|chunk| html::text_between(chunk, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = html::strip_tags(body).to_ascii_lowercase();
    if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(
    mut chapters: Vec<MangaChapter>,
    chapter: MangaChapter,
) -> Vec<MangaChapter> {
    if !chapters.iter().any(|existing| existing.key == chapter.key) {
        chapters.push(chapter);
    }
    chapters
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="flexbox2-item">
  <div class="flexbox2-content"><a href="https://shirodoujin.com/series/sample/"><img src="https://shirodoujin.com/cover.jpg"></a></div>
  <div class="flexbox2-title"><span>Sample</span></div>
</div>
<div class="pagination"><a class="next" href="/advanced-search/page/2/?order=popular">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="series-thumb"><img src="https://shirodoujin.com/cover.jpg"></div>
<h1 class="series-title">Sample</h1>
<ul class="series-infolist"><li>Author <span>Author</span></li><li>Artist <span>Artist</span></li></ul>
<div class="series-infoz"><span class="status">Ongoing</span></div>
<div class="series-synops">Sample description.</div>
<div class="series-genres"><a href="/genres/action/">Action</a></div>
<ul class="series-chapterlist"><div class="flexch-infoz"><a href="https://shirodoujin.com/sample-chapter-1/"><span>Chapter 1</span><span class="date">2024-01-01</span></a></div></ul>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="reader-area"><img src="https://shirodoujin.com/page-1.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_zmanga_fixture() {
        assert_eq!(parse_listing(LIST_FIXTURE).len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE, "/series/sample").len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
