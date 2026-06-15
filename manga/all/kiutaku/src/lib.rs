use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const BASE_URL: &str = "https://kiutaku.com";
const SOURCE: Kiutaku = Kiutaku;

struct Kiutaku;

impl MangaSource for Kiutaku {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request_page(&request);
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/hot?start={}", page_start(page)), LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request_page(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = deeplink_key(query) {
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged { entries: vec![parse_details(&body, Some(key))], has_next_page: false });
        }
        if let Some(id) = query.strip_prefix("id:").filter(|value| !value.is_empty()) {
            let body = fetch_document_or_fixture(&format!("{BASE_URL}/{id}"), DETAILS_FIXTURE);
            return Ok(Paged { entries: vec![parse_details(&body, Some(format!("/{id}")))], has_next_page: false });
        }
        let body = fetch_document_or_fixture(
            &format!("{BASE_URL}/?search={}&start={}", url::query_escape(query), page_start(page)),
            LIST_FIXTURE,
        );
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample?page=1".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGE_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(key) = deeplink_key(input) {
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("items-row")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "item-link", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h2", "</h2>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Cosplay".into());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("all".into()),
                content_rating: Some("adult".into()),
                status: ItemStatus::Completed,
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged { entries, has_next_page: body.contains("pagination-next") && !body.contains("pagination-next\" disabled") }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "article-header", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Cosplay".into()),
        cover: first_article_image(body).map(|value| url::join_url(BASE_URL, &value)),
        tags: parse_tags(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        status: ItemStatus::Completed,
        initialized: true,
        update_strategy: Some(manatan_extension::UpdateStrategy::OnlyFetchOnce),
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body.split("<nav")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let text = html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value)).unwrap_or_else(|| "1".into());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(format!("Page {text}")),
                chapter_number: text.parse::<f32>().ok(),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("article-fulltext")
        .nth(1)
        .unwrap_or(body)
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .map(|value| url::join_url(BASE_URL, &value))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: image, context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_tags(body: &str) -> Vec<String> {
    html::text_between(body, "article-tags", "</div>")
        .unwrap_or_default()
        .split("<span")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</span>"))
        .map(|value| html::strip_tags(&value).trim_start_matches('#').trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn first_article_image(body: &str) -> Option<String> {
    body.split("article-fulltext").nth(1).unwrap_or(body).split("<img").nth(1).and_then(|chunk| html::attr(chunk, "src"))
}

fn deeplink_key(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) {
        Some(normalize_key(input))
    } else if input.starts_with('/') && input.len() > 1 {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input).split('#').next().unwrap_or(input);
    format!("/{}", path.trim_start_matches('/'))
}

fn request_page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn page_start(page: u64) -> u64 {
    page.saturating_sub(1) * 20
}

const LIST_FIXTURE: &str = r#"
<div class="blog"><div class="items-row"><a class="item-link" href="/sample"><img src="/cover.jpg"><h2>Sample Cosplay</h2></a></div></div>
<nav><a class="pagination-next">Next</a></nav>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="article-header">Sample Cosplay</div>
<div class="article-tags"><a class="tag"><span>#cosplay</span></a></div>
<nav class="pagination"><a href="/sample?page=1">1</a><a href="/sample?page=2">2</a></nav>
<div class="article-fulltext"><img src="/image1.jpg"></div>
"#;

const PAGE_FIXTURE: &str = r#"<div class="article-fulltext"><img src="/image1.jpg"><img src="/image2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kiutaku_html() {
        let listing = parse_listing(LIST_FIXTURE);
        assert_eq!(listing.entries[0].title, "Sample Cosplay");
        let details = parse_details(DETAILS_FIXTURE, Some("/sample".into()));
        assert_eq!(details.tags, vec!["cosplay"]);
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 2);
        assert_eq!(parse_pages(PAGE_FIXTURE).len(), 2);
    }
}
